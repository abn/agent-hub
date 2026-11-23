//! The hub as a hook sees it: settings resolved for the harness, the peek, the
//! project list, and one MCP connection that collects the notification
//! trailers of every call it makes.

use std::time::{Duration, Instant};

use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};

use super::input::Harness;
use super::render::Item;
use crate::client::credential::{self, ConfigView, Env, ResolveInput, State};
use crate::client::{Failure, call_on, connect, projects};
use crate::config::{self, ClientConfig, ParsedConfigFile};

/// The budget of one hub call on a hook path.
pub const HOOK_BUDGET: Duration = Duration::from_secs(5);
/// The budget of one hub call while a session starts.
pub const STARTUP_BUDGET: Duration = Duration::from_secs(10);
/// How long past its own wait a held peek is given to answer.
const PEEK_SLACK: Duration = Duration::from_secs(10);
/// How long closing a connection may take.
pub const CLOSE_BUDGET: Duration = Duration::from_secs(2);

/// The connection settings of one hook run, as the credential resolver chose
/// them.
#[derive(Debug, Clone)]
pub struct Settings {
    pub url: String,
    pub token: Option<String>,
    /// The member the resolver selected, when it selected one.
    pub member: Option<String>,
    /// Sent as `X-Agent-Id`: the canonical name of the harness, so the hooks
    /// and the harness's own MCP connection are one agent.
    pub agent: String,
    /// What the hook does next: connect, connect without a token, or say why
    /// not.
    pub state: State,
    /// Context lines from the resolver, such as a project file naming another
    /// hub than the plugin's.
    pub notices: Vec<String>,
}

impl Settings {
    /// The line the session block carries instead of any hub work, for a
    /// resolver state that rules out connecting. `None` when the hook may
    /// connect.
    pub fn refusal(&self) -> Option<String> {
        match self.state {
            State::SeveralMembers => Some(format!(
                "Several members for {}; set member in .agent-hub.toml or HUB_MEMBER.",
                self.url
            )),
            State::NeedsTrust => Some(format!(
                "This project asks to use member {} on {}. Allow it with agent-hub trust or under [trust] in config.toml.",
                self.member.as_deref().unwrap_or("(none)"),
                self.url
            )),
            State::Tokenless => Some(NOT_A_MEMBER.to_string()),
            State::Ok | State::NoHub => None,
        }
    }
}

/// The line for a caller the hub does not know yet.
pub const NOT_A_MEMBER: &str = "Not a member of this hub yet: call enrol.";
/// The line for a hub that does not answer.
pub const OFFLINE: &str = "Hub unreachable; working without it.";

fn present(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The config files a client reads, each one its own source: the file named by
/// `HUB_CONFIG` alone, or else the user's file and then the system's. A file that
/// cannot be read or parsed is left out and reported, so one broken file does
/// not stop a configuration that is complete without it. A path is read only
/// when it is a regular file, opened without waiting, so a FIFO in its place
/// cannot hold the hook.
fn config_files(env: &dyn Fn(&str) -> Option<String>) -> (Vec<ParsedConfigFile>, Vec<String>) {
    let (named, system, user_candidates) = config::resolve_paths(env);
    let paths = match named {
        Some(path) => vec![path],
        None => user_candidates
            .into_iter()
            .find(|path| path.is_file())
            .into_iter()
            .chain(system.is_file().then_some(system))
            .collect(),
    };
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        if std::fs::metadata(&path).is_ok() && super::resolve::open_regular(&path).is_none() {
            errors.push(format!("{} is not a regular file", path.display()));
            continue;
        }
        match config::load_config_file(&path) {
            Ok(Some(file)) => files.push(file),
            Ok(None) => {}
            Err(err) => errors.push(err.to_string()),
        }
    }
    (files, errors)
}

