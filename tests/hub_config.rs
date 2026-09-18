//! Resolving the settings that name the hub a client talks to.
//!
//! The file and the environment carry the same keys, so the tests cover both
//! sources, their precedence, and the shapes a hand-written file takes. The
//! process environment is never touched: resolution takes its lookup as an
//! argument.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::config::{ClientConfig, Config};

/// A temp directory holding one config file, removed when the test ends.
struct TempHome(PathBuf);

impl TempHome {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-config-{}-{nanos}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join(".agent-hub")).expect("create temp home");
        Self(path)
    }

    /// Write the default config file and return the home directory.
    fn with_config(self, contents: &str) -> Self {
        std::fs::write(self.0.join(".agent-hub").join("config"), contents).expect("write config");
        self
    }

    fn path(&self) -> PathBuf {
        self.0.join(".agent-hub").join("config")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An environment lookup over a fixed set of pairs.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

#[test]
fn the_file_supplies_every_key_when_the_environment_is_empty() {
    let home = TempHome::new("file-only").with_config(
        "HUB_URL=http://hub.lan:8080\nHUB_TOKEN=file-token\nHUB_AGENT_ID=agent/laptop\n",
    );
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config.url.as_deref(), Some("http://hub.lan:8080"));
    assert_eq!(config.token.as_deref(), Some("file-token"));
    assert_eq!(config.agent_id.as_deref(), Some("agent/laptop"));
}

#[test]
fn the_environment_supplies_every_key_with_no_file() {
    let home = TempHome::new("env-only");
    let lookup = env(&[
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_URL", "http://env.lan:8080"),
        ("HUB_TOKEN", "env-token"),
        ("HUB_AGENT_ID", "agent/env"),
    ]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config.url.as_deref(), Some("http://env.lan:8080"));
    assert_eq!(config.token.as_deref(), Some("env-token"));
    assert_eq!(config.agent_id.as_deref(), Some("agent/env"));
}

#[test]
fn the_environment_wins_over_the_file_key_by_key() {
    let home =
        TempHome::new("both").with_config("HUB_URL=http://file.lan:8080\nHUB_TOKEN=file-token\n");
    let lookup = env(&[
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_TOKEN", "env-token"),
    ]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    // Each key resolves on its own, so an override of one keeps the others.
    assert_eq!(config.url.as_deref(), Some("http://file.lan:8080"));
    assert_eq!(config.token.as_deref(), Some("env-token"));
    assert_eq!(config.agent_id, None);
}

#[test]
fn comments_blank_lines_and_quotes_are_read_the_way_a_shell_reads_them() {
    let home = TempHome::new("shapes").with_config(concat!(
        "# the hub on the landing\n",
        "\n",
        "  HUB_URL = \"http://hub.lan:8080\"  \n",
        "HUB_TOKEN='quoted token'\n",
        "  # trailing comment line\n",
    ));
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config.url.as_deref(), Some("http://hub.lan:8080"));
    assert_eq!(config.token.as_deref(), Some("quoted token"));
}

