//! The contentless nudge to an operator-configured target.
//!
//! When something starts waiting on the human, the hub can POST one fixed
//! sentence to a URL the operator runs, such as a self-hosted ntfy topic or a
//! webhook on the LAN. The body names no project, agent, title, id or count, so
//! the target learns only that the hub wants a look. Sends are coalesced: at
//! most one per interval, and a trigger that lands inside the quiet period
//! produces one trailing send when it ends. Delivery runs on its own task, so a
//! slow or unreachable target never holds up the write that woke it.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::mpsc;

use crate::error::{Error, Result};

/// The one body every send carries.
pub const BODY: &str = "Something is waiting for you in Agent Hub.";

/// The default quiet period between two sends.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// The longest quiet period the setting accepts.
const INTERVAL_SECS_MAX: u64 = 24 * 60 * 60;

/// How long one send may take, connect to response head.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the nudge goes and how often it may go.
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
        let url = parse_url(url)?;
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

fn parse_url(value: &str) -> Result<url::Url> {
    let url: url::Url = value
        .parse()
        .map_err(|err| Error::Config(format!("HUB_NOTIFY_URL is not a URL: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Config(format!(
            "HUB_NOTIFY_URL must be http or https, got '{}'",
            url.scheme()
        )));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(Error::Config(
            "HUB_NOTIFY_URL needs a host, such as http://ntfy.lan/agent-hub".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Config(
            "HUB_NOTIFY_URL must not carry credentials; set HUB_NOTIFY_TOKEN instead".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(Error::Config(
            "HUB_NOTIFY_URL must not carry a fragment".to_string(),
        ));
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

/// The handle a write uses to say something is waiting on the human.
///
/// Cheap to clone. When no target is configured it does nothing.
#[derive(Clone, Default)]
pub struct Notifier {
    wake: Option<mpsc::Sender<()>>,
}

impl Notifier {
    /// A notifier with no target.
    pub fn off() -> Self {
        Self::default()
    }

    /// Start the delivery task for a target. Must be called inside a Tokio
    /// runtime. The task ends when the last handle is dropped.
    pub fn start(target: NotifyTarget) -> Self {
        // One slot is the coalescing: a trigger that finds it full is already
        // represented by the send it is waiting for.
        let (wake, pending) = mpsc::channel(1);
        let client = client(&target);
        tokio::spawn(run(Arc::new(target), client, pending));
        Self { wake: Some(wake) }
    }

    /// Whether a target is configured.
    pub fn enabled(&self) -> bool {
        self.wake.is_some()
    }

    /// Record that something now waits on the human. Never blocks.
    pub fn waiting(&self) {
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        }
    }
}

type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

fn client(target: &NotifyTarget) -> HttpClient {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = hyper_rustls::HttpsConnectorBuilder::new();
    let builder = match builder.with_provider_and_native_roots(provider.clone()) {
        Ok(builder) => builder,
        Err(err) => {
            if target.url.scheme() == "https" {
                tracing::warn!(
                    target = %target.redacted(),
                    error = %err,
                    "no system certificate roots were loaded; https notify sends will fail"
                );
            }
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

async fn run(target: Arc<NotifyTarget>, client: HttpClient, mut pending: mpsc::Receiver<()>) {
    while pending.recv().await.is_some() {
        match send(&client, &target).await {
            Ok(()) => crate::metrics::record_notify(true),
            Err(reason) => {
                crate::metrics::record_notify(false);
                tracing::warn!(target = %target.redacted(), reason, "notify send failed");
            }
        }
        tokio::time::sleep(target.interval).await;
    }
}

async fn send(client: &HttpClient, target: &NotifyTarget) -> std::result::Result<(), String> {
    let mut request = hyper::Request::post(target.url.as_str()).header(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if let Some(token) = &target.token {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "the token is not a header value".to_string())?;
        value.set_sensitive(true);
        request = request.header(AUTHORIZATION, value);
    }
    let request = request
        .body(Full::new(Bytes::from_static(BODY.as_bytes())))
        .map_err(|err| format!("could not build the request: {err}"))?;
    match tokio::time::timeout(SEND_TIMEOUT, client.request(request)).await {
        Err(_) => Err(format!("timed out after {}s", SEND_TIMEOUT.as_secs())),
        Ok(Err(err)) => Err(chain(&err)),
        Ok(Ok(response)) if response.status().is_success() => Ok(()),
        Ok(Ok(response)) => Err(format!("the target answered {}", response.status())),
    }
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
}
