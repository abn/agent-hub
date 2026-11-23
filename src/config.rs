//! Process and client configuration, read from config files and the environment.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::limits::{EventCeiling, InboxCaps};

/// Keys the client table carries.
pub const CLIENT_KEYS: &[&str] = &["url", "member", "timeout", "project"];

/// Keys an older release wrote under `[client]`. `token` is a legacy
/// credential and keeps working as one with no member; `agent_id` is read,
/// warned about and ignored.
const LEGACY_CLIENT_KEYS: &[&str] = &["token", "agent_id"];

/// The warning for a `[client] agent_id` line.
const AGENT_ID_WARNING: &str =
    "agent_id no longer names the member; the agent comes from the client";

/// Keys the hub table carries.
pub const HUB_KEYS: &[&str] = &[
    "bind",
    "data_dir",
    "admin_token",
    "node_name",
    "public_url",
    "active_window_secs",
    "inbox_action_per_agent",
    "inbox_action_per_project",
    "events_per_project",
    "enrol",
    "enrol_pending_max",
    "enrol_pending_ttl_secs",
    "integrity_sample_secs",
    "trust_proxy",
    "tailnet",
    "tailnet_port",
    "tailnet_control_url",
    "notify_url",
    "notify_token",
    "notify_interval_secs",
    "backup_dir",
    "log_file",
];

/// The settings that name the hub a client reaches.
///
/// `url`, `member` and `token` are what the credential resolver chose from the
/// environment, the project file and `config.toml`; nothing else reads a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// Base URL of a running hub, such as `http://hub.lan:8080`.
    pub url: Option<String>,
    /// Bearer token the hub resolves to a member.
    pub token: Option<String>,
    /// The member the token belongs to, when the resolver knows it.
    pub member: Option<String>,
    /// The agent slug `HUB_AGENT_ID` sets explicitly. The agent is otherwise
    /// the client's own name, which the hub reads from the handshake.
    pub agent_id: Option<String>,
    /// The project the knowledge-base shorthands act on, when no flag names one.
    pub project: Option<String>,
    /// How long one call may take, handshake to answer.
    pub timeout: std::time::Duration,
    /// Every `[[credential]]` of the config files, and a legacy `[client]
    /// token` as one with no member.
    #[cfg(feature = "client")]
    pub credentials: Vec<crate::client::credential::Credential>,
    /// The `[trust]` table of the config files.
    #[cfg(feature = "client")]
    pub trust: crate::client::credential::Trust,
    /// What the resolver decided: connect, connect without a token, or why not.
    #[cfg(feature = "client")]
    pub state: crate::client::credential::State,
    /// Where the URL, member and token came from.
    #[cfg(feature = "client")]
    pub sources: crate::client::credential::Sources,
    /// Context lines for the human, such as a project file naming another hub.
    #[cfg(feature = "client")]
    pub notices: Vec<String>,
}

/// The default for `HUB_TIMEOUT`.
const CLIENT_TIMEOUT_SECS: f64 = 120.0;

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            url: None,
            token: None,
            member: None,
            agent_id: None,
            project: None,
            timeout: std::time::Duration::from_secs_f64(CLIENT_TIMEOUT_SECS),
            #[cfg(feature = "client")]
            credentials: Vec::new(),
            #[cfg(feature = "client")]
            trust: crate::client::credential::Trust::default(),
            #[cfg(feature = "client")]
            state: crate::client::credential::State::NoHub,
            #[cfg(feature = "client")]
            sources: crate::client::credential::Sources {
                url: crate::client::credential::UrlSource::None,
                member: crate::client::credential::MemberSource::None,
                token: crate::client::credential::TokenSource::None,
            },
            #[cfg(feature = "client")]
            notices: Vec::new(),
        }
    }
}

impl ClientConfig {
    /// Read the settings from the process environment and config files.
    pub fn from_env() -> Result<Self> {
        Self::resolve(&|key| std::env::var(key).ok())
    }

    /// Resolve the settings from an environment lookup and layered config files.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        Self::resolve_pinned(env, None)
    }

    /// [`ClientConfig::resolve`] with a URL a flag pins, which beats every
    /// other source.
    pub fn resolve_pinned(
        env: &dyn Fn(&str) -> Option<String>,
        pinned_url: Option<&str>,
    ) -> Result<Self> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();

        let project = Setting::resolved(env, "client", "project", &files, None).value;
        let agent_id = present(env("HUB_AGENT_ID"));

        let timeout_str = Setting::resolved(env, "client", "timeout", &files, Some("120")).value;
        let timeout = match timeout_str {
            None => CLIENT_TIMEOUT_SECS,
            Some(value) => match value.parse::<f64>() {
                Ok(seconds) if seconds.is_finite() && seconds > 0.0 => seconds,
                _ => {
                    return Err(Error::Config(format!(
                        "HUB_TIMEOUT must be a number of seconds above zero, got '{value}'"
                    )));
                }
            },
        };

        let mut config = Self {
            project,
            agent_id,
            timeout: std::time::Duration::from_secs_f64(timeout),
            ..Self::default()
        };
        #[cfg(feature = "client")]
        config.apply_credentials(env, &files, pinned_url)?;
        #[cfg(not(feature = "client"))]
        {
            let _ = pinned_url;
            config.url = Setting::resolved(env, "client", "url", &files, None).value;
            config.token = Setting::resolved(env, "client", "token", &files, None).value;
            config.member = Setting::resolved(env, "client", "member", &files, None).value;
        }
        Ok(config)
    }

    /// Run the credential resolver over the environment, the project file at
    /// the git top level and the config files.
    #[cfg(feature = "client")]
    fn apply_credentials(
        &mut self,
        env: &dyn Fn(&str) -> Option<String>,
        files: &[&ParsedConfigFile],
        pinned_url: Option<&str>,
    ) -> Result<()> {
        use crate::client::credential::{self, ConfigView, Env, ResolveInput};

        let mut view = ConfigView::default();
        for file in files {
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

        let start = std::env::current_dir().ok();
        let root = start.as_deref().and_then(credential::project_root);
        let (project_file, mut notices) = match root.as_deref() {
            Some(root) => credential::read_project_file(root)?,
            None => (None, Vec::new()),
        };

        let input = ResolveInput {
            env: Env {
                hub_url: present(env("HUB_URL")),
                hub_token: present(env("HUB_TOKEN")),
                hub_member: present(env("HUB_MEMBER")),
                hub_config: present(env("HUB_CONFIG")),
            },
            pinned_url: pinned_url.map(str::to_string),
            project_file,
            project_root: root.map(|root| root.to_string_lossy().into_owned()),
            config: view.clone(),
        };
        let resolved = credential::resolve(&input);
        notices.extend(resolved.notices);
        self.url = resolved.url;
        self.member = resolved.member;
        self.token = resolved.token;
        self.state = resolved.state;
        self.sources = resolved.source;
        self.notices = notices;
        self.credentials = view.credentials;
        self.trust = view.trust;
        Ok(())
    }
}

/// The config file credentials are written to: `HUB_CONFIG`, else the user
/// file already in use, else the first place a user file would go.
pub fn credential_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let (hub_config, _system, candidates) = resolve_paths(env);
    hub_config.or_else(|| {
        candidates
            .iter()
            .find(|path| path.is_file())
            .or_else(|| candidates.first())
            .cloned()
    })
}

/// An empty value is the same as an unset one.
pub fn present(value: Option<String>) -> Option<String> {
    value.filter(|val| !val.is_empty())
}

/// Strip one layer of matching surrounding quotes.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Escape string for TOML serialization during migration.
fn escape_toml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Tell the operator when a file holding a token is readable by others.
#[cfg(unix)]
pub fn config_permissions_warning(path: &Path, mode: u32, has_token: bool) -> Option<String> {
    (has_token && mode & 0o077 != 0).then(|| {
        format!(
            "{} (mode {:04o}) holds a token and is readable beyond its owner; chmod 600 it",
            path.display(),
            mode & 0o7777
        )
    })
}

#[cfg(not(unix))]
pub fn config_permissions_warning(_path: &Path, _mode: u32, _has_token: bool) -> Option<String> {
    None
}

/// Everything the process needs to start.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory holding the hub store, session files, and artifact blobs.
    pub data_dir: PathBuf,
    /// Address the HTTP API and PWA bind to.
    pub bind: SocketAddr,
    /// External origin the hub is reached at, when the operator set one.
    pub public_url: Option<String>,
    /// Optional admin token for the control surface.
    pub admin_token: Option<String>,
    /// Ceilings on the open action items an agent may leave on the human.
    pub inbox_caps: InboxCaps,
    /// Ceiling on the events one project's feed may hold.
    pub events_per_project: EventCeiling,
    /// How long after its last tool call a session still counts its owner as
    /// an agent at work.
    pub active_window: std::time::Duration,
    /// The name the human sees for this node, when the operator set one.
    pub node_name: Option<String>,
    /// Whether agent self-enrolment is enabled.
    pub enrol_enabled: bool,
    /// The most pending enrolment requests the hub keeps at once.
    pub enrol_pending_max: usize,
    /// How long a pending enrolment waits before it counts as abandoned.
    pub enrol_pending_ttl: std::time::Duration,
    /// Peers whose forwarded client header the hub trusts, a proxy on the
    /// operator's own machine most often. Empty trusts none, and the socket
    /// peer is the identity.
    pub trusted_proxies: Vec<std::net::IpAddr>,
    /// Where the contentless "something is waiting" nudge goes. None is off.
    pub notify: Option<crate::notify::NotifyTarget>,
    /// Where the serving hub writes an online backup, one new directory per
    /// backup. Unset turns online backup off.
    pub backup_dir: Option<PathBuf>,
}