/// Resolve the settings through the one credential resolver, over the
/// environment, the project file at the git top level above `cwd` and each
/// `config.toml`. For Claude the plugin's `hub_url` option pins the URL, as it
/// does for the plugin's own MCP entry, so a hook and the connection it sits
/// beside reach one hub. The agent is the harness's canonical name.
pub fn resolve_settings(
    harness: Harness,
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &str,
) -> Result<Settings, String> {
    let get = |key: &str| present(env(key));
    let (files, errors) = config_files(env);

    let mut view = ConfigView::default();
    for file in &files {
        view.client.url = view.client.url.or_else(|| file.view.client.url.clone());
        view.client.member = view
            .client
            .member
            .or_else(|| file.view.client.member.clone());
        view.credentials
            .extend(file.view.credentials.iter().cloned());
        view.trust
            .pairings
            .extend(file.view.trust.pairings.iter().cloned());
    }

    let root = (!cwd.is_empty())
        .then(|| credential::project_root(std::path::Path::new(cwd)))
        .flatten();
    let (project_file, mut notices) = match root.as_deref() {
        Some(root) => credential::read_project_file(root).map_err(|err| err.to_string())?,
        None => (None, Vec::new()),
    };
    let pinned_url = (harness == Harness::Claude)
        .then(|| get("CLAUDE_PLUGIN_OPTION_HUB_URL"))
        .flatten();

    let credentials = view.credentials.clone();
    let resolved = credential::resolve(&ResolveInput {
        env: Env {
            hub_url: get("HUB_URL"),
            hub_token: get("HUB_TOKEN"),
            hub_member: get("HUB_MEMBER"),
            hub_config: get("HUB_CONFIG"),
        },
        pinned_url,
        project_file,
        project_root: root.map(|root| root.to_string_lossy().into_owned()),
        config: view,
    });
    notices.extend(resolved.notices);
    if harness == Harness::Claude
        && let Some(url) = resolved.url.as_deref()
        && resolved.state == State::Ok
        && let Some(line) = super::helper::mismatch(&credentials, url, resolved.member.as_deref())
    {
        notices.push(line);
    }

    let Some(url) = resolved.url else {
        let broken = errors
            .first()
            .map(|err| format!(" ({err})"))
            .unwrap_or_default();
        return Err(format!(
            "no hub URL configured; set HUB_URL or write [client] url to config.toml{broken}"
        ));
    };
    Ok(Settings {
        url: url.trim_end_matches('/').to_string(),
        token: resolved.token,
        member: resolved.member,
        agent: harness.agent().to_string(),
        state: resolved.state,
        notices,
    })
}

/// What a peek found.
#[derive(Debug, Clone, Default)]
pub struct Peek {
    pub count: usize,
    pub items: Vec<Item>,
    /// What to send as `after` on the next peek.
    pub cursor: Option<String>,
}

/// A hub the hook reaches. With a deadline, no call outlasts it.
#[derive(Debug, Clone)]
pub struct Hub {
    settings: Settings,
    deadline: Option<Instant>,
}

fn out_of_time() -> Failure {
    Failure::Unavailable("the hook ran out of time".to_string())
}

/// `budget`, cut to what is left before `deadline`.
fn within(deadline: Option<Instant>, budget: Duration) -> Result<Duration, Failure> {
    let Some(deadline) = deadline else {
        return Ok(budget);
    };
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(out_of_time());
    }
    Ok(budget.min(left))
}

impl Hub {
    pub fn new(settings: Settings, deadline: Option<Instant>) -> Self {
        Self { settings, deadline }
    }

    fn within(&self, budget: Duration) -> Result<Duration, Failure> {
        within(self.deadline, budget)
    }

