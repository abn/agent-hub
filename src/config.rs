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