const ACTIVE_WINDOW_SECS: u64 = 900;
const ACTIVE_WINDOW_SECS_MAX: u64 = 30 * 24 * 60 * 60;

/// The embedded tailnet endpoint configuration.
#[derive(Debug, Clone, Default)]
pub struct Tailnet {
    /// A Tailscale auth key. Present when the endpoint is enabled.
    pub auth_key: Option<String>,
    /// Port to serve on the tailnet IP.
    pub port: u16,
    /// Directory holding the device key state, under the data directory.
    pub state_dir: PathBuf,
    /// Optional control server URL, for a self-hosted control plane.
    pub control_url: Option<url::Url>,
}

impl Tailnet {
    /// Whether the embedded endpoint is configured.
    pub fn enabled(&self) -> bool {
        self.auth_key.is_some()
    }
}

impl Config {
    /// Read configuration from environment and config files.
    pub fn from_env() -> Result<Self> {
        Self::resolve(&|key| std::env::var(key).ok())
    }

    /// Read only the data directory, without the serve-time checks.
    ///
    /// The offline data-lifecycle commands (`backup`, `restore`, `check`,
    /// `doctor`) act on the store alone, so they must not depend on a
    /// serve-time credential: a non-loopback bind needs an admin token to
    /// serve, but resolving where the store is does not. This reads the
    /// layered config files and `HUB_DATA_DIR` for `data_dir` only.
    pub fn data_dir_from_env() -> Result<PathBuf> {
        Self::resolve_data_dir(&|key| std::env::var(key).ok())
    }

    /// Read only the admin token, as the hub itself resolves it.
    ///
    /// `agent-hub backup --url` acts as the admin against a running hub, so it
    /// takes the token the hub serves with, from `HUB_ADMIN_TOKEN` or the
    /// layered config files, without the serve-time checks.
    pub fn admin_token_from_env() -> Result<Option<String>> {
        Self::resolve_admin_token(&|key| std::env::var(key).ok())
    }

    /// Resolve the admin token from an environment lookup and layered config
    /// files.
    pub fn resolve_admin_token(env: &dyn Fn(&str) -> Option<String>) -> Result<Option<String>> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();
        Ok(Setting::resolved(env, "hub", "admin_token", &files, None)
            .value
            .filter(|token| !token.is_empty()))
    }

    /// Resolve `data_dir` from an environment lookup and layered config files.
    pub fn resolve_data_dir(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();
        let data_dir = Setting::resolved(env, "hub", "data_dir", &files, Some("./data"))
            .value
            .unwrap_or_else(|| "./data".to_string());
        Ok(PathBuf::from(data_dir))
    }

    /// Resolve configuration from environment lookup and layered config files.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();

        let data_dir_str = Setting::resolved(env, "hub", "data_dir", &files, Some("./data"))
            .value
            .unwrap_or_else(|| "./data".to_string());
        let data_dir = PathBuf::from(data_dir_str);

        let bind_str = Setting::resolved(env, "hub", "bind", &files, Some("127.0.0.1:8080"))
            .value
            .unwrap_or_else(|| "127.0.0.1:8080".to_string());
        let bind: SocketAddr = bind_str
            .parse()
            .map_err(|err| Error::Config(format!("HUB_BIND is not a socket address: {err}")))?;

        let public_url = match Setting::resolved(env, "hub", "public_url", &files, None).value {
            Some(value) if !value.trim().is_empty() => Some(Self::parse_public_url(value.trim())?),
            _ => None,
        };

        let admin_token = Setting::resolved(env, "hub", "admin_token", &files, None)
            .value
            .filter(|token| !token.is_empty());

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

        let inbox_agent =
            Setting::resolved(env, "hub", "inbox_action_per_agent", &files, Some("100")).value;
        let inbox_project =
            Setting::resolved(env, "hub", "inbox_action_per_project", &files, Some("1000")).value;
        let inbox_caps = InboxCaps::parse(inbox_agent.as_deref(), inbox_project.as_deref())?;
        let events_per_project = EventCeiling::parse(
            Setting::resolved(env, "hub", "events_per_project", &files, Some("1000000"))
                .value
                .as_deref(),
        )?;

        let active_window_str =
            Setting::resolved(env, "hub", "active_window_secs", &files, Some("900")).value;
        let active_window = Self::parse_active_window(active_window_str.as_deref())?;

        let node_name = Setting::resolved(env, "hub", "node_name", &files, None)
            .value
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());

        let enrol_enabled = Setting::resolved(env, "hub", "enrol", &files, None)
            .value
            .map(|value| value.trim() != "off")
            .unwrap_or(true);

        let enrol_pending_max =
            Setting::resolved(env, "hub", "enrol_pending_max", &files, Some("20"))
                .value
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "20".to_string());
        let enrol_pending_max: usize = enrol_pending_max.parse().map_err(|_| {
            Error::Config(format!(
                "HUB_ENROL_PENDING_MAX must be a whole number, got '{enrol_pending_max}'"
            ))
        })?;
        let enrol_pending_ttl = Self::parse_enrol_pending_ttl(
            Setting::resolved(env, "hub", "enrol_pending_ttl_secs", &files, Some("86400"))
                .value
                .as_deref(),
        )?;
        let trusted_proxies = Self::parse_trusted_proxies(
            Setting::resolved(env, "hub", "trust_proxy", &files, None)
                .value
                .as_deref(),
        )?;
        let backup_dir = Self::parse_backup_dir(
            Setting::resolved(env, "hub", "backup_dir", &files, None)
                .value
                .as_deref(),
            &data_dir,
        )?;

        let notify = crate::notify::NotifyTarget::parse(
            Setting::resolved(env, "hub", "notify_url", &files, None)
                .value
                .as_deref(),
            Setting::resolved(env, "hub", "notify_token", &files, None)
                .value
                .as_deref(),
            Setting::resolved(env, "hub", "notify_interval_secs", &files, Some("60"))
                .value
                .as_deref(),
        )?;

        Ok(Self {
            data_dir,
            bind,
            public_url,
            admin_token,
            inbox_caps,
            events_per_project,
            active_window,
            node_name,
            enrol_enabled,
            enrol_pending_max,
            enrol_pending_ttl,
            trusted_proxies,
            notify,
            backup_dir,
        })
    }

    /// Read the online backup directory, refusing one inside the data
    /// directory.
    ///
    /// A backup written into the tree it copies would be walked as content by
    /// the next backup and pruned or restored along with the store it is meant
    /// to outlive, so the two must be apart. This is the lexical check, which
    /// catches a typo at startup; the hub compares the resolved paths again
    /// before each backup, which catches a symlink.
    pub fn parse_backup_dir(value: Option<&str>, data_dir: &Path) -> Result<Option<PathBuf>> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let dir = PathBuf::from(value);
        if backup_dir_inside(&dir, data_dir) {
            return Err(Error::Config(format!(
                "HUB_BACKUP_DIR {} must be outside the data directory {}",
                dir.display(),
                data_dir.display()
            )));
        }
        Ok(Some(dir))
    }

    /// Read the window an agent stays counted as active for.
    pub fn parse_active_window(value: Option<&str>) -> Result<std::time::Duration> {
        let secs = match value.map(str::trim).filter(|value| !value.is_empty()) {
            None => ACTIVE_WINDOW_SECS,
            Some(value) => match value.parse::<u64>() {
                Ok(secs) if secs > 0 && secs <= ACTIVE_WINDOW_SECS_MAX => secs,
                _ => {
                    return Err(Error::Config(format!(
                        "HUB_ACTIVE_WINDOW_SECS must be a whole number of seconds from 1 to {ACTIVE_WINDOW_SECS_MAX}, got '{value}'"
                    )));
                }
            },
        };
        Ok(std::time::Duration::from_secs(secs))
    }

    /// Read how long an abandoned enrolment waits before it is cleaned up.
    pub fn parse_enrol_pending_ttl(value: Option<&str>) -> Result<std::time::Duration> {
        const DEFAULT_SECS: u64 = 24 * 60 * 60;
        const MAX_SECS: u64 = 30 * 24 * 60 * 60;
        let secs = match value.map(str::trim).filter(|value| !value.is_empty()) {
            None => DEFAULT_SECS,
            Some(value) => match value.parse::<u64>() {
                Ok(secs) if secs > 0 && secs <= MAX_SECS => secs,
                _ => {
                    return Err(Error::Config(format!(
                        "HUB_ENROL_PENDING_TTL_SECS must be a whole number of seconds from 1 to {MAX_SECS}, got '{value}'"
                    )));
                }
            },
        };
        Ok(std::time::Duration::from_secs(secs))
    }

    /// Read how often the background store integrity sample runs. Zero
    /// disables it; the interval is bounded so a fast sample cannot spin.
    pub fn parse_integrity_sample_secs(value: Option<&str>) -> Result<std::time::Duration> {
        const DEFAULT_SECS: u64 = 0;
        const MAX_SECS: u64 = 7 * 24 * 60 * 60;
        let secs = match value.map(str::trim).filter(|value| !value.is_empty()) {
            None => DEFAULT_SECS,
            Some(value) => match value.parse::<u64>() {
                Ok(secs) if secs <= MAX_SECS => secs,
                _ => {
                    return Err(Error::Config(format!(
                        "HUB_INTEGRITY_SAMPLE_SECS must be a whole number of seconds from 0 (disabled) to {MAX_SECS}, got '{value}'"
                    )));
                }
            },
        };
        Ok(std::time::Duration::from_secs(secs))
    }

    /// Resolve the background integrity sample cadence from the environment and
    /// the layered config files. Kept off [`Config`] because only the sweeper
    /// reads it, and the sweeper resolves its own cadence the same way.
    pub fn integrity_sample_from_env() -> Result<std::time::Duration> {
        Self::integrity_sample_resolve(&|key| std::env::var(key).ok())
    }

    /// Resolve the integrity sample cadence from an environment lookup and the
    /// layered config files.
    pub fn integrity_sample_resolve(
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<std::time::Duration> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();
        let value = Setting::resolved(env, "hub", "integrity_sample_secs", &files, Some("0")).value;
        Self::parse_integrity_sample_secs(value.as_deref())
    }

    pub fn parse_trusted_proxies(value: Option<&str>) -> Result<Vec<std::net::IpAddr>> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Vec::new());
        };
        let mut proxies = Vec::new();
        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let ip = part.parse().map_err(|_| {
                Error::Config(format!(
                    "HUB_TRUST_PROXY entries must be IP addresses, got '{part}'"
                ))
            })?;
            proxies.push(ip);
        }
        Ok(proxies)
    }

    /// The instant a session must have been touched since to count as active.
    pub fn active_since(&self) -> String {
        let now = time::OffsetDateTime::now_utc();
        let window = time::Duration::try_from(self.active_window).unwrap_or(time::Duration::MAX);
        let since = now.checked_sub(window).unwrap_or_else(|| {
            tracing::warn!(
                window_secs = self.active_window.as_secs(),
                "the active window reaches beyond the calendar; counting from the earliest date"
            );
            time::OffsetDateTime::UNIX_EPOCH
        });
        since
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    }

    /// Normalise the operator's external origin.
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
        let authority = origin.split_once("://").map_or("", |(_, rest)| rest);
        if !crate::http::origin::is_safe_host(authority) {
            return Err(Error::Config(format!(
                "HUB_PUBLIC_URL has characters a host cannot carry, got '{value}'"
            )));
        }
        Ok(origin)
    }

    /// Whether a tailnet auth key is configured.
    pub fn tailnet_requested(&self) -> bool {
        self.tailnet_requested_with(&|key| std::env::var(key).ok())
    }

    pub fn tailnet_requested_with(&self, env: &dyn Fn(&str) -> Option<String>) -> bool {
        let loaded = load_layered_configs(env).unwrap_or_default();
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();
        Setting::resolved(env, "hub", "tailnet", &files, None)
            .value
            .is_some_and(|key| !key.is_empty())
    }

    /// Read the optional embedded tailnet endpoint configuration.
    pub fn tailnet_from_env(&self) -> Result<Tailnet> {
        self.tailnet_resolve(&|key| std::env::var(key).ok())
    }

    pub fn tailnet_resolve(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<Tailnet> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();

        let auth_key = Setting::resolved(env, "hub", "tailnet", &files, None)
            .value
            .filter(|key| !key.is_empty());

        let port_str = Setting::resolved(env, "hub", "tailnet_port", &files, Some("8080")).value;
        let port = match port_str {
            Some(value) => value
                .parse::<u16>()
                .map_err(|_| Error::Config(format!("HUB_TAILNET_PORT is not a port: {value}")))?,
            None => 8080,
        };

        let control_url_str =
            Setting::resolved(env, "hub", "tailnet_control_url", &files, None).value;
        let control_url = match control_url_str {
            Some(value) if !value.is_empty() => Some(value.parse().map_err(|err| {
                Error::Config(format!("HUB_TAILNET_CONTROL_URL is not a URL: {err}"))
            })?),
            _ => None,
        };

        let tailnet = Tailnet {
            auth_key,
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
    pub fn knowledge_dir(&self) -> PathBuf {
        crate::brain::knowledge_dir(&self.data_dir)
    }

    /// Directory holding artifact blobs.
    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }
}

