//! Resolving the settings that name the hub a client talks to.
//!
//! The file and the environment carry the same keys, so the tests cover both
//! sources, their precedence, and the shapes a TOML config file takes. The
//! process environment is never touched directly: resolution takes its lookup
//! as an argument.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use agent_hub::config::{ClientConfig, Config};

mod common;

use common::temp::TempDir;

/// A temp directory holding a user config file, removed when the test ends.
struct TempHome(TempDir);

impl TempHome {
    fn new(tag: &str) -> Self {
        let home = TempDir::new(&format!("config-{tag}"));
        std::fs::create_dir_all(home.join(".config").join("agent-hub")).expect("create temp home");
        Self(home)
    }

    /// Write the default user config.toml and return the home directory.
    fn with_config(self, contents: &str) -> Self {
        std::fs::write(
            self.0.join(".config").join("agent-hub").join("config.toml"),
            contents,
        )
        .expect("write config");
        self
    }

    /// Write a legacy env-style config file at ~/.agent-hub/config.
    fn with_legacy_config(self, contents: &str) -> Self {
        std::fs::create_dir_all(self.0.join(".agent-hub")).expect("create .agent-hub");
        std::fs::write(self.0.join(".agent-hub").join("config"), contents)
            .expect("write legacy config");
        self
    }

