//! Notices to the operator's own targets.
//!
//! When something happens that the human may want to hear about with the app
//! closed, the hub sends a notice to each target the operator configured: a
//! self-hosted ntfy topic, a generic webhook, or a chat service. A target says
//! how much the notice may carry. At `none`, the default, it is one fixed
//! sentence that names no project, agent, title, id or count, so the target
//! learns only that the hub wants a look. A confidential project's notices are
//! always sent at `none`.
//!
//! Delivery never holds up the write that woke it: a write hands a notice to a
//! background dispatcher and returns. Each target has its own worker, which
//! sends at most once per quiet period and folds what arrives meanwhile into
//! one trailing send, drops a notice it has already queued or recently sent,
//! and retries a failed send a bounded number of times. The outcome of the last
//! send is kept per target for the operator to read.

pub mod preset;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::HeaderValue;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::store::notify_targets::StoredTarget;
pub use preset::{Facts, Preset, PresetInfo};

/// The one body a `none` notice of something waiting carries.
pub const BODY: &str = "Something is waiting for you in Agent Hub.";

/// The default quiet period between two sends to one target.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// The id the `notify_url` target from the configuration goes by.
pub const CONFIG_ID: &str = "config";

/// The longest a target's name may be, in characters.
pub const NAME_CHARS_MAX: usize = 64;

/// The longest quiet period the setting accepts.
const INTERVAL_SECS_MAX: u64 = 24 * 60 * 60;

/// How long one attempt may take, connect to response head.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How many notices wait for the dispatcher before a new one is dropped.
const DISPATCH_QUEUE: usize = 256;

/// How many notices wait for one target's worker.
const TARGET_QUEUE: usize = 64;

/// How many sent keys a worker remembers for deduplication.
const RECENT_KEYS: usize = 64;

/// What happened, from the target's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Event {
    /// A question, approval, idea or enrolment request started waiting.
    Ask,
    /// An open item is close to its deadline.
    Deadline,
    /// An agent ended a session.
    Ended,
    /// The operator asked for a test send.
    Test,
}

impl Event {
    const ALL: [Event; 3] = [Event::Ask, Event::Deadline, Event::Ended];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Deadline => "deadline",
            Self::Ended => "ended",
            Self::Test => "test",
        }
    }

    fn bit(self) -> u8 {
        match self {
            Self::Ask => 1,
            Self::Deadline => 2,
            Self::Ended => 4,
            Self::Test => 0,
        }
    }
}

/// The events a target takes. A test send goes to every target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSet(u8);

impl EventSet {
    /// What a new target takes when it names nothing.
    pub const DEFAULT: EventSet = EventSet(1 | 2);
    /// What the configuration's target takes: the ADR 0026 behaviour.
    pub const ASK: EventSet = EventSet(1);

    pub fn contains(self, event: Event) -> bool {
        event == Event::Test || self.0 & event.bit() != 0
    }

    pub fn names(self) -> Vec<&'static str> {
        Event::ALL
            .into_iter()
            .filter(|event| self.contains(*event))
            .map(Event::as_str)
            .collect()
    }

    /// Parse a list of event names. An unknown name is refused; an empty list
    /// is allowed and takes nothing but tests.
    pub fn parse(names: &[String]) -> Result<Self> {
        let mut bits = 0;
        for name in names {
            let event = Event::ALL
                .into_iter()
                .find(|event| event.as_str() == name)
                .ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "'{name}' is not a notify event; use ask, deadline or ended"
                    ))
                })?;
            bits |= event.bit();
        }
        Ok(Self(bits))
    }

    pub fn to_stored(self) -> String {
        self.names().join(",")
    }

    pub fn from_stored(value: &str) -> Self {
        let names: Vec<String> = value
            .split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        Self::parse(&names).unwrap_or(Self::DEFAULT)
    }
}