/// The operating systems whose directory layout the hub knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    /// The layout of the system this binary runs on. Anything that is neither
    /// macOS nor Windows follows the XDG layout.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

/// An absolute directory from an environment variable, or `None` when it is
/// unset, empty or relative (the XDG rule for its base directories).
fn absolute_env(env: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<PathBuf> {
    present(env(key))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Where a native install keeps the store when `data_dir` is not set.
pub fn platform_data_dir(env: &dyn Fn(&str) -> Option<String>, os: Os) -> Option<PathBuf> {
    match os {
        Os::Linux => absolute_env(env, "XDG_DATA_HOME")
            .or_else(|| absolute_env(env, "HOME").map(|home| home.join(".local").join("share")))
            .map(|base| base.join("agent-hub")),
        Os::Macos => absolute_env(env, "HOME").map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("agent-hub")
        }),
        Os::Windows => {
            absolute_env(env, "LOCALAPPDATA").map(|base| base.join("agent-hub").join("data"))
        }
    }
}

/// Where a native install writes the log when `log_file` is not set.
pub fn platform_log_file(env: &dyn Fn(&str) -> Option<String>, os: Os) -> Option<PathBuf> {
    match os {
        Os::Linux => absolute_env(env, "XDG_STATE_HOME")
            .or_else(|| absolute_env(env, "HOME").map(|home| home.join(".local").join("state")))
            .map(|base| base.join("agent-hub").join("hub.log")),
        Os::Macos => absolute_env(env, "HOME").map(|home| {
            home.join("Library")
                .join("Logs")
                .join("agent-hub")
                .join("hub.log")
        }),
        Os::Windows => absolute_env(env, "LOCALAPPDATA")
            .map(|base| base.join("agent-hub").join("logs").join("hub.log")),
    }
}

/// The settings the lifecycle commands (`start`, `stop`, `status`, `logs`)
/// act on.
///
/// They read the same layered configuration `serve` does, but a data directory
/// or log file nobody set resolves to the platform's own place rather than
/// `./data`, so `agent-hub start` run from any directory finds the same hub.
/// `serve` itself keeps `./data`. A value that is set is used as it is, and the
/// `*_set` flags say which case it was, so a child process is handed only what
/// the configuration left open.
#[derive(Debug, Clone)]
pub struct LifecycleSettings {
    pub data_dir: PathBuf,
    pub data_dir_set: bool,
    pub log_file: PathBuf,
    pub log_file_set: bool,
    pub bind: SocketAddr,
    /// The config files the layers read, in precedence order.
    pub config_files: Vec<PathBuf>,
}

impl LifecycleSettings {
    /// Resolve from the process environment and the layered config files.
    pub fn from_env() -> Result<Self> {
        Self::resolve(&|key| std::env::var(key).ok(), Os::current())
    }

    /// Resolve from an environment lookup, for one operating system's layout.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>, os: Os) -> Result<Self> {
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();

        let data_dir_setting = Setting::resolved(env, "hub", "data_dir", &files, None);
        let (data_dir, data_dir_set) = match data_dir_setting.value {
            Some(value) => (PathBuf::from(value), true),
            None => (
                platform_data_dir(env, os).ok_or_else(|| {
                    Error::Config(
                        "no data directory: set HUB_DATA_DIR or [hub] data_dir".to_string(),
                    )
                })?,
                false,
            ),
        };
        let log_file_setting = Setting::resolved(env, "hub", "log_file", &files, None);
        let (log_file, log_file_set) = match log_file_setting.value {
            Some(value) => (PathBuf::from(value), true),
            None => (
                platform_log_file(env, os).ok_or_else(|| {
                    Error::Config("no log file: set HUB_LOG_FILE or [hub] log_file".to_string())
                })?,
                false,
            ),
        };
        let bind_str = Setting::resolved(env, "hub", "bind", &files, Some("127.0.0.1:8080"))
            .value
            .unwrap_or_else(|| "127.0.0.1:8080".to_string());
        let bind: SocketAddr = bind_str
            .parse()
            .map_err(|err| Error::Config(format!("HUB_BIND is not a socket address: {err}")))?;

        Ok(Self {
            data_dir,
            data_dir_set,
            log_file,
            log_file_set,
            bind,
            config_files: loaded.into_iter().map(|file| file.path).collect(),
        })
    }
}

/// Read only the log file a serving hub writes to, when one is set.
pub fn log_file_from_env() -> Result<Option<PathBuf>> {
    let env = |key: &str| std::env::var(key).ok();
    let loaded = load_layered_configs(&env)?;
    let files: Vec<&ParsedConfigFile> = loaded.iter().collect();
    Ok(Setting::resolved(&env, "hub", "log_file", &files, None)
        .value
        .map(PathBuf::from))
}

