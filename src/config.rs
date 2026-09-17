//! Process configuration, read from the environment.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::{Error, Result};

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
    /// Optional admin token for the control surface.
    pub admin_token: Option<String>,
    /// Posture applied to a newly created agent.
    pub trust_default: TrustDefault,
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

        Ok(Self {
            data_dir,
            bind,
            admin_token,
            trust_default,
        })
    }

    /// Read the optional embedded tailnet endpoint configuration.
    ///
    /// Kept out of [`Config`] so the common configuration stays small and a
    /// build without the feature does not carry it.
    pub fn tailnet_from_env() -> Result<Tailnet> {
        let tailnet = Tailnet {
            auth_key: std::env::var("HUB_TAILNET")
                .ok()
                .filter(|key| !key.is_empty()),
            port: std::env::var("HUB_TAILNET_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8080),
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

    /// Directory holding artifact blobs.
    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }
}