/// How much a notice may say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Content {
    /// The fixed sentence for the event, and nothing else.
    None,
    /// The kind, the project, the session name and a link back to the hub.
    Title,
    /// As `title`, and the question or approval text.
    Full,
}

impl Content {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Title => "title",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "title" => Some(Self::Title),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// The `notify_url` target from the configuration.
#[derive(Clone)]
pub struct NotifyTarget {
    /// The http or https URL the hub POSTs to.
    pub url: url::Url,
    /// Sent as `Authorization: Bearer` when set.
    pub token: Option<String>,
    /// The quiet period after a send.
    pub interval: Duration,
}

impl std::fmt::Debug for NotifyTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifyTarget")
            .field("url", &redact_url(&self.url))
            .field("token", &self.token.as_ref().map(|_| "(set)"))
            .field("interval", &self.interval)
            .finish()
    }
}

impl NotifyTarget {
    /// Validate the settings. An unset or empty URL means the feature is off.
    pub fn parse(
        url: Option<&str>,
        token: Option<&str>,
        interval_secs: Option<&str>,
    ) -> Result<Option<Self>> {
        let Some(url) = url.map(str::trim).filter(|url| !url.is_empty()) else {
            return Ok(None);
        };
        let url = parse_url(
            url,
            "HUB_NOTIFY_URL",
            "set HUB_NOTIFY_TOKEN instead",
            Error::Config,
        )?;
        let token = token
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        if let Some(token) = &token
            && HeaderValue::from_str(&format!("Bearer {token}")).is_err()
        {
            return Err(Error::Config(
                "HUB_NOTIFY_TOKEN has characters a header cannot carry".to_string(),
            ));
        }
        let interval = parse_interval(interval_secs)?;
        Ok(Some(Self {
            url,
            token,
            interval,
        }))
    }

    /// The URL as it may be printed or logged.
    pub fn redacted(&self) -> String {
        redact_url(&self.url)
    }
}

/// How the notifier delivers: the quiet period after a send, and the waits
/// between the attempts of one send.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub interval: Duration,
    /// One wait per retry, so a send makes this many attempts plus one.
    pub backoff: Vec<Duration>,
}

impl Default for Delivery {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL,
            backoff: vec![Duration::from_secs(2), Duration::from_secs(10)],
        }
    }
}

impl Delivery {
    /// The delivery settings from `notify_interval_secs`.
    pub fn parse(interval_secs: Option<&str>) -> Result<Self> {
        Ok(Self {
            interval: parse_interval(interval_secs)?,
            ..Self::default()
        })
    }
}

fn parse_url(
    value: &str,
    what: &str,
    instead: &str,
    error: fn(String) -> Error,
) -> Result<url::Url> {
    let url: url::Url = value
        .parse()
        .map_err(|err| error(format!("{what} is not a URL: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(error(format!(
            "{what} must be http or https, got '{}'",
            url.scheme()
        )));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(error(format!(
            "{what} needs a host, such as http://ntfy.lan/agent-hub"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(error(format!(
            "{what} must not carry credentials; {instead}"
        )));
    }
    if url.fragment().is_some() {
        return Err(error(format!("{what} must not carry a fragment")));
    }
    Ok(url)
}

fn parse_interval(value: Option<&str>) -> Result<Duration> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(DEFAULT_INTERVAL),
        Some(value) => match value.parse::<u64>() {
            Ok(secs) if (1..=INTERVAL_SECS_MAX).contains(&secs) => Ok(Duration::from_secs(secs)),
            _ => Err(Error::Config(format!(
                "HUB_NOTIFY_INTERVAL_SECS must be a whole number of seconds from 1 to {INTERVAL_SECS_MAX}, got '{value}'"
            ))),
        },
    }
}