/// A parsed TOML configuration file.
#[derive(Debug, Clone, Default)]
pub struct ParsedConfigFile {
    pub path: PathBuf,
    pub hub: HashMap<String, String>,
    pub client: HashMap<String, String>,
    /// The credentials, trust pairings and client defaults the resolver reads.
    #[cfg(feature = "client")]
    pub view: crate::client::credential::ConfigView,
}

/// Load and parse a config file. An unknown key under `[hub]` or `[client]` is
/// an error naming the key; an unknown table warns and is ignored. A file that
/// others can read and that holds a token warns.
pub fn load_config_file(path: &Path) -> Result<Option<ParsedConfigFile>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(Error::Config(format!(
                "{} could not be read: {err}",
                path.display()
            )));
        }
    };

    let table = match contents.parse::<toml::Table>() {
        Ok(t) => t,
        Err(err) => {
            return Err(Error::Config(format!("{}: {err}", path.display())));
        }
    };

    let mut parsed = ParsedConfigFile {
        path: path.to_path_buf(),
        hub: HashMap::new(),
        client: HashMap::new(),
        #[cfg(feature = "client")]
        view: crate::client::credential::parse_config(&contents)?,
    };

    for (key, value) in &table {
        if key == "hub" {
            let Some(subtable) = value.as_table() else {
                return Err(Error::Config(format!(
                    "{}: [hub] must be a table",
                    path.display()
                )));
            };
            for (k, v) in subtable {
                if !HUB_KEYS.contains(&k.as_str()) {
                    return Err(Error::Config(format!(
                        "{}: unknown key '{k}' under '[hub]'",
                        path.display()
                    )));
                }
                let val_str = extract_value(path, "hub", k, v)?;
                if let Some(val) = val_str {
                    parsed.hub.insert(k.clone(), val);
                }
            }
        } else if key == "client" {
            let Some(subtable) = value.as_table() else {
                return Err(Error::Config(format!(
                    "{}: [client] must be a table",
                    path.display()
                )));
            };
            for (k, v) in subtable {
                if !CLIENT_KEYS.contains(&k.as_str()) && !LEGACY_CLIENT_KEYS.contains(&k.as_str()) {
                    return Err(Error::Config(format!(
                        "{}: unknown key '{k}' under '[client]'",
                        path.display()
                    )));
                }
                if k == "agent_id" {
                    eprintln!("agent-hub: {}: {AGENT_ID_WARNING}", path.display());
                    continue;
                }
                let val_str = extract_value(path, "client", k, v)?;
                if let Some(val) = val_str {
                    parsed.client.insert(k.clone(), val);
                }
            }
        } else if key == "credential" || key == "trust" {
            // The credential store and the trust list: read as text by the
            // resolver, which is the one reader of both.
        } else if value.is_table() {
            eprintln!("agent-hub: {}: unknown table '[{key}]'", path.display());
        } else {
            eprintln!("agent-hub: {}: unknown key '{key}'", path.display());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let has_token = parsed.client.contains_key("token")
            || table.contains_key("credential")
            || parsed.hub.contains_key("admin_token")
            || parsed.hub.contains_key("notify_token");
        if let Ok(metadata) = std::fs::metadata(path)
            && let Some(warning) =
                config_permissions_warning(path, metadata.permissions().mode(), has_token)
        {
            eprintln!("agent-hub: {warning}");
        }
    }

    Ok(Some(parsed))
}

fn extract_value(
    path: &Path,
    table: &str,
    key: &str,
    value: &toml::Value,
) -> Result<Option<String>> {
    match (table, key) {
        ("client", "timeout") => match value {
            toml::Value::Integer(i) if *i > 0 => Ok(Some(i.to_string())),
            toml::Value::Float(f) if f.is_finite() && *f > 0.0 => Ok(Some(f.to_string())),
            _ => Err(Error::Config(format!(
                "{}: [{table}] timeout must be a number of seconds above zero",
                path.display()
            ))),
        },
        (
            "hub",
            "active_window_secs"
            | "inbox_action_per_agent"
            | "inbox_action_per_project"
            | "events_per_project"
            | "enrol_pending_max"
            | "enrol_pending_ttl_secs"
            | "integrity_sample_secs"
            | "tailnet_port"
            | "notify_interval_secs",
        ) => match value {
            toml::Value::Integer(i) => Ok(Some(i.to_string())),
            _ => Err(Error::Config(format!(
                "{}: [{table}] {key} must be an integer",
                path.display()
            ))),
        },
        _ => match value {
            toml::Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(trimmed.to_string()))
                }
            }
            _ => Err(Error::Config(format!(
                "{}: [{table}] {key} must be a string",
                path.display()
            ))),
        },
    }
}

/// Whether `dir` is the data directory or under it, comparing the paths as
/// written, made absolute against the current directory with `.` and `..`
/// folded. Symlinks are not followed here.
pub fn backup_dir_inside(dir: &Path, data_dir: &Path) -> bool {
    let normal = |path: &Path| -> PathBuf {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let mut folded = PathBuf::new();
        for component in absolute.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    folded.pop();
                }
                other => folded.push(other),
            }
        }
        folded
    };
    normal(dir).starts_with(normal(data_dir))
}

/// Where a setting resolved from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingSource {
    Environment,
    File(PathBuf),
    Default,
}

/// A resolved configuration setting.
#[derive(Debug, Clone)]
pub struct Setting {
    pub value: Option<String>,
    pub source: SettingSource,
}

impl Setting {
    pub fn resolved(
        env: &dyn Fn(&str) -> Option<String>,
        table: &str,
        key: &str,
        files: &[&ParsedConfigFile],
        default_val: Option<&str>,
    ) -> Self {
        let env_var = format!("HUB_{}", key.to_ascii_uppercase());
        if let Some(val) = present(env(&env_var)) {
            return Self {
                value: Some(val),
                source: SettingSource::Environment,
            };
        }

        for file in files {
            let map = match table {
                "hub" => &file.hub,
                "client" => &file.client,
                _ => continue,
            };
            if let Some(val) = map.get(key)
                && let Some(val) = present(Some(val.clone()))
            {
                return Self {
                    value: Some(val),
                    source: SettingSource::File(file.path.clone()),
                };
            }
        }

        Self {
            value: default_val.map(str::to_string),
            source: SettingSource::Default,
        }
    }
}

/// The user's home directory: `HOME`, and on Windows `USERPROFILE` when `HOME`
/// is not set (Windows sets only the latter). Elsewhere `USERPROFILE` is never
/// read.
pub fn home_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    home_dir_for(env, cfg!(windows))
}

fn home_dir_for(env: &dyn Fn(&str) -> Option<String>, windows: bool) -> Option<String> {
    let home = present(env("HOME"));
    if windows {
        home.or_else(|| present(env("USERPROFILE")))
    } else {
        home
    }
}

/// The user config files in the order they are tried. On Windows the first is
/// `%APPDATA%\agent-hub\config.toml`; `APPDATA` is never read elsewhere.
fn user_config_candidates(env: &dyn Fn(&str) -> Option<String>, windows: bool) -> Vec<PathBuf> {
    let mut user_candidates = Vec::new();
    if windows && let Some(appdata) = present(env("APPDATA")) {
        user_candidates.push(PathBuf::from(appdata).join("agent-hub").join("config.toml"));
    }
    let home = home_dir_for(env, windows);
    if let Some(xdg) = present(env("XDG_CONFIG_HOME")) {
        user_candidates.push(PathBuf::from(xdg).join("agent-hub").join("config.toml"));
    } else if let Some(home) = &home {
        user_candidates.push(
            PathBuf::from(home)
                .join(".config")
                .join("agent-hub")
                .join("config.toml"),
        );
    }
    if let Some(home) = home {
        user_candidates.push(PathBuf::from(home).join(".agent-hub").join("config.toml"));
    }
    user_candidates
}

/// Resolve candidate paths for config files.
pub fn resolve_paths(
    env: &dyn Fn(&str) -> Option<String>,
) -> (Option<PathBuf>, PathBuf, Vec<PathBuf>) {
    let hub_config = present(env("HUB_CONFIG")).map(PathBuf::from);

    let system_path = present(env("AGENT_HUB_SYSTEM_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/agent-hub/config.toml"));

    (
        hub_config,
        system_path,
        user_config_candidates(env, cfg!(windows)),
    )
}

/// Check if migration from old env-style `~/.agent-hub/config` is needed.
pub fn check_and_perform_migration(
    env: &dyn Fn(&str) -> Option<String>,
    user_candidates: &[PathBuf],
) {
    let Some(home) = home_dir(env) else {
        return;
    };
    let old_path = PathBuf::from(&home).join(".agent-hub").join("config");
    if !old_path.is_file() {
        return;
    }
    if user_candidates.iter().any(|p| p.is_file()) {
        return;
    }

    let target_path = PathBuf::from(home).join(".agent-hub").join("config.toml");
    if let Err(err) = migrate_legacy_config(&old_path, &target_path) {
        eprintln!(
            "agent-hub: warning: could not migrate {} to {}: {err}",
            old_path.display(),
            target_path.display()
        );
    }
}