    fn path(&self) -> PathBuf {
        self.0.join(".config").join("agent-hub").join("config.toml")
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
fn a_key_set_only_in_the_system_file_survives_a_user_file_setting_a_different_key() {
    let system_dir = TempDir::new("system-config");
    let system_file = system_dir.join("config.toml");
    std::fs::write(&system_file, "[hub]\ndata_dir = \"/custom/data\"\n").expect("write system");

    let home = TempDir::new("user-home");
    let user_config_dir = home.join(".config").join("agent-hub");
    std::fs::create_dir_all(&user_config_dir).expect("create user dir");
    std::fs::write(
        user_config_dir.join("config.toml"),
        "[hub]\nbind = \"127.0.0.1:9090\"\n",
    )
    .expect("write user");

    let lookup = env(&[
        (
            "AGENT_HUB_SYSTEM_CONFIG",
            system_file.to_str().expect("utf-8 path"),
        ),
        ("HOME", home.to_str().expect("utf-8 path")),
    ]);

    let config = Config::resolve(&lookup).expect("resolve");
    assert_eq!(config.data_dir, PathBuf::from("/custom/data"));
    assert_eq!(config.bind, "127.0.0.1:9090".parse().expect("addr"));
}

#[test]
fn user_file_overrides_system_file_key_by_key() {
    let system_dir = TempDir::new("system-override");
    let system_file = system_dir.join("config.toml");
    std::fs::write(
        &system_file,
        "[hub]\nbind = \"127.0.0.1:8000\"\ndata_dir = \"/system/data\"\n",
    )
    .expect("write system");

    let home = TempHome::new("override").with_config("[hub]\nbind = \"127.0.0.1:9090\"\n");
    let lookup = env(&[
        (
            "AGENT_HUB_SYSTEM_CONFIG",
            system_file.to_str().expect("utf-8 path"),
        ),
        ("HOME", home.0.to_str().expect("utf-8 path")),
    ]);

    let config = Config::resolve(&lookup).expect("resolve");
    assert_eq!(
        config.bind,
        "127.0.0.1:9090".parse().expect("addr"),
        "user file overrides bind"
    );
    assert_eq!(
        config.data_dir,
        PathBuf::from("/system/data"),
        "system file data_dir survives"
    );
}

#[test]
fn environment_beats_user_and_system_file_for_every_key() {
    let system_dir = TempDir::new("system-env");
    let system_file = system_dir.join("config.toml");
    std::fs::write(
        &system_file,
        "[hub]\nbind = \"127.0.0.1:8000\"\ndata_dir = \"/system/data\"\n",
    )
    .expect("write system");

    let home = TempHome::new("env-beats").with_config("[hub]\nbind = \"127.0.0.1:9000\"\n");
    let lookup = env(&[
        (
            "AGENT_HUB_SYSTEM_CONFIG",
            system_file.to_str().expect("utf-8 path"),
        ),
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_BIND", "127.0.0.1:9999"),
    ]);

    let config = Config::resolve(&lookup).expect("resolve");
    assert_eq!(
        config.bind,
        "127.0.0.1:9999".parse().expect("addr"),
        "environment beats both files"
    );
    assert_eq!(
        config.data_dir,
        PathBuf::from("/system/data"),
        "system data_dir survives"
    );
}

#[test]
fn empty_environment_variable_does_not_shadow_file_value() {
    let home = TempHome::new("empty-env").with_config("[client]\nurl = \"http://file.lan:8080\"\n");
    let lookup = env(&[
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_URL", ""),
    ]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");
    assert_eq!(config.url.as_deref(), Some("http://file.lan:8080"));
}

#[test]
fn empty_toml_value_does_not_shadow_system_value() {
    let system_dir = TempDir::new("empty-toml-sys");
    let system_file = system_dir.join("config.toml");
    std::fs::write(&system_file, "[hub]\ndata_dir = \"/system/data\"\n").expect("write system");

    let home = TempHome::new("empty-toml-user").with_config("[hub]\ndata_dir = \"\"\n");
    let lookup = env(&[
        (
            "AGENT_HUB_SYSTEM_CONFIG",
            system_file.to_str().expect("utf-8 path"),
        ),
        ("HOME", home.0.to_str().expect("utf-8 path")),
    ]);

    let config = Config::resolve(&lookup).expect("resolve");
    assert_eq!(
        config.data_dir,
        PathBuf::from("/system/data"),
        "empty user value does not shadow system value"
    );
}

#[test]
fn hub_config_replaces_both_levels() {
    let system_dir = TempDir::new("hub-cfg-sys");
    let system_file = system_dir.join("config.toml");
    std::fs::write(&system_file, "[hub]\ndata_dir = \"/system/data\"\n").expect("write system");

    let home = TempHome::new("hub-cfg-user").with_config("[hub]\nnode_name = \"user-node\"\n");

    let custom_dir = TempDir::new("hub-cfg-custom");
    let custom_file = custom_dir.join("custom.toml");
    std::fs::write(
        &custom_file,
        "[hub]\nbind = \"127.0.0.1:7777\"\nnode_name = \"custom-node\"\n",
    )
    .expect("write custom");

    let lookup = env(&[
        (
            "AGENT_HUB_SYSTEM_CONFIG",
            system_file.to_str().expect("utf-8 path"),
        ),
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_CONFIG", custom_file.to_str().expect("utf-8 path")),
    ]);

    let config = Config::resolve(&lookup).expect("resolve");
    assert_eq!(config.bind, "127.0.0.1:7777".parse().expect("addr"));
    assert_eq!(config.node_name.as_deref(), Some("custom-node"));
    assert_eq!(
        config.data_dir,
        PathBuf::from("./data"),
        "system file data_dir is not read"
    );
}

#[test]
fn the_enrolment_and_proxy_keys_are_read_from_the_file() {
    let home = TempHome::new("hub-enrol-keys").with_config(concat!(
        "[hub]\n",
        "trust_proxy = \"127.0.0.1,10.0.0.5\"\n",
        "enrol_pending_max = 30\n",
        "enrol_pending_ttl_secs = 3600\n",
        "enrol = \"off\"\n",
    ));
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = Config::resolve(&lookup).expect("the new hub keys are not unknown");
    assert_eq!(
        config.trusted_proxies,
        vec![
            "127.0.0.1".parse::<std::net::IpAddr>().expect("ip"),
            "10.0.0.5".parse::<std::net::IpAddr>().expect("ip"),
        ]
    );
    assert_eq!(config.enrol_pending_max, 30);
    assert_eq!(
        config.enrol_pending_ttl,
        std::time::Duration::from_secs(3600)
    );
    assert!(
        !config.enrol_enabled,
        "enrol = \"off\" in the file disables enrolment"
    );
}

#[test]
fn enrol_is_on_by_default_and_off_only_for_off() {
    let home = TempHome::new("hub-enrol-default");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);
    assert!(
        Config::resolve(&lookup).expect("resolve").enrol_enabled,
        "enrolment is on when the key is absent"
    );

    let home = TempHome::new("hub-enrol-on").with_config("[hub]\nenrol = \"on\"\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);
    assert!(
        Config::resolve(&lookup).expect("resolve").enrol_enabled,
        "enrol = \"on\" keeps enrolment on"
    );

    let home = TempHome::new("hub-enrol-env").with_config("[hub]\nenrol = \"off\"\n");
    let lookup = env(&[
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_ENROL", "on"),
    ]);
    assert!(
        Config::resolve(&lookup).expect("resolve").enrol_enabled,
        "the environment beats the file"
    );
}

#[test]
fn xdg_user_file_shadows_fallback_user_file() {
    let home = TempDir::new("shadow-home");
    let xdg_dir = home.join(".config").join("agent-hub");
    std::fs::create_dir_all(&xdg_dir).expect("create xdg dir");
    std::fs::write(
        xdg_dir.join("config.toml"),
        "[client]\nurl = \"http://xdg.lan:8080\"\n",
    )
    .expect("write xdg config");

    let fallback_dir = home.join(".agent-hub");
    std::fs::create_dir_all(&fallback_dir).expect("create fallback dir");
    std::fs::write(
        fallback_dir.join("config.toml"),
        "[client]\nurl = \"http://fallback.lan:8080\"\n",
    )
    .expect("write fallback config");

    let lookup = env(&[("HOME", home.to_str().expect("utf-8 path"))]);
    let config = ClientConfig::resolve(&lookup).expect("resolve");
    assert_eq!(config.url.as_deref(), Some("http://xdg.lan:8080"));
}

#[test]
fn fallback_user_file_is_read_when_no_xdg_file_exists() {
    let home = TempDir::new("fallback-home");
    let fallback_dir = home.join(".agent-hub");
    std::fs::create_dir_all(&fallback_dir).expect("create fallback dir");
    std::fs::write(
        fallback_dir.join("config.toml"),
        "[client]\nurl = \"http://fallback.lan:8080\"\n",
    )
    .expect("write fallback config");

    let lookup = env(&[("HOME", home.to_str().expect("utf-8 path"))]);
    let config = ClientConfig::resolve(&lookup).expect("resolve");
    assert_eq!(config.url.as_deref(), Some("http://fallback.lan:8080"));
}

#[test]
fn a_client_key_under_hub_is_an_unknown_key_and_fails() {
    let home = TempHome::new("wrong-table").with_config("[hub]\nurl = \"http://hub.lan:8080\"\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let err = ClientConfig::resolve(&lookup).expect_err("a client key under [hub] is unknown");
    let message = err.to_string();
    assert!(
        message.contains("unknown key 'url'"),
        "names the key: {message}"
    );
    assert!(message.contains("[hub]"), "names the table: {message}");
    assert!(matches!(err, agent_hub::error::Error::Config(_)), "{err}");
}

#[test]
fn an_unknown_client_key_fails() {
    let home = TempHome::new("unknown-client").with_config("[client]\nnot_a_key = \"x\"\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let err = ClientConfig::resolve(&lookup).expect_err("an unknown [client] key is an error");
    let message = err.to_string();
    assert!(
        message.contains("unknown key 'not_a_key'"),
        "names the key: {message}"
    );
    assert!(message.contains("[client]"), "names the table: {message}");
}

#[test]
fn unknown_table_warns_and_continues() {
    let home = TempHome::new("unknown-table").with_config(concat!(
        "[custom_section]\n",
        "some_setting = true\n\n",
        "[client]\n",
        "url = \"http://hub.lan:8080\"\n"
    ));
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");
    assert_eq!(config.url.as_deref(), Some("http://hub.lan:8080"));
}

#[test]
fn unparseable_toml_fails_startup_naming_file_and_line() {
    let home = TempHome::new("bad-toml").with_config("[client\nurl = 123\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let err = ClientConfig::resolve(&lookup).expect_err("bad TOML must fail");
    let message = err.to_string();
    assert!(
        message.contains(&home.path().display().to_string()),
        "names the file: {message}"
    );
    assert!(message.contains("line"), "names the line number: {message}");
}

#[test]
fn the_file_supplies_every_key_when_the_environment_is_empty() {
    let home = TempHome::new("file-only").with_config(concat!(
        "[client]\n",
        "url = \"http://hub.lan:8080\"\n",
        "token = \"file-token\"\n",
        "agent_id = \"agent/laptop\"\n",
    ));
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
    let home = TempHome::new("both").with_config(concat!(
        "[client]\n",
        "url = \"http://file.lan:8080\"\n",
        "token = \"file-token\"\n",
    ));
    let lookup = env(&[
        ("HOME", home.0.to_str().expect("utf-8 path")),
        ("HUB_TOKEN", "env-token"),
    ]);

    let config = ClientConfig::resolve(&lookup).expect("resolve");

    assert_eq!(config.url.as_deref(), Some("http://file.lan:8080"));
    assert_eq!(config.token.as_deref(), Some("env-token"));
    assert_eq!(config.agent_id, None);
}

#[test]
fn a_missing_file_is_not_an_error() {
    let home = TempHome::new("absent");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    let config = ClientConfig::resolve(&lookup).expect("a missing file resolves to nothing set");

    assert_eq!(config, ClientConfig::default());
}

#[test]
fn migration_creates_config_toml_once_with_mode_0600_and_is_noop_on_second_run() {
    let home = TempHome::new("migrate").with_legacy_config(concat!(
        "# old agent config\n",
        "HUB_URL=http://legacy.lan:8080\n",
        "HUB_TOKEN=\"legacy-token-secret-1234567890\"\n",
        "HUB_AGENT_ID=legacy-agent\n",
        "HUB_TIMEOUT=45\n",
    ));

    let migrated_file = home.0.join(".agent-hub").join("config.toml");
    assert!(
        !migrated_file.exists(),
        "target does not exist before first run"
    );

    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);

    // First run triggers migration:
    let config = ClientConfig::resolve(&lookup).expect("resolve triggers migration");
    assert_eq!(config.url.as_deref(), Some("http://legacy.lan:8080"));
    assert_eq!(
        config.token.as_deref(),
        Some("legacy-token-secret-1234567890")
    );
    assert_eq!(config.agent_id.as_deref(), Some("legacy-agent"));
    assert_eq!(config.timeout, std::time::Duration::from_secs(45));

    assert!(migrated_file.is_file(), "config.toml was created");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&migrated_file)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "migrated file has mode 0600");
    }

    // Old file was preserved:
    let old_file = home.0.join(".agent-hub").join("config");
    assert!(old_file.is_file(), "old file was not deleted");

    // Second run: no-op, reads config.toml directly:
    let config2 = ClientConfig::resolve(&lookup).expect("second run succeeds");
    assert_eq!(config2.url.as_deref(), Some("http://legacy.lan:8080"));
}

#[test]
fn migration_routes_the_enrolment_and_proxy_keys_to_the_hub_table() {
    let home = TempHome::new("migrate-hub-keys").with_legacy_config(concat!(
        "HUB_URL=http://legacy.lan:8080\n",
        "HUB_ENROL=off\n",
        "HUB_ENROL_PENDING_MAX=30\n",
        "HUB_ENROL_PENDING_TTL_SECS=3600\n",
        "HUB_TRUST_PROXY=127.0.0.1\n",
    ));

    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);
    // Parsing the migrated file is what proves the shapes are loadable, so the
    // resolve has to succeed, not merely the migration.
    let config = ClientConfig::resolve(&lookup).expect("resolve triggers migration");
    assert_eq!(config.url.as_deref(), Some("http://legacy.lan:8080"));

    let migrated = std::fs::read_to_string(home.0.join(".agent-hub").join("config.toml"))
        .expect("migrated file");
    let hub_start = migrated.find("[hub]").expect("[hub] table in the output");
    let hub = &migrated[hub_start..];

    assert!(hub.contains("enrol = \"off\""), "{migrated}");
    assert!(hub.contains("enrol_pending_max = 30\n"), "{migrated}");
    assert!(
        hub.contains("enrol_pending_ttl_secs = 3600\n"),
        "{migrated}"
    );
    assert!(hub.contains("trust_proxy = \"127.0.0.1\""), "{migrated}");
    assert!(
        !migrated[..hub_start].contains("enrol"),
        "an enrolment key does not leak into [client]: {migrated}"
    );
}

#[test]
fn config_check_fails_on_an_unknown_key_naming_it() {
    let home = TempHome::new("cli-check-unknown").with_config("[hub]\nnot_a_hub_key = \"x\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["config", "--check"])
        .env("HOME", home.0.to_str().expect("utf-8 path"))
        .env("XDG_CONFIG_HOME", home.0.join(".config"))
        .env("AGENT_HUB_SYSTEM_CONFIG", home.0.join("absent-system.toml"))
        .output()
        .expect("run agent-hub config --check");

    assert_eq!(output.status.code(), Some(78), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown key 'not_a_hub_key'"),
        "names the key: {stderr}"
    );
    assert!(stderr.contains("[hub]"), "names the table: {stderr}");
}

#[test]
fn config_command_prints_both_spellings_and_masks_token() {
    let home = TempHome::new("cli-config").with_config(concat!(
        "[client]\n",
        "url = \"http://hub.lan:8080\"\n",
        "token = \"tok_1234567890abcdef123456789012\"\n",
        "[hub]\n",
        "bind = \"127.0.0.1:47318\"\n",
        "admin_token = \"adm_secret1234567890abcdef12345678\"\n"
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("config")
        .env("HOME", home.0.to_str().expect("utf-8 path"))
        .env("XDG_CONFIG_HOME", home.0.join(".config"))
        .env("AGENT_HUB_SYSTEM_CONFIG", home.0.join("absent-system.toml"))
        .output()
        .expect("run agent-hub config");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Both spellings on each line:
    assert!(
        stdout.contains("[hub] bind") && stdout.contains("HUB_BIND"),
        "prints both spellings for bind: {stdout}"
    );
    assert!(
        stdout.contains("[client] url") && stdout.contains("HUB_URL"),
        "prints both spellings for url: {stdout}"
    );

    // Token masking:
    assert!(
        stdout.contains("tok_... (32 chars)"),
        "client token is masked: {stdout}"
    );
    assert!(
        stdout.contains("adm_... (34 chars)"),
        "admin token is masked: {stdout}"
    );

    // Secret token is never printed in full:
    assert!(
        !stdout.contains("tok_1234567890abcdef123456789012"),
        "token never printed in full: {stdout}"
    );
    assert!(
        !stdout.contains("adm_secret1234567890abcdef12345678"),
        "admin token never printed in full: {stdout}"
    );
}

#[test]
fn config_path_command_reports_file_states() {
    let home = TempHome::new("cli-path").with_config("[client]\nurl = \"http://hub.lan:8080\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["config", "--path"])
        .env("HOME", home.0.to_str().expect("utf-8 path"))
        .env("XDG_CONFIG_HOME", home.0.join(".config"))
        .env("AGENT_HUB_SYSTEM_CONFIG", home.0.join("absent-system.toml"))
        .output()
        .expect("run agent-hub config --path");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("system"), "lists system level: {stdout}");
    assert!(stdout.contains("user"), "lists user level: {stdout}");
    assert!(stdout.contains("found"), "reports found state: {stdout}");
}

#[test]
fn config_check_command_validates_configuration() {
    let home =
        TempHome::new("cli-check-ok").with_config("[client]\nurl = \"http://hub.lan:8080\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["config", "--check"])
        .env("HOME", home.0.to_str().expect("utf-8 path"))
        .env("XDG_CONFIG_HOME", home.0.join(".config"))
        .env("AGENT_HUB_SYSTEM_CONFIG", home.0.join("absent-system.toml"))
        .output()
        .expect("run agent-hub config --check");

    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // Invalid config should fail with non-zero exit code:
    let bad_home =
        TempHome::new("cli-check-bad").with_config("[client]\ntimeout = \"not-a-number\"\n");
    let bad_output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["config", "--check"])
        .env("HOME", bad_home.0.to_str().expect("utf-8 path"))
        .env("XDG_CONFIG_HOME", bad_home.0.join(".config"))
        .env(
            "AGENT_HUB_SYSTEM_CONFIG",
            bad_home.0.join("absent-system.toml"),
        )
        .output()
        .expect("run agent-hub config --check");

    assert_eq!(bad_output.status.code(), Some(78), "{bad_output:?}");
}

#[cfg(unix)]
#[test]
fn a_file_others_can_read_warns_when_it_holds_a_token() {
    let path = Path::new(".agent-hub/config.toml");

    let warning = agent_hub::config::config_permissions_warning(path, 0o644, true)
        .expect("a group and world readable token file warns");

    assert!(
        warning.contains("chmod 600"),
        "the warning says how to fix it, got {warning:?}"
    );
    assert!(
        warning.contains("config.toml"),
        "the warning names the file, got {warning:?}"
    );
}

#[cfg(unix)]
#[test]
fn an_owner_only_file_and_a_tokenless_file_do_not_warn() {
    let path = Path::new(".agent-hub/config.toml");

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

#[test]
fn the_event_ceiling_defaults_and_is_read_from_env_and_file() {
    let home = TempHome::new("events-ceiling-default");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);
    assert_eq!(
        Config::resolve(&lookup)
            .expect("resolve")
            .events_per_project
            .per_project,
        1_000_000,
        "the default ceiling"
    );

    let home = TempHome::new("events-ceiling-file").with_config("[hub]\nevents_per_project = 25\n");
    let lookup = env(&[("HOME", home.0.to_str().expect("utf-8 path"))]);
    assert_eq!(
        Config::resolve(&lookup)
            .expect("resolve")
            .events_per_project
            .per_project,
        25,
        "the file sets the ceiling"
    );

    let lookup = env(&[("HUB_EVENTS_PER_PROJECT", "0")]);
    assert_eq!(
        Config::resolve(&lookup)
            .expect("resolve")
            .events_per_project
            .per_project,
        0,
        "zero disables the ceiling"
    );
}

#[test]
fn a_negative_or_unparseable_event_ceiling_is_refused() {
    for value in ["-1", "many", "1.5"] {
        let err = Config::resolve(&env(&[("HUB_EVENTS_PER_PROJECT", value)])).expect_err("refused");
        assert!(
            err.to_string().contains("HUB_EVENTS_PER_PROJECT"),
            "names the setting: {err}"
        );
        assert!(matches!(err, agent_hub::error::Error::Config(_)), "{value}");
    }
}
