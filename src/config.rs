//! Process configuration, read from the environment.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::limits::InboxCaps;

/// The default trust posture a new agent is created with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDefault {
    Trusted,
    Untrusted,
}

/// Everything the process needs to start.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory holding the hub store, session files, and artifact blobs.
    pub data_dir: PathBuf,
    /// Address the HTTP API and PWA bind to.
    pub bind: SocketAddr,
    /// External origin the hub is reached at, when the operator set one.
    /// Overrides every request-derived origin in a served document.
    pub public_url: Option<String>,
    /// Optional admin token for the control surface.
    pub admin_token: Option<String>,
    /// Posture applied to a newly created agent.
    pub trust_default: TrustDefault,
    /// Ceilings on the open action items an agent may leave on the human.
    pub inbox_caps: InboxCaps,
}

/// The embedded tailnet endpoint configuration.
///
/// The endpoint is addressed by tailnet IP, since the library has no tailnet
/// name resolution, and is off unless `HUB_TAILNET` is set.
#[derive(Debug, Clone, Default)]
pub struct Tailnet {
    /// A Tailscale auth key. Present when the endpoint is enabled.
    pub auth_key: Option<String>,
    /// Port to serve on the tailnet IP.
    pub port: u16,
    /// Directory holding the device key state, under the data directory.
    pub state_dir: PathBuf,
    /// Optional control server URL, for a self-hosted control plane. The
    /// library's public control plane is used when this is unset.
    pub control_url: Option<url::Url>,
}

impl Tailnet {
    /// Whether the embedded endpoint is configured.
    pub fn enabled(&self) -> bool {
        self.auth_key.is_some()
    }
}

impl Config {
    /// Read configuration from the environment, falling back to safe defaults.
    pub fn from_env() -> Result<Self> {
        let data_dir = std::env::var("HUB_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));

        let bind: SocketAddr = std::env::var("HUB_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse()
            .map_err(|err| Error::Config(format!("HUB_BIND is not a socket address: {err}")))?;

        // Validated here, not at the first request, so a typo fails startup
        // instead of shipping a wrong origin into a policy and a served skill.
        let public_url = match std::env::var("HUB_PUBLIC_URL") {
            Ok(value) if !value.trim().is_empty() => Some(Self::parse_public_url(value.trim())?),
            _ => None,
        };

        let admin_token = std::env::var("HUB_ADMIN_TOKEN")
            .ok()
            .filter(|token| !token.is_empty());

        // The control surface fails closed without a token, so an exposed bind
        // would be dead. Require the token off loopback; only warn locally.
        if admin_token.is_none() {
            if !bind.ip().is_loopback() {
                return Err(Error::Config(
                    "HUB_ADMIN_TOKEN is required when HUB_BIND is not loopback".to_string(),
                ));
            }
            tracing::warn!(
                "HUB_ADMIN_TOKEN is unset; the control surface will reject every request"
            );
        }

        let trust_default = match std::env::var("HUB_TRUST_DEFAULT").as_deref() {
            Ok("untrusted") => TrustDefault::Untrusted,
            Ok("trusted") | Err(_) => TrustDefault::Trusted,
            Ok(other) => {
                return Err(Error::Config(format!(
                    "HUB_TRUST_DEFAULT must be 'trusted' or 'untrusted', got '{other}'"
                )));
            }
        };

        let inbox_caps = InboxCaps::parse(
            std::env::var("HUB_INBOX_ACTION_PER_AGENT").ok().as_deref(),
            std::env::var("HUB_INBOX_ACTION_PER_PROJECT")
                .ok()
                .as_deref(),
        )?;

        Ok(Self {
            data_dir,
            bind,
            public_url,
            admin_token,
            trust_default,
            inbox_caps,
        })
    }

    /// Normalise the operator's external origin.
    ///
    /// The value is spliced into a content policy and into served documents,
    /// so it has to be a bare `http` or `https` origin: a host, an optional
    /// port, and nothing else. The parsed origin drops a default port and a
    /// trailing slash, so every call site names the same string.
    pub fn parse_public_url(value: &str) -> Result<String> {
        let url: url::Url = value
            .parse()
            .map_err(|err| Error::Config(format!("HUB_PUBLIC_URL is not a URL: {err}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Config(format!(
                "HUB_PUBLIC_URL must be http or https, got '{}'",
                url.scheme()
            )));
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(Error::Config(
                "HUB_PUBLIC_URL needs a host, such as https://hub.example".to_string(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Error::Config(
                "HUB_PUBLIC_URL must not carry credentials".to_string(),
            ));
        }
        if !url.path().trim_end_matches('/').is_empty()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Config(format!(
                "HUB_PUBLIC_URL must be an origin with no path, got '{value}'"
            )));
        }
        let origin = url.origin().ascii_serialization();
        // The origin is written into a content policy and into markup exactly
        // as a request host is, so it is held to the same characters. A stray
        // `;` or quote would otherwise break the frame policy silently.
        let authority = origin.split_once("://").map_or("", |(_, rest)| rest);
        if !crate::http::origin::is_safe_host(authority) {
            return Err(Error::Config(format!(
                "HUB_PUBLIC_URL has characters a host cannot carry, got '{value}'"
            )));
        }
        Ok(origin)
    }

    /// Whether a tailnet auth key is configured, so the process should opt into
    /// the embedded tailnet's experimental guard before starting.
    pub fn tailnet_requested(&self) -> bool {
        std::env::var("HUB_TAILNET").is_ok_and(|key| !key.is_empty())
    }

    /// Read the optional embedded tailnet endpoint configuration.
    ///
    /// Kept out of [`Config`] so the common configuration stays small and a
    /// build without the feature does not carry it.
    pub fn tailnet_from_env(&self) -> Result<Tailnet> {
        let port = match std::env::var("HUB_TAILNET_PORT") {
            Ok(value) => value
                .parse()
                .map_err(|_| Error::Config(format!("HUB_TAILNET_PORT is not a port: {value}")))?,
            Err(_) => 8080,
        };
        // Validated here, not in the spawned endpoint task, so a typo fails
        // startup instead of leaving a dead endpoint behind a healthy hub.
        let control_url = match std::env::var("HUB_TAILNET_CONTROL_URL") {
            Ok(value) if !value.is_empty() => Some(value.parse().map_err(|err| {
                Error::Config(format!("HUB_TAILNET_CONTROL_URL is not a URL: {err}"))
            })?),
            _ => None,
        };
        let tailnet = Tailnet {
            auth_key: std::env::var("HUB_TAILNET")
                .ok()
                .filter(|key| !key.is_empty()),
            port,
            state_dir: self.data_dir.join("tailnet"),
            control_url,
        };
        if tailnet.enabled() && !cfg!(feature = "tailnet") {
            return Err(Error::Config(
                "HUB_TAILNET is set but the binary was built without the tailnet feature"
                    .to_string(),
            ));
        }
        Ok(tailnet)
    }

    /// Path to the hub store.
    pub fn hub_db_path(&self) -> PathBuf {
        self.data_dir.join("hub.db")
    }

    /// Directory holding one brain file per session.
    pub fn sessions_dir(&self) -> PathBuf {
        self.data_dir.join("sessions")
    }

    /// Directory holding one knowledge base file per project.
    ///
    /// A sibling of the sessions directory, not a file inside it, so a session
    /// prune cannot reach a knowledge base by construction.
    pub fn knowledge_dir(&self) -> PathBuf {
        crate::brain::knowledge_dir(&self.data_dir)
    }

    /// Directory holding artifact blobs.
    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }
}