fn migrate_legacy_config(old_path: &Path, new_path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(old_path).map_err(|err| {
        Error::Config(format!("{}: could not be read: {err}", old_path.display()))
    })?;

    let mut client_entries = Vec::new();
    let mut hub_entries = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let k = k.trim();
            let v = unquote(v.trim());
            let lower = k.strip_prefix("HUB_").unwrap_or(k).to_ascii_lowercase();
            if HUB_KEYS.contains(&lower.as_str()) {
                hub_entries.push((lower, v.to_string()));
            } else {
                client_entries.push((lower, v.to_string()));
            }
        }
    }

    let mut toml_out = String::new();
    if !client_entries.is_empty() {
        toml_out.push_str("[client]\n");
        for (k, v) in client_entries {
            if k == "timeout"
                && let Ok(num) = v.parse::<f64>()
            {
                toml_out.push_str(&format!("{k} = {num}\n"));
                continue;
            }
            toml_out.push_str(&format!("{k} = \"{}\"\n", escape_toml_string(&v)));
        }
        toml_out.push('\n');
    }
    if !hub_entries.is_empty() {
        toml_out.push_str("[hub]\n");
        for (k, v) in hub_entries {
            if matches!(
                k.as_str(),
                "active_window_secs"
                    | "inbox_action_per_agent"
                    | "inbox_action_per_project"
                    | "events_per_project"
                    | "enrol_pending_max"
                    | "enrol_pending_ttl_secs"
                    | "integrity_sample_secs"
                    | "tailnet_port"
                    | "notify_interval_secs"
            ) && let Ok(num) = v.parse::<i64>()
            {
                toml_out.push_str(&format!("{k} = {num}\n"));
                continue;
            }
            toml_out.push_str(&format!("{k} = \"{}\"\n", escape_toml_string(&v)));
        }
    }

    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            Error::Config(format!(
                "{}: could not create directory: {err}",
                parent.display()
            ))
        })?;
    }

    std::fs::write(new_path, toml_out)
        .map_err(|err| Error::Config(format!("{}: could not write: {err}", new_path.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(new_path, std::fs::Permissions::from_mode(0o600));
    }

    eprintln!(
        "agent-hub: migrated {} to {}",
        old_path.display(),
        new_path.display()
    );

    Ok(())
}

/// Load configuration files in layer order (user first, then system; or HUB_CONFIG alone).
pub fn load_layered_configs(env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<ParsedConfigFile>> {
    let (hub_config, system_path, user_candidates) = resolve_paths(env);
    check_and_perform_migration(env, &user_candidates);

    if let Some(path) = hub_config {
        if let Some(parsed) = load_config_file(&path)? {
            return Ok(vec![parsed]);
        }
        return Ok(Vec::new());
    }

    let mut loaded = Vec::new();
    if let Some(user_path) = user_candidates.iter().find(|p| p.is_file())
        && let Some(parsed) = load_config_file(user_path)?
    {
        loaded.push(parsed);
    }
    if system_path.is_file()
        && let Some(parsed) = load_config_file(&system_path)?
    {
        loaded.push(parsed);
    }

    Ok(loaded)
}

/// Format token securely: first 4 chars + length, never in full.
pub fn mask_token(token: &str) -> String {
    let char_count = token.chars().count();
    let prefix: String = token.chars().take(4).collect();
    let unit = if char_count == 1 { "char" } else { "chars" };
    format!("{prefix}... ({char_count} {unit})")
}

fn format_source(source: &SettingSource, home: Option<&str>) -> String {
    match source {
        SettingSource::Environment => "environment".to_string(),
        SettingSource::Default => "default".to_string(),
        SettingSource::File(path) => {
            let path_str = path.display().to_string();
            if let Some(home) = home
                && let Some(rest) = path_str.strip_prefix(home)
            {
                return format!("~{rest}");
            }
            path_str
        }
    }
}

fn format_path_display(path: &Path, home: Option<&str>) -> String {
    let path_str = path.display().to_string();
    if let Some(home) = home
        && let Some(rest) = path_str.strip_prefix(home)
    {
        return format!("~{rest}");
    }
    path_str
}

pub struct ConfigReportRow {
    pub col1: String,
    pub env_var: String,
    pub value: String,
    pub source: String,
}

pub fn generate_config_rows(env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<ConfigReportRow>> {
    let loaded = load_layered_configs(env)?;
    let files: Vec<&ParsedConfigFile> = loaded.iter().collect();
    let home = home_dir(env);

    let defs: Vec<(&str, &str, &str, Option<&str>, bool)> = vec![
        ("hub", "bind", "HUB_BIND", Some("127.0.0.1:8080"), false),
        ("hub", "data_dir", "HUB_DATA_DIR", Some("./data"), false),
        ("hub", "admin_token", "HUB_ADMIN_TOKEN", None, true),
        ("hub", "node_name", "HUB_NODE_NAME", None, false),
        ("hub", "public_url", "HUB_PUBLIC_URL", None, false),
        (
            "hub",
            "active_window_secs",
            "HUB_ACTIVE_WINDOW_SECS",
            Some("900"),
            false,
        ),
        (
            "hub",
            "inbox_action_per_agent",
            "HUB_INBOX_ACTION_PER_AGENT",
            Some("100"),
            false,
        ),
        (
            "hub",
            "inbox_action_per_project",
            "HUB_INBOX_ACTION_PER_PROJECT",
            Some("1000"),
            false,
        ),
        (
            "hub",
            "events_per_project",
            "HUB_EVENTS_PER_PROJECT",
            Some("1000000"),
            false,
        ),
        ("hub", "enrol", "HUB_ENROL", Some("on"), false),
        (
            "hub",
            "enrol_pending_max",
            "HUB_ENROL_PENDING_MAX",
            Some("20"),
            false,
        ),
        (
            "hub",
            "enrol_pending_ttl_secs",
            "HUB_ENROL_PENDING_TTL_SECS",
            Some("86400"),
            false,
        ),
        (
            "hub",
            "integrity_sample_secs",
            "HUB_INTEGRITY_SAMPLE_SECS",
            Some("0"),
            false,
        ),
        ("hub", "trust_proxy", "HUB_TRUST_PROXY", None, false),
        ("hub", "tailnet", "HUB_TAILNET", None, true),
        (
            "hub",
            "tailnet_port",
            "HUB_TAILNET_PORT",
            Some("8080"),
            false,
        ),
        (
            "hub",
            "tailnet_control_url",
            "HUB_TAILNET_CONTROL_URL",
            None,
            false,
        ),
        ("hub", "notify_url", "HUB_NOTIFY_URL", None, false),
        ("hub", "notify_token", "HUB_NOTIFY_TOKEN", None, true),
        (
            "hub",
            "notify_interval_secs",
            "HUB_NOTIFY_INTERVAL_SECS",
            Some("60"),
            false,
        ),
        ("hub", "backup_dir", "HUB_BACKUP_DIR", None, false),
        ("hub", "log_file", "HUB_LOG_FILE", None, false),
        ("client", "url", "HUB_URL", None, false),
        ("client", "token", "HUB_TOKEN", None, true),
        ("client", "member", "HUB_MEMBER", None, false),
        ("client", "project", "HUB_PROJECT", None, false),
        ("client", "timeout", "HUB_TIMEOUT", Some("120"), false),
    ];

    let mut rows = Vec::new();
    for (table, key, env_var, default_val, is_secret) in defs {
        let setting = Setting::resolved(env, table, key, &files, default_val);
        let value_display = match setting.value {
            None => "(unset)".to_string(),
            Some(val) if is_secret => mask_token(&val),
            // A topic path or a query can be a credential on some targets.
            Some(val) if key == "notify_url" => crate::notify::redact_url_str(&val),
            Some(val) => val,
        };
        let source_display = format_source(&setting.source, home.as_deref());
        rows.push(ConfigReportRow {
            col1: format!("[{table}] {key}"),
            env_var: env_var.to_string(),
            value: value_display,
            source: source_display,
        });
    }

    Ok(rows)
}

/// Print the report of every setting, its value, and where it came from.
pub fn print_config(env: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    let rows = generate_config_rows(env)?;
    let w1 = rows.iter().map(|r| r.col1.len()).max().unwrap_or(18) + 2;
    let w2 = rows.iter().map(|r| r.env_var.len()).max().unwrap_or(28) + 2;
    let w3 = rows.iter().map(|r| r.value.len()).max().unwrap_or(20) + 2;

    for row in rows {
        println!(
            "{:<w1$}{:<w2$}{:<w3$}{}",
            row.col1, row.env_var, row.value, row.source
        );
    }
    Ok(())
}

/// The real user config path to name when a client has no hub URL. The file is
/// the one enrolment writes and the one reads, so the message points at the
/// path the operator actually has.
pub fn client_config_hint(env: &dyn Fn(&str) -> Option<String>) -> String {
    match user_config_target(env) {
        Some(path) => format_path_display(&path, home_dir(env).as_deref()),
        None => "config.toml".to_string(),
    }
}

/// Print the configuration files that would be read, found or not.
pub fn print_config_paths(env: &dyn Fn(&str) -> Option<String>) {
    let (hub_config, system_path, user_candidates) = resolve_paths(env);
    check_and_perform_migration(env, &user_candidates);

    let home = home_dir(env);
    let mut rows: Vec<(&str, String, &str)> = Vec::new();

    if let Some(path) = hub_config {
        let status = if path.is_file() { "found" } else { "not found" };
        rows.push((
            "custom",
            format_path_display(&path, home.as_deref()),
            status,
        ));
        rows.push((
            "system",
            format_path_display(&system_path, home.as_deref()),
            "not read (replaced by HUB_CONFIG)",
        ));
        for cand in &user_candidates {
            rows.push((
                "user",
                format_path_display(cand, home.as_deref()),
                "not read (replaced by HUB_CONFIG)",
            ));
        }
    } else {
        let status = if system_path.is_file() {
            "found"
        } else {
            "not found"
        };
        rows.push((
            "system",
            format_path_display(&system_path, home.as_deref()),
            status,
        ));

        let cand1 = user_candidates.first();
        let cand2 = user_candidates.get(1);

        if let Some(c1) = cand1 {
            if c1.is_file() {
                rows.push(("user", format_path_display(c1, home.as_deref()), "found"));
                if let Some(c2) = cand2 {
                    rows.push((
                        "user",
                        format_path_display(c2, home.as_deref()),
                        "not read (shadowed)",
                    ));
                }
            } else if let Some(c2) = cand2 {
                if c2.is_file() {
                    rows.push((
                        "user",
                        format_path_display(c1, home.as_deref()),
                        "not found",
                    ));
                    rows.push(("user", format_path_display(c2, home.as_deref()), "found"));
                } else {
                    rows.push((
                        "user",
                        format_path_display(c1, home.as_deref()),
                        "not found",
                    ));
                    rows.push((
                        "user",
                        format_path_display(c2, home.as_deref()),
                        "not found",
                    ));
                }
            } else {
                rows.push((
                    "user",
                    format_path_display(c1, home.as_deref()),
                    "not found",
                ));
            }
        }
    }

    let w_path = rows.iter().map(|(_, p, _)| p.len()).max().unwrap_or(36) + 2;
    for (level, path, status) in rows {
        println!("{:<8}{:<w_path$}{}", level, path, status);
    }
}

/// Validate process and client configuration, exiting with error if invalid.
pub fn validate_configuration(env: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    let (hub_config, system_path, user_candidates) = resolve_paths(env);
    check_and_perform_migration(env, &user_candidates);

    if let Some(path) = &hub_config {
        let _ = load_config_file(path)?;
    } else {
        if system_path.is_file() {
            let _ = load_config_file(&system_path)?;
        }
        if let Some(user_path) = user_candidates.iter().find(|p| p.is_file()) {
            let _ = load_config_file(user_path)?;
        }
    }

    let config = Config::resolve(env)?;
    ClientConfig::resolve(env)?;
    check_data_dir(&config.data_dir)?;

    Ok(())
}

/// The data directory must be writable, or its nearest existing ancestor must
/// be, so the hub can create it on start. `config --check` turns a bad path
/// into one clear line rather than a failure at first write.
fn check_data_dir(path: &std::path::Path) -> Result<()> {
    let mut probe = path;
    while !probe.exists() {
        match probe.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => probe = parent,
            _ => break,
        }
    }
    if !probe.is_dir() {
        return Err(Error::Config(format!(
            "the data directory {} is not a directory",
            path.display()
        )));
    }
    let marker = probe.join(".agent-hub-write-check");
    match std::fs::File::create(&marker) {
        Ok(_) => {
            let _ = std::fs::remove_file(&marker);
            Ok(())
        }
        Err(err) => Err(Error::Config(format!(
            "the data directory {} is not writable: {err}",
            probe.display()
        ))),
    }
}

/// The user config file a client writes its token to: the first candidate
/// `resolve_paths` names, so the file that is read is the file that is written.
pub fn user_config_target(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let (hub_config, _system, candidates) = resolve_paths(env);
    hub_config.or_else(|| candidates.into_iter().next())
}

/// Hold the lock that serialises every read, change and replace of the config
/// file at `path`, so two writers (an install and an enrolment, say) cannot
/// each read the old file and the second undo the first.
///
/// The lock is on `.<file name>.agent-hub.lock`, a file beside the config
/// that is never unlinked (removing a lock file lets two writers each hold a
/// lock on a different inode). It is released when the returned handle is
/// dropped.
fn lock_config(path: &Path) -> std::io::Result<std::fs::File> {
    let lock_path = config_lock_path(path);
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(lock_path)?;
    file.lock()?;
    Ok(file)
}

/// The lock file that guards the config at `path`.
fn config_lock_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    path.with_file_name(format!(".{file_name}.agent-hub.lock"))
}

/// Write `contents` to `path` at mode 0600 through a temporary file beside it.
///
/// The temporary file has a name of its own and is created exclusively at
/// 0600, so a file someone else left at that name is never written into, and
/// the token is never in a file with broader permissions. It is renamed into
/// place, so the config is never half-written.
fn write_private_file(path: &Path, contents: &str) -> std::io::Result<()> {
    use rand::RngCore;
    let (temp_path, mut file) = loop {
        let mut temp_name = path.as_os_str().to_owned();
        temp_name.push(format!(
            ".tmp-{}-{:016x}",
            std::process::id(),
            rand::rng().next_u64()
        ));
        let temp_path = PathBuf::from(temp_name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp_path) {
            Ok(file) => break (temp_path, file),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    };

    let write = (|| -> std::io::Result<()> {
        use std::io::Write;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)
    })();

    if write.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write
}

/// What [`merge_missing_settings`] did to one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merged {
    /// The key was missing and is now in the file.
    Added,
    /// The key was already set, and the file still holds its own value.
    Kept,
}