    /// The settings this hub was opened with.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn config(&self, budget: Duration) -> ClientConfig {
        ClientConfig {
            url: Some(self.settings.url.clone()),
            token: self.settings.token.clone(),
            member: self.settings.member.clone(),
            agent_id: Some(self.settings.agent.clone()),
            project: None,
            timeout: budget,
            state: self.settings.state,
            ..ClientConfig::default()
        }
    }

    /// Read what is waiting, without delivering any of it. With `wait` the
    /// request is held until something newer than `after` is waiting or the
    /// time is up.
    pub async fn peek(
        &self,
        session: Option<&str>,
        wait: u64,
        after: Option<&str>,
    ) -> Result<Peek, Failure> {
        let budget = self.within(if wait == 0 {
            HOOK_BUDGET
        } else {
            Duration::from_secs(wait) + PEEK_SLACK
        })?;
        let config = self.config(budget);
        let mut url = url::Url::parse(&format!("{}/api/v1/me/pending", self.settings.url))
            .map_err(|err| Failure::Config(format!("the hub URL is not a URL: {err}")))?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(session) = session {
                query.append_pair("session", session);
            }
            query.append_pair("wait", &wait.to_string());
            if let Some(after) = after {
                query.append_pair("after", after);
            }
        }
        let agent = self.settings.agent.clone();
        let (_, text) = projects::call(&config, url.as_str(), move |url, client| {
            client
                .get(url)
                .header(crate::principal::AGENT_ID_HEADER, &agent)
        })
        .await?;
        let body = projects::json_body(&text)?;
        let items = Item::list_from_wire(body.get("items"));
        Ok(Peek {
            count: body
                .get("count")
                .and_then(Value::as_u64)
                .map_or(items.len(), |count| count as usize),
            items,
            cursor: body
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string),
        })
    }

    /// The ids of the projects the caller can see.
    pub async fn projects(&self) -> Result<Vec<String>, Failure> {
        let budget = self.within(STARTUP_BUDGET)?;
        let listed = projects::list(&self.config(budget)).await?;
        Ok(listed
            .get("projects")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Open one MCP connection, whose calls each have `budget`.
    pub async fn open(&self, budget: Duration) -> Result<Conn, Failure> {
        let budget = self.within(budget)?;
        let config = self.config(budget);
        let service = tokio::time::timeout(budget, connect(&config))
            .await
            .map_err(|_| {
                Failure::Unavailable(format!(
                    "the hub did not answer within {} seconds",
                    budget.as_secs()
                ))
            })??;
        Ok(Conn {
            service,
            config,
            deadline: self.deadline,
            trailers: Vec::new(),
        })
    }
}

/// One MCP connection and the items its results reported as pending.
pub struct Conn {
    service: RunningService<RoleClient, crate::client::CliClient>,
    config: ClientConfig,
    deadline: Option<Instant>,
    trailers: Vec<Item>,
}

impl Conn {
    async fn raw(&self, tool: &str, arguments: Value) -> Result<Value, Failure> {
        let Value::Object(arguments) = arguments else {
            return Err(Failure::Usage(
                "tool arguments must be a JSON object".to_string(),
            ));
        };
        let budget = within(self.deadline, self.config.timeout)?;
        tokio::time::timeout(
            budget,
            call_on(&self.service, &self.config, tool, arguments),
        )
        .await
        .map_err(|_| out_of_time())?
    }

    /// Call one tool. A trailer on the result is kept for [`Conn::take_trailers`].
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, Failure> {
        let value = self.raw(tool, arguments).await?;
        self.trailers.extend(Item::list_from_wire(
            value.pointer("/notifications/pending"),
        ));
        Ok(value)
    }

    /// Deliver what is pending for a session, once, and return it.
    pub async fn drain(&self, session_id: &str) -> Result<Vec<Item>, Failure> {
        let value = self
            .raw(
                "notify_wait",
                json!({"wait_seconds": 0, "session": {"session_id": session_id}}),
            )
            .await?;
        Ok(Item::list_from_wire(
            value.pointer("/notifications/pending"),
        ))
    }

    /// The items the results so far reported, once each, oldest first.
    pub fn take_trailers(&mut self) -> Vec<Item> {
        dedupe(std::mem::take(&mut self.trailers))
    }

    /// Close the connection, without waiting long on a hub that is gone.
    pub async fn close(self) {
        let budget = within(self.deadline, CLOSE_BUDGET).unwrap_or(Duration::ZERO);
        tokio::time::timeout(budget, self.service.cancel())
            .await
            .ok();
    }
}

/// The items with a repeated id removed, first occurrence kept.
pub fn dedupe(items: Vec<Item>) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .collect()
}

/// The `details.reason` the hub gave a failure, when it gave one.
pub fn reason(failure: &Failure) -> Option<&str> {
    match failure {
        Failure::Tool(error) | Failure::Denied(error) => error
            .pointer("/error/details/reason")
            .and_then(Value::as_str),
        _ => None,
    }
}
