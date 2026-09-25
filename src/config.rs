//! Process and client configuration, read from config files and the environment.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::limits::InboxCaps;

/// Keys the client table carries.
pub const CLIENT_KEYS: &[&str] = &["url", "token", "agent_id", "project", "timeout"];

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
    "tailnet",
    "tailnet_port",
    "tailnet_control_url",
];

/// The settings that name the hub a client reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// Base URL of a running hub, such as `http://hub.lan:8080`.
    pub url: Option<String>,
    /// Bearer token the hub resolves to an agent.
    pub token: Option<String>,
    /// Advisory agent label. The hub derives the actor from the token.
    pub agent_id: Option<String>,
    /// The project the knowledge-base shorthands act on, when no flag names one.
    pub project: Option<String>,
    /// How long one call may take, handshake to answer.
    pub timeout: std::time::Duration,
}

/// The default for `HUB_TIMEOUT`.
const CLIENT_TIMEOUT_SECS: f64 = 120.0;

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            url: None,
            token: None,
            agent_id: None,
            project: None,
            timeout: std::time::Duration::from_secs_f64(CLIENT_TIMEOUT_SECS),
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
        let loaded = load_layered_configs(env)?;
        let files: Vec<&ParsedConfigFile> = loaded.iter().collect();

        let url = Setting::resolved(env, "client", "url", &files, None).value;
        let token = Setting::resolved(env, "client", "token", &files, None).value;
        let agent_id = Setting::resolved(env, "client", "agent_id", &files, None).value;
        let project = Setting::resolved(env, "client", "project", &files, None).value;

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

        Ok(Self {
            url,
            token,
            agent_id,
            project,
            timeout: std::time::Duration::from_secs_f64(timeout),
        })
    }
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
    /// How long after its last tool call a session still counts its owner as
    /// an agent at work.
    pub active_window: std::time::Duration,
    /// The name the human sees for this node, when the operator set one.
    pub node_name: Option<String>,
    /// Whether agent self-enrolment is enabled.
    pub enrol_enabled: bool,
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

        let active_window_str =
            Setting::resolved(env, "hub", "active_window_secs", &files, Some("900")).value;
        let active_window = Self::parse_active_window(active_window_str.as_deref())?;

        let node_name = Setting::resolved(env, "hub", "node_name", &files, None)
            .value
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());

        let enrol_enabled = std::env::var("HUB_ENROL")
            .map(|value| value.trim() != "off")
            .unwrap_or(true);

        Ok(Self {
            data_dir,
            bind,
            public_url,
            admin_token,
            inbox_caps,
            active_window,
            node_name,
            enrol_enabled,
        })
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

/// A parsed TOML configuration file.
#[derive(Debug, Clone, Default)]
pub struct ParsedConfigFile {
    pub path: PathBuf,
    pub hub: HashMap<String, String>,
    pub client: HashMap<String, String>,
}