/// Check a target's URL and token for a preset and return the parsed URL.
pub fn check_target(preset: Preset, url: &str, token: Option<&str>) -> Result<url::Url> {
    let url = parse_url(
        url.trim(),
        "the URL",
        "put a token in the token field",
        Error::InvalidArgument,
    )?;
    if let Some(token) = token
        && HeaderValue::from_str(&format!("Bearer {token}")).is_err()
    {
        return Err(Error::InvalidArgument(
            "the token has characters a header cannot carry".to_string(),
        ));
    }
    preset.check(&url, token)?;
    Ok(url)
}

/// A URL with everything after the origin hidden. A topic path or a query can
/// be a credential on some targets, so neither is printed.
fn redact_url(url: &url::Url) -> String {
    let origin = url.origin().ascii_serialization();
    let rest = url.path() != "/" || url.query().is_some();
    if rest {
        format!("{origin}/...")
    } else {
        origin
    }
}

/// Redact a configured URL string for display, or say it is not a URL.
pub fn redact_url_str(value: &str) -> String {
    match value.trim().parse::<url::Url>() {
        Ok(url) => redact_url(&url),
        Err(_) => "(not a URL)".to_string(),
    }
}

/// What a write tells the notifier. The dispatcher fills in the names.
#[derive(Debug, Clone)]
pub struct Notice {
    pub event: Event,
    pub kind: String,
    pub id: Option<String>,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    /// The text a `full` target may carry.
    pub summary: Option<String>,
    pub due_in_mins: Option<i64>,
}

impl Notice {
    /// A notice of something new waiting on the human. `summary` is the text a
    /// `full` target may carry; an enrolment request passes none, because its
    /// text comes from a caller that holds no token.
    pub fn ask(
        kind: &str,
        id: &str,
        project_id: Option<&str>,
        session_id: Option<&str>,
        summary: Option<&str>,
    ) -> Self {
        Self {
            event: Event::Ask,
            kind: kind.to_string(),
            id: Some(id.to_string()),
            project_id: project_id.map(str::to_string),
            session_id: session_id.map(str::to_string),
            summary: summary.map(str::to_string),
            due_in_mins: None,
        }
    }
}

/// One target as the notifier runs it.
#[derive(Clone)]
struct Target {
    id: String,
    name: String,
    preset: Preset,
    url: url::Url,
    token: Option<String>,
    content: Content,
    events: EventSet,
    enabled: bool,
    from_config: bool,
    interval: Duration,
}

impl Target {
    fn from_config(target: &NotifyTarget) -> Self {
        Self {
            id: CONFIG_ID.to_string(),
            name: "config.toml".to_string(),
            preset: Preset::Ntfy,
            url: target.url.clone(),
            token: target.token.clone(),
            content: Content::None,
            events: EventSet::ASK,
            enabled: true,
            from_config: true,
            interval: target.interval,
        }
    }

    fn from_stored(stored: &StoredTarget, interval: Duration) -> Option<Self> {
        let url = match stored.url.parse() {
            Ok(url) => url,
            Err(err) => {
                tracing::warn!(target_id = %stored.id, error = %err, "a stored notify target has no usable URL");
                return None;
            }
        };
        Some(Self {
            id: stored.id.clone(),
            name: stored.name.clone(),
            preset: stored.preset,
            url,
            token: stored.token.clone(),
            content: stored.content,
            events: stored.events,
            enabled: stored.enabled,
            from_config: false,
            interval,
        })
    }

    fn takes(&self, event: Event) -> bool {
        self.enabled && self.events.contains(event)
    }
}

/// The outcome of a target's sends since the hub started.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub last_attempt_at: Option<String>,
    pub last_delivered_at: Option<String>,
    /// Why the last send failed. Cleared by a delivery.
    pub last_error: Option<String>,
    /// Failed sends since the last delivery.
    pub failures: u32,
}