/// Add the named keys to `[table]` of the config file at `path`, only where the
/// key is missing, and say which were added and which were kept.
///
/// This is the writer behind `service install`. It never changes a value the
/// file already holds (an empty string holds nothing: it counts as missing and
/// is replaced), and comments, order and layout stay as they were. The
/// file is created (0600, in an owner-only directory when the directory is new)
/// if it does not exist, and nothing is written when no key is missing. A file
/// that is not valid TOML is refused, not rewritten. The whole read, merge and
/// replace runs under the config file's lock.
pub fn merge_missing_settings(
    path: &Path,
    table: &str,
    settings: &[(&str, &str)],
) -> std::io::Result<Vec<(String, Merged)>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        private_dir(parent)?;
    }
    let _lock = lock_config(path)?;
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let content = existing.clone().unwrap_or_default();
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    let parsed: toml::Table = content
        .parse()
        .map_err(|err| invalid(format!("{}: {err}", path.display())))?;
    let present = |key: &str| {
        parsed
            .get(table)
            .and_then(toml::Value::as_table)
            .and_then(|section| section.get(key))
            .is_some_and(|value| value.as_str() != Some(""))
    };
    let missing: Vec<(&str, &str)> = settings
        .iter()
        .copied()
        .filter(|(key, _)| !present(key))
        .collect();
    let outcome = settings
        .iter()
        .map(|(key, _)| {
            let merged = if missing.iter().any(|(wanted, _)| wanted == key) {
                Merged::Added
            } else {
                Merged::Kept
            };
            ((*key).to_string(), merged)
        })
        .collect();
    if missing.is_empty() && existing.is_some() {
        return Ok(outcome);
    }
    let updated = insert_missing_keys(&content, table, &missing)
        .map_err(|message| invalid(format!("{}: {message}", path.display())))?;
    write_private_file(path, &updated)?;
    Ok(outcome)
}

/// Put `keys` at the end of `[table]`, or add the table, keeping every comment
/// and the rest of the layout.
///
/// The result is parsed back and compared with the original: the only change
/// allowed is the requested keys appearing in the table. Anything else is an
/// error and nothing is written.
fn insert_missing_keys(
    content: &str,
    table: &str,
    keys: &[(&str, &str)],
) -> std::result::Result<String, String> {
    let original: toml::Table = content.parse().map_err(|err| format!("{err}"))?;
    let mut document: toml_edit::DocumentMut = content.parse().map_err(|err| format!("{err}"))?;
    let had_content = !document.as_table().is_empty();
    if !document.contains_key(table) {
        let mut new_table = toml_edit::Table::new();
        if had_content {
            new_table.decor_mut().set_prefix("\n");
        }
        document.insert(table, toml_edit::Item::Table(new_table));
    }
    let section = document
        .get_mut(table)
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| format!("{table} is not a table"))?;
    for (key, value) in keys {
        section.insert(key, toml_edit::value(*value));
    }
    let text = document.to_string();

    let mut expected = original;
    let entry = expected
        .entry(table.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(section) = entry else {
        return Err(format!("{table} is not a table"));
    };
    for (key, value) in keys {
        section.insert(
            (*key).to_string(),
            toml::Value::String((*value).to_string()),
        );
    }
    let written: toml::Table = text
        .parse()
        .map_err(|err| format!("adding the keys would leave invalid TOML ({err})"))?;
    if written != expected {
        return Err("adding the keys would change another setting; add them by hand".to_string());
    }
    Ok(text)
}