/// Load and parse a config file, warning on unknown keys/tables and mode.
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
                    eprintln!(
                        "agent-hub: {}: unknown key '{k}' under '[hub]'",
                        path.display()
                    );
                    continue;
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
                if !CLIENT_KEYS.contains(&k.as_str()) {
                    eprintln!(
                        "agent-hub: {}: unknown key '{k}' under '[client]'",
                        path.display()
                    );
                    continue;
                }
                let val_str = extract_value(path, "client", k, v)?;
                if let Some(val) = val_str {
                    parsed.client.insert(k.clone(), val);
                }
            }
        } else if value.is_table() {
            eprintln!("agent-hub: {}: unknown table '[{key}]'", path.display());
        } else {
            eprintln!("agent-hub: {}: unknown key '{key}'", path.display());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let has_token =
            parsed.client.contains_key("token") || parsed.hub.contains_key("admin_token");
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
            | "tailnet_port",
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

/// Resolve candidate paths for config files.
pub fn resolve_paths(
    env: &dyn Fn(&str) -> Option<String>,
) -> (Option<PathBuf>, PathBuf, Vec<PathBuf>) {
    let hub_config = present(env("HUB_CONFIG")).map(PathBuf::from);

    let system_path = present(env("AGENT_HUB_SYSTEM_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/agent-hub/config.toml"));

    let mut user_candidates = Vec::new();
    if let Some(xdg) = present(env("XDG_CONFIG_HOME")) {
        user_candidates.push(PathBuf::from(xdg).join("agent-hub").join("config.toml"));
    } else if let Some(home) = present(env("HOME")) {
        user_candidates.push(
            PathBuf::from(&home)
                .join(".config")
                .join("agent-hub")
                .join("config.toml"),
        );
    }
    if let Some(home) = present(env("HOME")) {
        user_candidates.push(PathBuf::from(home).join(".agent-hub").join("config.toml"));
    }

    (hub_config, system_path, user_candidates)
}

/// Check if migration from old env-style `~/.agent-hub/config` is needed.
pub fn check_and_perform_migration(
    env: &dyn Fn(&str) -> Option<String>,
    user_candidates: &[PathBuf],
) {
    let Some(home) = present(env("HOME")) else {
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
                    | "tailnet_port"
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
    let home = present(env("HOME"));

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
        ("client", "url", "HUB_URL", None, false),
        ("client", "token", "HUB_TOKEN", None, true),
        ("client", "agent_id", "HUB_AGENT_ID", None, false),
        ("client", "project", "HUB_PROJECT", None, false),
        ("client", "timeout", "HUB_TIMEOUT", Some("120"), false),
    ];

    let mut rows = Vec::new();
    for (table, key, env_var, default_val, is_secret) in defs {
        let setting = Setting::resolved(env, table, key, &files, default_val);
        let value_display = match setting.value {
            None => "(unset)".to_string(),
            Some(val) if is_secret => mask_token(&val),
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

/// Print the configuration files that would be read, found or not.
pub fn print_config_paths(env: &dyn Fn(&str) -> Option<String>) {
    let (hub_config, system_path, user_candidates) = resolve_paths(env);
    check_and_perform_migration(env, &user_candidates);

    let home = present(env("HOME"));
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

    Config::resolve(env)?;
    ClientConfig::resolve(env)?;

    Ok(())
}

/// The user config file a client writes its token to: the first candidate
/// `resolve_paths` names, so the file that is read is the file that is written.
pub fn user_config_target(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let (hub_config, _system, candidates) = resolve_paths(env);
    hub_config.or_else(|| candidates.into_iter().next())
}

/// Write or update `[client] token` in `path`, at file mode 0600.
///
/// The config file is the config module's to write: the enrolment command
/// stores the token it is given here, and nothing else opens the file for
/// writing.
pub fn write_client_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    let updated = update_client_toml(&existing, token);

    // Write beside the target at 0600 and rename it into place: the token is
    // then never in a file with broader permissions, and never half-written.
    // `mode` only applies when a file is created, so an existing temp file is
    // repaired explicitly, and a permission failure is an error rather than
    // something to ignore while the token sits readable. H12.
    let mut temp_name = path.as_os_str().to_owned();
    temp_name.push(format!(".tmp-{}", std::process::id()));
    let temp_path = PathBuf::from(temp_name);

    let write = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        {
            use std::io::Write;
            file.write_all(updated.as_bytes())?;
            file.sync_all()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp_path, path)
    })();

    if write.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write
}

/// Replace the token in the `[client]` table, or add the table and the key.
fn update_client_toml(content: &str, token: &str) -> String {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut client_section_idx = None;
    let mut next_section_idx = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section_name = trimmed[1..trimmed.len() - 1].trim();
            if section_name == "client" {
                client_section_idx = Some(i);
            } else if client_section_idx.is_some() && next_section_idx.is_none() {
                next_section_idx = Some(i);
            }
        }
    }

    if let Some(c_idx) = client_section_idx {
        let end_idx = next_section_idx.unwrap_or(lines.len());
        let mut token_line_idx = None;
        for (i, line) in lines.iter().enumerate().take(end_idx).skip(c_idx + 1) {
            let trimmed = line.trim();
            if trimmed.split_once('=').map(|(k, _)| k.trim()) == Some("token") {
                token_line_idx = Some(i);
                break;
            }
        }
        if let Some(t_idx) = token_line_idx {
            lines[t_idx] = format!("token = \"{token}\"");
        } else {
            lines.insert(c_idx + 1, format!("token = \"{token}\""));
        }
        let mut out = lines.join("\n");
        if content.ends_with('\n') {
            out.push('\n');
        }
        out
    } else {
        let mut out = content.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[client]\ntoken = \"{token}\"\n"));
        out
    }
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

#[cfg(test)]
mod token_tests {
    use super::update_client_toml;

    #[test]
    fn toml_insertion_on_empty() {
        let updated = update_client_toml("", "secret");
        assert_eq!(updated, "[client]\ntoken = \"secret\"\n");
    }

    #[test]
    fn toml_insertion_existing_client_no_token() {
        let existing = "[client]\nurl = \"http://hub:4000\"\n";
        let updated = update_client_toml(existing, "secret");
        assert_eq!(
            updated,
            "[client]\ntoken = \"secret\"\nurl = \"http://hub:4000\"\n"
        );
    }

    #[test]
    fn toml_replacement_existing_token() {
        let existing = "[client]\ntoken = \"old\"\n";
        let updated = update_client_toml(existing, "secret");
        assert_eq!(updated, "[client]\ntoken = \"secret\"\n");
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

    #[test]
    fn client_token_write_is_owner_only_and_leaves_no_temp() {
        use super::write_client_token;
        let dir = scratch("token");
        std::fs::create_dir_all(&dir).expect("create scratch");
        let file = dir.join("config.toml");
        std::fs::write(&file, "[client]\nurl = \"http://hub\"\n").expect("seed");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("loosen");

        write_client_token(&file, "secret-token").expect("write");
        assert_eq!(
            mode(&file),
            0o600,
            "an existing broad file is replaced at 0600"
        );
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(
            text.contains("token = \"secret-token\""),
            "token written: {text}"
        );
        assert!(
            text.contains("url = \"http://hub\""),
            "the rest of the file is kept: {text}"
        );

        let entries: Vec<_> = std::fs::read_dir(&dir).expect("list").collect();
        assert_eq!(
            entries.len(),
            1,
            "the temp file is renamed, not left behind"
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