/// A target as the operator reads it: never the URL past its origin, never the
/// token.
#[derive(Debug, Clone, Serialize)]
pub struct TargetView {
    pub id: String,
    pub name: String,
    pub preset: Preset,
    pub url: String,
    pub token_set: bool,
    pub content: Content,
    pub events: Vec<&'static str>,
    pub enabled: bool,
    /// `config` for `notify_url`, which is read-only here, or `hub`.
    pub source: &'static str,
    pub status: Status,
}

struct Worker {
    target: Arc<Mutex<Target>>,
    queue: mpsc::Sender<Facts>,
}

/// The targets in the order they are listed, and their workers.
#[derive(Default)]
struct Registry {
    order: Vec<String>,
    workers: HashMap<String, Worker>,
}

struct Inner {
    dispatch: mpsc::Sender<Notice>,
    registry: Mutex<Registry>,
    status: Arc<Mutex<HashMap<String, Status>>>,
    client: HttpClient,
    delivery: Delivery,
    public_url: Option<String>,
    config: Option<NotifyTarget>,
    db: turso::Database,
}

/// The handle a write uses to hand the notifier a notice.
///
/// Cheap to clone. [`Notifier::off`] does nothing at all.
#[derive(Clone, Default)]
pub struct Notifier {
    inner: Option<Arc<Inner>>,
}

impl Notifier {
    /// A notifier that sends nothing and keeps no targets.
    pub fn off() -> Self {
        Self::default()
    }

    /// Start the dispatcher with the configured target and the stored ones.
    /// Must be called inside a Tokio runtime. The tasks end when the last
    /// handle is dropped.
    pub fn start(
        db: turso::Database,
        config: Option<NotifyTarget>,
        stored: &[StoredTarget],
        delivery: Delivery,
        public_url: Option<String>,
    ) -> Self {
        let (dispatch, pending) = mpsc::channel(DISPATCH_QUEUE);
        let inner = Arc::new(Inner {
            dispatch,
            registry: Mutex::new(Registry::default()),
            status: Arc::new(Mutex::new(HashMap::new())),
            client: client(),
            delivery,
            public_url,
            config,
            db: db.clone(),
        });
        let notifier = Self {
            inner: Some(inner.clone()),
        };
        notifier.reload(stored);
        tokio::spawn(dispatch_loop(Arc::downgrade(&inner), db, pending));
        notifier
    }

    /// Whether any target that is on takes `event`.
    pub fn takes(&self, event: Event) -> bool {
        self.inner.as_ref().is_some_and(|inner| {
            let registry = inner.registry.lock().unwrap();
            registry
                .workers
                .values()
                .any(|worker| worker.target.lock().unwrap().takes(event))
        })
    }

    /// Hand over a notice. Never blocks: a notice that finds the queue full is
    /// dropped and counted as a failed send.
    pub fn notice(&self, notice: Notice) {
        let Some(inner) = &self.inner else { return };
        if inner.dispatch.try_send(notice).is_err() {
            crate::metrics::record_notify(false);
        }
    }

    /// Replace the stored targets with `stored`. The configuration's target
    /// stays first. A worker for a target that is gone ends after its current
    /// send; a changed target's next send uses the change.
    pub fn reload(&self, stored: &[StoredTarget]) {
        let Some(inner) = &self.inner else { return };
        let mut wanted: Vec<Target> = Vec::new();
        if let Some(config) = &inner.config {
            wanted.push(Target::from_config(config));
        }
        wanted.extend(
            stored
                .iter()
                .filter_map(|stored| Target::from_stored(stored, inner.delivery.interval)),
        );
        let mut registry = inner.registry.lock().unwrap();
        let mut order = Vec::new();
        let mut workers = HashMap::new();
        for target in wanted {
            let id = target.id.clone();
            let worker = match registry.workers.remove(&id) {
                Some(worker) => {
                    *worker.target.lock().unwrap() = target;
                    worker
                }
                None => spawn_worker(inner, target),
            };
            order.push(id.clone());
            workers.insert(id, worker);
        }
        inner
            .status
            .lock()
            .unwrap()
            .retain(|id, _| workers.contains_key(id));
        *registry = Registry { order, workers };
    }