/// What [`remove_hub_table`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubRemoval {
    /// The file or its `[hub]` table was not there.
    Absent,
    /// The table was cut out and the rest of the file kept.
    TableRemoved,
    /// The table was the whole file, so the file went with it.
    FileDeleted,
}

/// Remove the `[hub]` table from the config file at `path`, and the file too
/// when nothing else is left in it.
///
/// The `[client]` table is never touched: agents and plugins on the machine use
/// it. The result is parsed and compared with the original before anything is
/// written, so a layout the line scan misreads is an error, not a lost setting.
pub fn remove_hub_table(path: &Path) -> std::io::Result<HubRemoval> {
    if !path.exists() {
        return Ok(HubRemoval::Absent);
    }
    let _lock = lock_config(path)?;
    let content = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HubRemoval::Absent),
        Err(err) => return Err(err),
    };
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    let original: toml::Table = content
        .parse()
        .map_err(|err| invalid(format!("{}: {err}", path.display())))?;
    if !original.contains_key("hub") {
        return Ok(HubRemoval::Absent);
    }
    let lines: Vec<&str> = content.lines().collect();
    let is_header = |line: &str| line.trim_start().starts_with('[');
    let hub_header = |line: &str| {
        let rest = line.trim_start();
        rest.strip_prefix("[hub]")
            .is_some_and(|tail| tail.trim().is_empty() || tail.trim_start().starts_with('#'))
    };
    let Some(start) = lines.iter().position(|line| hub_header(line)) else {
        return Err(invalid(format!(
            "{}: the [hub] table is not in a form that can be cut out; remove it by hand",
            path.display()
        )));
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| is_header(line))
        .map_or(lines.len(), |(index, _)| index);
    let kept: Vec<&str> = lines[..start]
        .iter()
        .chain(lines[end..].iter())
        .copied()
        .collect();
    let mut text = kept.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let remaining: toml::Table = text.parse().map_err(|err| {
        invalid(format!(
            "{}: cutting [hub] out leaves invalid TOML ({err}); remove it by hand",
            path.display()
        ))
    })?;
    let mut expected = original.clone();
    expected.remove("hub");
    if remaining != expected {
        return Err(invalid(format!(
            "{}: cutting [hub] out would change another setting; remove it by hand",
            path.display()
        )));
    }
    if remaining.is_empty() {
        std::fs::remove_file(path)?;
        return Ok(HubRemoval::FileDeleted);
    }
    write_private_file(path, &text)?;
    Ok(HubRemoval::TableRemoved)
}