#[test]
fn an_unknown_key_is_ignored_rather_than_refused() {
    let home = TempHome::new("unknown")
        .with_config("HUB_FUTURE_SETTING=whatever\nHUB_URL=http://hub.lan:8080\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config.url.as_deref(), Some("http://hub.lan:8080"));
}

#[test]
fn a_missing_file_is_not_an_error() {
    let home = TempHome::new("absent");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = ClientConfig::resolve(&lookup).expect("a missing file resolves to nothing set");

    assert_eq!(config, ClientConfig::default());
}

#[test]
fn hub_config_names_another_file() {
    let home = TempHome::new("hub-config").with_config("HUB_URL=http://default.lan:8080\n");
    let other = home.0.join("other-config");
    std::fs::write(&other, "HUB_URL=http://other.lan:8080\n").expect("write other config");
    let lookup = env(&[
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_CONFIG", other.to_str().expect("utf-8 path")),
    ]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config.url.as_deref(), Some("http://other.lan:8080"));
}

#[test]
fn a_line_that_is_not_a_key_value_pair_names_its_line_number() {
    let home = TempHome::new("bad-line")
        .with_config("HUB_URL=http://hub.lan:8080\nthis is not a setting\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let err = ClientConfig::resolve(&lookup).expect_err("an unparseable line is refused");
    let message = err.to_string();

    assert!(
        message.contains("line 2"),
        "the message names the line to fix, got {message:?}"
    );
    assert!(
        message.contains(&home.path().display().to_string()),
        "the message names the file to fix, got {message:?}"
    );
}

#[test]
fn no_home_and_no_hub_config_resolves_to_nothing_set() {
    let lookup = env(&[]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config, ClientConfig::default());
}

#[cfg(unix)]
#[test]
fn a_file_others_can_read_warns_when_it_holds_a_token() {
    let path = Path::new(".agent-hub/config");

    let warning = agent_hub::config::config_permissions_warning(path, 0o644, true)
        .expect("a group and world readable token file warns");

    assert!(
        warning.contains("chmod 600"),
        "the warning says how to fix it, got {warning:?}"
    );
    assert!(
        warning.contains("config"),
        "the warning names the file, got {warning:?}"
    );
}

#[cfg(unix)]
#[test]
fn an_owner_only_file_and_a_tokenless_file_do_not_warn() {
    let path = Path::new(".agent-hub/config");

    assert_eq!(
        agent_hub::config::config_permissions_warning(path, 0o600, true),
        None,
        "an owner-only token file is what the warning asks for"
    );
    assert_eq!(
        agent_hub::config::config_permissions_warning(path, 0o644, false),
        None,
        "a file with no token carries no secret to leak"
    );
}

#[test]
fn the_call_limit_defaults_and_can_be_set() {
    let config = ClientConfig::resolve(&env(&[])).expect("resolve");
    assert_eq!(config.timeout, std::time::Duration::from_secs(120));

    let config = ClientConfig::resolve(&env(&[("HUB_TIMEOUT", "2.5")])).expect("resolve");
    assert_eq!(config.timeout, std::time::Duration::from_millis(2500));

    for bad in ["0", "-1", "soon", "inf"] {
        assert!(
            ClientConfig::resolve(&env(&[("HUB_TIMEOUT", bad)])).is_err(),
            "{bad} should be refused"
        );
    }
}

#[test]
fn the_active_window_defaults_to_fifteen_minutes() {
    for unset in [None, Some(""), Some("  ")] {
        assert_eq!(
            Config::parse_active_window(unset).expect("parse"),
            std::time::Duration::from_secs(900),
            "{unset:?}"
        );
    }
}

#[test]
fn the_active_window_is_read_from_the_setting() {
    assert_eq!(
        Config::parse_active_window(Some("60")).expect("parse"),
        std::time::Duration::from_secs(60)
    );
}

#[test]
fn an_active_window_that_counts_nobody_is_rejected() {
    for value in ["0", "-5", "fifteen", "90.5"] {
        let err = Config::parse_active_window(Some(value)).expect_err("rejected");
        assert!(
            err.to_string().contains("HUB_ACTIVE_WINDOW_SECS"),
            "the error names the variable: {err}"
        );
        assert!(matches!(err, agent_hub::error::Error::Config(_)), "{value}");
    }
}

#[test]
fn an_active_window_no_clock_can_subtract_is_rejected() {
    for value in ["2592001", &u64::MAX.to_string()] {
        let err = Config::parse_active_window(Some(value)).expect_err("rejected");
        assert!(
            err.to_string().contains("HUB_ACTIVE_WINDOW_SECS"),
            "the error names the variable: {err}"
        );
        assert!(matches!(err, agent_hub::error::Error::Config(_)), "{value}");
    }
    assert_eq!(
        Config::parse_active_window(Some("2592000")).expect("parse"),
        std::time::Duration::from_secs(2_592_000),
        "the ceiling itself is accepted"
    );
}