    /// Every target, the configuration's first, with the outcome of its sends.
    pub fn targets(&self) -> Vec<TargetView> {
        let Some(inner) = &self.inner else {
            return Vec::new();
        };
        let registry = inner.registry.lock().unwrap();
        let status = inner.status.lock().unwrap();
        registry
            .order
            .iter()
            .filter_map(|id| registry.workers.get(id))
            .map(|worker| {
                let target = worker.target.lock().unwrap();
                TargetView {
                    id: target.id.clone(),
                    name: target.name.clone(),
                    preset: target.preset,
                    url: redact_url(&target.url),
                    token_set: target.token.is_some(),
                    content: target.content,
                    events: target.events.names(),
                    enabled: target.enabled,
                    source: if target.from_config { "config" } else { "hub" },
                    status: status.get(&target.id).cloned().unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Send a test notice to one target now: one attempt, outside its quiet
    /// period. The outcome is recorded as any send's is. `None` when no
    /// target has that id.
    pub async fn test(&self, id: &str) -> Option<std::result::Result<(), String>> {
        let inner = self.inner.as_ref()?;
        let target = {
            let registry = inner.registry.lock().unwrap();
            registry.workers.get(id)?.target.lock().unwrap().clone()
        };
        let facts = Facts {
            event: Event::Test,
            kind: "test".to_string(),
            id: None,
            project: None,
            project_name: None,
            session: None,
            summary: None,
            due_in_mins: None,
            confidential: false,
            count: 1,
            at: crate::store::now_rfc3339(),
        };
        let txn = format!("test-{}", crate::store::next_id());
        let outcome = attempt(
            &inner.client,
            &target,
            &facts,
            inner.public_url.as_deref(),
            &txn,
        )
        .await
        .map_err(|failure| failure.reason);
        crate::metrics::record_notify(outcome.is_ok());
        record(&inner.status, &target, &outcome);
        Some(outcome)
    }
}

fn spawn_worker(inner: &Inner, target: Target) -> Worker {
    let (queue, pending) = mpsc::channel(TARGET_QUEUE);
    let target = Arc::new(Mutex::new(target));
    tokio::spawn(worker_loop(
        target.clone(),
        pending,
        Sender {
            client: inner.client.clone(),
            status: inner.status.clone(),
            backoff: inner.delivery.backoff.clone(),
            public_url: inner.public_url.clone(),
            db: inner.db.clone(),
        },
    ));
    Worker { target, queue }
}

async fn dispatch_loop(
    inner: std::sync::Weak<Inner>,
    db: turso::Database,
    mut pending: mpsc::Receiver<Notice>,
) {
    while let Some(notice) = pending.recv().await {
        let Some(inner) = inner.upgrade() else { return };
        let facts = resolve(&db, notice).await;
        let registry = inner.registry.lock().unwrap();
        for worker in registry.workers.values() {
            if worker.target.lock().unwrap().takes(facts.event) {
                // A full queue already holds a send that stands for this one.
                let _ = worker.queue.try_send(facts.clone());
            }
        }
    }
}

/// Fill in a notice's names. Confidentiality is always read, whatever the
/// targets' levels are now, because a level raised while the notice waits in a
/// queue must not widen what it says; a project that cannot be read counts as
/// confidential.
async fn resolve(db: &turso::Database, notice: Notice) -> Facts {
    let mut facts = Facts {
        event: notice.event,
        kind: notice.kind,
        id: notice.id,
        project: notice.project_id.clone(),
        project_name: None,
        session: None,
        summary: notice.summary,
        due_in_mins: notice.due_in_mins,
        confidential: notice.project_id.is_some(),
        count: 1,
        at: crate::store::now_rfc3339(),
    };
    if let Some(project_id) = &notice.project_id
        && let Ok(Some(project)) = crate::store::projects::get(db, project_id).await
    {
        facts.confidential = project.confidential;
        facts.project_name = Some(project.display_name);
    }
    if let Some(session_id) = &notice.session_id
        && let Ok(Some(session)) = crate::store::sessions::lookup_id(db, session_id).await
    {
        facts.session = Some(session.session_name);
    }
    facts
}

/// What a worker sends with.
struct Sender {
    client: HttpClient,
    status: Arc<Mutex<HashMap<String, Status>>>,
    backoff: Vec<Duration>,
    public_url: Option<String>,
    db: turso::Database,
}

async fn worker_loop(
    target: Arc<Mutex<Target>>,
    mut pending: mpsc::Receiver<Facts>,
    sender: Sender,
) {
    let mut recent: VecDeque<String> = VecDeque::with_capacity(RECENT_KEYS);
    while let Some(first) = pending.recv().await {
        // Everything already waiting is one send: the trailing send after a
        // quiet period, or a burst that landed together.
        let mut batch: Vec<Facts> = Vec::new();
        let mut next = Some(first);
        while let Some(facts) = next {
            let key = facts.key();
            let seen = recent.contains(&key) || batch.iter().any(|held| held.key() == key);
            if !seen {
                batch.push(facts);
            }
            next = pending.try_recv().ok();
        }
        if batch.is_empty() {
            continue;
        }
        for facts in &batch {
            if recent.len() == RECENT_KEYS {
                recent.pop_front();
            }
            recent.push_back(facts.key());
        }
        let count = batch.len();
        let mut facts = batch.swap_remove(0);
        facts.count = count;
        let txn = format!("{}-{}", facts.event.as_str(), crate::store::next_id());
        let current = target.lock().unwrap().clone();
        // A project made confidential since the notice was queued, or one that
        // cannot be read now, is sent as a ping.
        if current.content > Content::None
            && let Some(project) = &facts.project
        {
            let open = matches!(
                crate::store::projects::get(&sender.db, project).await,
                Ok(Some(found)) if !found.confidential
            );
            facts.confidential |= !open;
        }
        let outcome = deliver(
            &sender.client,
            &current,
            &facts,
            sender.public_url.as_deref(),
            &txn,
            &sender.backoff,
        )
        .await;
        crate::metrics::record_notify(outcome.is_ok());
        if let Err(reason) = &outcome {
            tracing::warn!(target = %redact_url(&current.url), target_id = %current.id, reason, "notify send failed");
        }
        record(&sender.status, &current, &outcome);
        tokio::time::sleep(current.interval).await;
    }
}

fn record(
    status: &Mutex<HashMap<String, Status>>,
    target: &Target,
    outcome: &std::result::Result<(), String>,
) {
    let now = crate::store::now_rfc3339();
    let mut status = status.lock().unwrap();
    let entry = status.entry(target.id.clone()).or_default();
    entry.last_attempt_at = Some(now.clone());
    match outcome {
        Ok(()) => {
            entry.last_delivered_at = Some(now);
            entry.last_error = None;
            entry.failures = 0;
        }
        Err(reason) => {
            entry.last_error = Some(reason.clone());
            entry.failures = entry.failures.saturating_add(1);
        }
    }
}

/// One send, retried after each wait in `backoff` while the failure is one a
/// retry may fix.
async fn deliver(
    client: &HttpClient,
    target: &Target,
    facts: &Facts,
    public_url: Option<&str>,
    txn: &str,
    backoff: &[Duration],
) -> std::result::Result<(), String> {
    let mut waits = backoff.iter();
    loop {
        match attempt(client, target, facts, public_url, txn).await {
            Ok(()) => return Ok(()),
            Err(failure) => match waits.next() {
                Some(wait) if failure.retry => tokio::time::sleep(*wait).await,
                _ => return Err(failure.reason),
            },
        }
    }
}

struct Failure {
    reason: String,
    retry: bool,
}

async fn attempt(
    client: &HttpClient,
    target: &Target,
    facts: &Facts,
    public_url: Option<&str>,
    txn: &str,
) -> std::result::Result<(), Failure> {
    let fixed = |reason: String| Failure {
        reason,
        retry: false,
    };
    let outbound = preset::render(
        target.preset,
        &target.url,
        target.token.as_deref(),
        target.content,
        facts,
        public_url,
        txn,
    )
    .map_err(fixed)?;
    let mut request = hyper::Request::builder()
        .method(outbound.method)
        .uri(outbound.url.as_str());
    for (name, value) in outbound.headers {
        request = request.header(name, value);
    }
    let request = request
        .body(Full::new(Bytes::from(outbound.body)))
        .map_err(|err| fixed(format!("could not build the request: {err}")))?;
    match tokio::time::timeout(SEND_TIMEOUT, client.request(request)).await {
        Err(_) => Err(Failure {
            reason: format!("timed out after {}s", SEND_TIMEOUT.as_secs()),
            retry: true,
        }),
        Ok(Err(err)) => Err(Failure {
            reason: chain(&err),
            retry: true,
        }),
        Ok(Ok(response)) if response.status().is_success() => Ok(()),
        Ok(Ok(response)) => {
            let status = response.status();
            Err(Failure {
                reason: format!("the target answered {status}"),
                retry: status.is_server_error() || status.as_u16() == 429,
            })
        }
    }
}

type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

fn client() -> HttpClient {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = hyper_rustls::HttpsConnectorBuilder::new();
    let builder = match builder.with_provider_and_native_roots(provider.clone()) {
        Ok(builder) => builder,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "no system certificate roots were loaded; https notify sends will fail"
            );
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("the default provider supports the default protocol versions")
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth();
            hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(config)
        }
    };
    let connector = builder.https_or_http().enable_http1().build();
    Client::builder(TokioExecutor::new()).build(connector)
}

/// An error and its causes on one line. The client's own message is generic,
/// and the cause (refused, unresolved, a certificate) is what the operator
/// needs. None of these carry the request or its headers.
fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redacted_url_keeps_only_the_origin() {
        let url: url::Url = "https://ntfy.lan:8443/secret-topic?auth=abc"
            .parse()
            .unwrap();
        assert_eq!(redact_url(&url), "https://ntfy.lan:8443/...");
        let bare: url::Url = "http://hook.lan".parse().unwrap();
        assert_eq!(redact_url(&bare), "http://hook.lan");
    }

    #[test]
    fn the_debug_view_hides_the_token_and_the_topic() {
        let target = NotifyTarget::parse(
            Some("http://ntfy.lan/topic-secret"),
            Some("tk_secret"),
            None,
        )
        .unwrap()
        .unwrap();
        let shown = format!("{target:?}");
        assert!(!shown.contains("tk_secret"), "{shown}");
        assert!(!shown.contains("topic-secret"), "{shown}");
    }

    #[test]
    fn an_event_set_round_trips_and_refuses_an_unknown_name() {
        let set = EventSet::parse(&["ask".to_string(), "ended".to_string()]).unwrap();
        assert_eq!(set.to_stored(), "ask,ended");
        assert_eq!(EventSet::from_stored("ask,ended"), set);
        assert!(set.contains(Event::Test));
        assert!(!set.contains(Event::Deadline));
        assert!(EventSet::parse(&["failed".to_string()]).is_err());
    }

    #[test]
    fn a_target_url_with_credentials_is_refused() {
        assert!(check_target(Preset::Webhook, "https://me:pw@hook.lan/x", None).is_err());
        assert!(check_target(Preset::Webhook, "ftp://hook.lan/x", None).is_err());
        assert!(check_target(Preset::Webhook, "https://hook.lan/x", Some("tk")).is_ok());
    }
}