/// Create a directory and make it readable only by its owner.
///
/// The data directory holds every session brain, knowledge page and artifact
/// blob. On Unix it is created `0700`, and an existing directory broader than
/// that is repaired rather than trusted: a permissive umask would otherwise
/// leave the hub's whole store readable by any other local account. H11.
pub fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)?;
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Restrict a file that already exists to its owner (`0600`). A missing file is
/// not an error: the caller hardens what it just created, and sidecars appear
/// only once the engine has written them. H11.
pub fn private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) => {
                if metadata.permissions().mode() & 0o077 != 0 {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                }
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// The store's own files are private to the operator's account. These run under
/// the build tree, never the system temp directory.
#[cfg(all(test, unix))]
mod private_mode_tests {
    use super::{private_dir, private_file};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let unique = format!(
            "private-mode-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after the epoch")
                .as_nanos()
        );
        PathBuf::from("target/tmp").join(unique)
    }

    fn mode(path: &PathBuf) -> u32 {
        std::fs::metadata(path)
            .expect("path exists")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn private_dir_creates_and_repairs_owner_only() {
        let dir = scratch("dir");
        std::fs::create_dir_all(&dir).expect("create scratch");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("loosen");
        private_dir(&dir).expect("harden");
        assert_eq!(mode(&dir), 0o700, "a broad directory is repaired to 0700");

        let nested = dir.join("child");
        private_dir(&nested).expect("create child");
        assert_eq!(mode(&nested), 0o700, "a created directory is 0700");

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn private_file_repairs_and_tolerates_absence() {
        let dir = scratch("file");
        std::fs::create_dir_all(&dir).expect("create scratch");
        let file = dir.join("hub.db");
        std::fs::write(&file, b"x").expect("write");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("loosen");
        private_file(&file).expect("harden");
        assert_eq!(mode(&file), 0o600, "a broad file is repaired to 0600");

        private_file(&dir.join("missing-wal")).expect("a missing sidecar is not an error");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}

#[cfg(test)]
mod platform_tests {
    use super::{Os, platform_data_dir, platform_log_file};
    use std::path::PathBuf;

    /// An absolute directory on every platform, so the environment values
    /// below are accepted whatever the separator is.
    fn base(name: &str) -> PathBuf {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        PathBuf::from(root).join(name)
    }

    fn env(pairs: &[(&str, &PathBuf)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string_lossy().into_owned()))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn linux_follows_xdg_and_falls_back_to_home() {
        let hubhome = base("hubhome");
        let home = env(&[("HOME", &hubhome)]);
        assert_eq!(
            platform_data_dir(&home, Os::Linux),
            Some(hubhome.join(".local").join("share").join("agent-hub"))
        );
        assert_eq!(
            platform_log_file(&home, Os::Linux),
            Some(
                hubhome
                    .join(".local")
                    .join("state")
                    .join("agent-hub")
                    .join("hub.log")
            )
        );
        let data = base("srv").join("data");
        let state = base("srv").join("state");
        let xdg = env(&[
            ("HOME", &hubhome),
            ("XDG_DATA_HOME", &data),
            ("XDG_STATE_HOME", &state),
        ]);
        assert_eq!(
            platform_data_dir(&xdg, Os::Linux),
            Some(data.join("agent-hub"))
        );
        assert_eq!(
            platform_log_file(&xdg, Os::Linux),
            Some(state.join("agent-hub").join("hub.log"))
        );
    }

    #[test]
    fn a_relative_xdg_base_is_ignored() {
        let hubhome = base("hubhome");
        let data = PathBuf::from("data");
        let relative = env(&[("HOME", &hubhome), ("XDG_DATA_HOME", &data)]);
        assert_eq!(
            platform_data_dir(&relative, Os::Linux),
            Some(hubhome.join(".local").join("share").join("agent-hub"))
        );
    }

    #[test]
    fn macos_uses_the_library_layout() {
        let machome = base("machome");
        let home = env(&[("HOME", &machome)]);
        assert_eq!(
            platform_data_dir(&home, Os::Macos),
            Some(
                machome
                    .join("Library")
                    .join("Application Support")
                    .join("agent-hub")
            )
        );
        assert_eq!(
            platform_log_file(&home, Os::Macos),
            Some(
                machome
                    .join("Library")
                    .join("Logs")
                    .join("agent-hub")
                    .join("hub.log")
            )
        );
    }

    #[test]
    fn no_home_means_no_default() {
        let none = env(&[]);
        assert_eq!(platform_data_dir(&none, Os::Linux), None);
        assert_eq!(platform_log_file(&none, Os::Macos), None);
    }
}

#[cfg(test)]
mod merge_tests {
    use super::{
        HubRemoval, Merged, insert_missing_keys, merge_missing_settings, remove_hub_table,
    };
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            PathBuf::from("target/tmp").join(format!("config-merge-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    #[test]
    fn a_missing_table_is_added_at_the_end() {
        let text = insert_missing_keys("[client]\nurl = \"http://hub\"\n", "hub", &[("bind", "x")])
            .expect("insert");
        assert_eq!(
            text,
            "[client]\nurl = \"http://hub\"\n\n[hub]\nbind = \"x\"\n"
        );
    }

    #[test]
    fn keys_land_inside_an_existing_table_and_leave_the_rest_alone() {
        let before =
            "# mine\n[hub]\nnode_name = \"n\"\n\n# about client\n[client]\ntoken = \"t\"\n";
        let text = insert_missing_keys(before, "hub", &[("bind", "b")]).expect("insert");
        assert_eq!(
            text,
            "# mine\n[hub]\nnode_name = \"n\"\nbind = \"b\"\n\n# about client\n[client]\ntoken = \"t\"\n"
        );
    }

    #[test]
    fn comments_and_inline_comments_survive_a_merge() {
        let before = "# top\n[hub] # the server\nnode_name = \"n\" # shown in the PWA\n# trailing\n\n[client]\nurl = \"u\"\n";
        let text = insert_missing_keys(before, "hub", &[("bind", "b")]).expect("insert");
        for comment in ["# top", "# the server", "# shown in the PWA", "# trailing"] {
            assert!(text.contains(comment), "{comment} kept: {text}");
        }
        let parsed: toml::Table = text.parse().expect("valid");
        assert_eq!(parsed["hub"]["bind"].as_str(), Some("b"));
        assert_eq!(parsed["hub"]["node_name"].as_str(), Some("n"));
        assert_eq!(parsed["client"]["url"].as_str(), Some("u"));
    }

    #[test]
    fn a_quoted_table_header_is_the_same_table() {
        let before = "[\"hub\"]\nnode_name = \"n\"\n";
        let text = insert_missing_keys(before, "hub", &[("bind", "b")]).expect("insert");
        let parsed: toml::Table = text.parse().expect("no duplicate table");
        let hub = parsed["hub"].as_table().expect("hub");
        assert_eq!(hub.len(), 2, "{text}");
        assert_eq!(hub["bind"].as_str(), Some("b"));
    }

    #[test]
    fn a_multiline_string_that_looks_like_a_header_is_left_whole() {
        let before = "[hub]\nnode_name = \"\"\"\n[client]\nurl = \"fake\"\n\"\"\"\n\n[client]\nurl = \"u\"\n";
        let text = insert_missing_keys(before, "hub", &[("bind", "b")]).expect("insert");
        let parsed: toml::Table = text.parse().expect("valid");
        assert_eq!(
            parsed["hub"]["node_name"].as_str(),
            Some("[client]\nurl = \"fake\"\n")
        );
        assert_eq!(parsed["hub"]["bind"].as_str(), Some("b"));
        assert_eq!(parsed["client"].as_table().unwrap().len(), 1);
        assert_eq!(parsed["client"]["url"].as_str(), Some("u"));
    }

    #[test]
    fn a_key_that_is_not_a_table_is_refused() {
        assert!(insert_missing_keys("hub = 1\n", "hub", &[("bind", "b")]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_temporary_file_is_created_exclusively_at_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("exclusive");
        let file = dir.join("config.toml");
        // A readable file at the name the old writer would have reused.
        let planted = dir.join(format!("config.toml.tmp-{}", std::process::id()));
        std::fs::write(&planted, "planted").unwrap();
        std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o644)).unwrap();
        merge_missing_settings(&file, "hub", &[("admin_token", "secret")]).expect("create");
        assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted");
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert_eq!(names.len(), 1, "no temporary file is left: {names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn concurrent_merges_each_keep_what_the_others_added() {
        let dir = scratch("concurrent");
        let file = dir.join("config.toml");
        let keys = [
            "bind",
            "data_dir",
            "admin_token",
            "node_name",
            "log_file",
            "backup_dir",
        ];
        std::thread::scope(|scope| {
            for key in keys {
                let file = &file;
                scope.spawn(move || {
                    merge_missing_settings(file, "hub", &[(key, "v")]).expect("merge");
                });
            }
        });
        let parsed: toml::Table = std::fs::read_to_string(&file).unwrap().parse().unwrap();
        assert_eq!(
            parsed["hub"].as_table().unwrap().len(),
            keys.len(),
            "{parsed:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_merge_adds_only_what_is_missing_and_keeps_the_rest() {
        let dir = scratch("keep");
        let file = dir.join("agent-hub").join("config.toml");
        let first = merge_missing_settings(
            &file,
            "hub",
            &[("bind", "127.0.0.1:8080"), ("data_dir", "/d")],
        )
        .expect("create");
        assert_eq!(
            first,
            vec![
                ("bind".to_string(), Merged::Added),
                ("data_dir".to_string(), Merged::Added)
            ]
        );
        std::fs::write(
            &file,
            std::fs::read_to_string(&file)
                .unwrap()
                .replace("8080", "9999"),
        )
        .unwrap();
        let again = merge_missing_settings(
            &file,
            "hub",
            &[("bind", "127.0.0.1:8080"), ("admin_token", "tok")],
        )
        .expect("merge");
        assert_eq!(
            again,
            vec![
                ("bind".to_string(), Merged::Kept),
                ("admin_token".to_string(), Merged::Added)
            ]
        );
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("bind = \"127.0.0.1:9999\""), "{text}");
        assert!(text.contains("admin_token = \"tok\""), "{text}");
        let parsed: toml::Table = text.parse().expect("still valid TOML");
        assert_eq!(parsed["hub"].as_table().unwrap().len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only_and_a_new_directory_is_too() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let file = dir.join("new").join("config.toml");
        merge_missing_settings(&file, "hub", &[("bind", "b")]).expect("create");
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_toml_is_refused_and_left_as_it_was() {
        let dir = scratch("bad");
        let file = dir.join("config.toml");
        std::fs::write(&file, "[hub\nbind = ").unwrap();
        let err = merge_missing_settings(&file, "hub", &[("bind", "b")]).expect_err("refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[hub\nbind = ");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_value_counts_as_missing_and_is_replaced_in_place() {
        let dir = scratch("empty");
        let file = dir.join("config.toml");
        std::fs::write(&file, "[hub]\nadmin_token = \"\"\nbind = \"b\"\n").unwrap();
        let merged =
            merge_missing_settings(&file, "hub", &[("admin_token", "t"), ("bind", "x")]).unwrap();
        assert_eq!(
            merged,
            vec![
                ("admin_token".to_string(), Merged::Added),
                ("bind".to_string(), Merged::Kept)
            ]
        );
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "[hub]\nadmin_token = \"t\"\nbind = \"b\"\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removing_the_hub_table_keeps_the_client_table() {
        let dir = scratch("remove");
        let file = dir.join("config.toml");
        std::fs::write(
            &file,
            "[hub]\nbind = \"b\"\nadmin_token = \"t\"\n\n[client]\nurl = \"http://hub\"\n",
        )
        .unwrap();
        assert_eq!(remove_hub_table(&file).unwrap(), HubRemoval::TableRemoved);
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "[client]\nurl = \"http://hub\"\n");
        assert_eq!(remove_hub_table(&file).unwrap(), HubRemoval::Absent);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_with_only_the_hub_table_goes_with_it() {
        let dir = scratch("whole");
        let file = dir.join("config.toml");
        std::fs::write(&file, "# agent-hub\n[hub]\nbind = \"b\"\n").unwrap();
        assert_eq!(remove_hub_table(&file).unwrap(), HubRemoval::FileDeleted);
        assert!(!file.exists());
        assert!(
            dir.join(".config.toml.agent-hub.lock").exists(),
            "the writer lock file is never unlinked"
        );
        assert_eq!(remove_hub_table(&file).unwrap(), HubRemoval::Absent);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hub_table_the_scan_cannot_cut_cleanly_is_refused() {
        let dir = scratch("array");
        let file = dir.join("config.toml");
        let text = "[hub]\ntrust_proxy = [\n[\"1.1.1.1\"],\n]\n[client]\nurl = \"u\"\n";
        std::fs::write(&file, text).unwrap();
        // The array's inner line looks like a table header. Either the file is
        // cut exactly (only [client] left) or it is refused untouched.
        match remove_hub_table(&file) {
            Ok(_) => {
                let left: toml::Table = std::fs::read_to_string(&file).unwrap().parse().unwrap();
                assert_eq!(left.keys().collect::<Vec<_>>(), vec!["client"]);
            }
            Err(_) => assert_eq!(std::fs::read_to_string(&file).unwrap(), text),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod windows_path_tests {
    use super::{home_dir_for, user_config_candidates};
    use std::path::PathBuf;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn home_falls_back_to_the_user_profile_on_windows_only() {
        let profile = env(&[("USERPROFILE", "C:\\Users\\u")]);
        assert_eq!(
            home_dir_for(&profile, true).as_deref(),
            Some("C:\\Users\\u")
        );
        assert_eq!(home_dir_for(&profile, false), None);
        let both = env(&[("HOME", "/h"), ("USERPROFILE", "C:\\Users\\u")]);
        assert_eq!(home_dir_for(&both, true).as_deref(), Some("/h"));
        let empty = env(&[("HOME", ""), ("USERPROFILE", "C:\\Users\\u")]);
        assert_eq!(
            home_dir_for(&empty, true).as_deref(),
            Some("C:\\Users\\u"),
            "an empty HOME is unset"
        );
    }

    #[test]
    fn appdata_is_the_first_user_candidate_on_windows() {
        let vars = env(&[
            ("APPDATA", "C:\\Users\\u\\AppData\\Roaming"),
            ("HOME", "/h"),
        ]);
        let found = user_config_candidates(&vars, true);
        assert_eq!(
            found.first(),
            Some(
                &PathBuf::from("C:\\Users\\u\\AppData\\Roaming")
                    .join("agent-hub")
                    .join("config.toml")
            )
        );
        assert_eq!(
            found.len(),
            3,
            "APPDATA, ~/.config and ~/.agent-hub: {found:?}"
        );
    }

    #[test]
    fn appdata_is_ignored_off_windows_and_the_unix_list_is_unchanged() {
        let vars = env(&[("APPDATA", "/ignored"), ("HOME", "/h")]);
        assert_eq!(
            user_config_candidates(&vars, false),
            vec![
                PathBuf::from("/h/.config/agent-hub/config.toml"),
                PathBuf::from("/h/.agent-hub/config.toml"),
            ]
        );
        let xdg = env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(
            user_config_candidates(&xdg, false),
            vec![
                PathBuf::from("/x/agent-hub/config.toml"),
                PathBuf::from("/h/.agent-hub/config.toml"),
            ]
        );
    }

    #[test]
    fn a_windows_user_with_only_a_profile_gets_the_home_candidates() {
        let vars = env(&[("USERPROFILE", "C:\\Users\\u")]);
        let found = user_config_candidates(&vars, true);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[1].ends_with(".agent-hub/config.toml")
                || found[1].ends_with(".agent-hub\\config.toml")
        );
    }
}
