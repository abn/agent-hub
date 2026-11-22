//! `agent-hub setup antigravity` installs the plugin under a fake home and
//! never touches anything else there.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

mod common;

use common::temp::TempDir;

fn setup(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("setup")
        .args(args)
        .env("HOME", home)
        .env("RUST_LOG", "error")
        .output()
        .expect("run agent-hub setup")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read file")).expect("parse json")
}

fn plugin_dir(home: &Path) -> std::path::PathBuf {
    home.join(".gemini/config/plugins/agent-hub")
}

/// The one timestamped directory a run's backups land in.
fn backup_run(root: &Path) -> std::path::PathBuf {
    let runs: Vec<_> = fs::read_dir(root)
        .expect("backup root exists")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
    runs.into_iter().next().unwrap()
}

#[test]
fn installs_the_plugin_enables_it_and_asks_for_a_restart() {
    let home = TempDir::new("setup-agy-fresh");
    let out = setup(&home, &["antigravity"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Restart Antigravity"), "{stdout}");

    let dir = plugin_dir(&home);
    assert_eq!(read_json(&dir.join("plugin.json"))["name"], "agent-hub");
    let mcp = read_json(&dir.join("mcp_config.json"));
    assert_eq!(
        mcp["mcpServers"]["agent-hub"]["args"],
        json!(["mcp", "--proxy"])
    );
    assert_eq!(
        mcp["mcpServers"]["agent-hub"]["env"]["HUB_AGENT_ID"],
        "antigravity"
    );
    let hooks = read_json(&dir.join("hooks.json"));
    assert_eq!(
        hooks["agent-hub"]["PreInvocation"][0]["command"],
        "agent-hub hook prompt --harness antigravity"
    );
    assert!(
        fs::read_to_string(dir.join("rules/agent-hub.md"))
            .unwrap()
            .contains("trigger: always_on")
    );
    assert!(dir.join("skills/agent-hub/SKILL.md").is_file());
    assert!(dir.join("skills/agent-hub/references/tools.md").is_file());
    assert!(fs::metadata(dir.join("logo.png")).unwrap().len() > 0);

    let config = read_json(&home.join(".gemini/config/config.json"));
    assert_eq!(config["plugins"]["agent-hub"]["enabled"], true);
}

#[test]
fn a_second_run_changes_nothing_and_makes_no_backup() {
    let home = TempDir::new("setup-agy-idem");
    assert!(setup(&home, &["antigravity"]).status.success());
    let config_path = home.join(".gemini/config/config.json");
    let before_config = fs::read(&config_path).unwrap();
    let before_mtime = fs::metadata(plugin_dir(&home).join("plugin.json"))
        .unwrap()
        .modified()
        .unwrap();

    let out = setup(&home, &["antigravity"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(fs::read(&config_path).unwrap(), before_config);
    assert_eq!(
        fs::metadata(plugin_dir(&home).join("plugin.json"))
            .unwrap()
            .modified()
            .unwrap(),
        before_mtime
    );
    assert!(!home.join(".gemini/config/agent-hub-backup").exists());
}

#[test]
fn enabling_keeps_unrelated_keys_and_backs_up_the_config() {
    let home = TempDir::new("setup-agy-keys");
    let config_dir = home.join(".gemini/config");
    fs::create_dir_all(&config_dir).unwrap();
    let original = json!({
        "enableAdc": true,
        "plugins": {"other": {"enabled": false}},
        "sidecars": {"x/y": {"enabled": true}}
    });
    fs::write(config_dir.join("config.json"), original.to_string()).unwrap();

    assert!(setup(&home, &["antigravity"]).status.success());

    let config = read_json(&config_dir.join("config.json"));
    assert_eq!(config["enableAdc"], true);
    assert_eq!(config["plugins"]["other"]["enabled"], false);
    assert_eq!(config["sidecars"]["x/y"]["enabled"], true);
    assert_eq!(config["plugins"]["agent-hub"]["enabled"], true);
    assert_eq!(
        read_json(&backup_run(&config_dir.join("agent-hub-backup")).join("config.json")),
        original
    );
}

#[test]
fn a_changed_plugin_file_is_restored_and_the_old_copy_kept() {
    let home = TempDir::new("setup-agy-restore");
    assert!(setup(&home, &["antigravity"]).status.success());
    let rules = plugin_dir(&home).join("rules/agent-hub.md");
    let shipped = fs::read(&rules).unwrap();
    fs::write(&rules, "edited by hand\n").unwrap();

    assert!(setup(&home, &["antigravity"]).status.success());

    assert_eq!(fs::read(&rules).unwrap(), shipped);
    assert_eq!(
        fs::read_to_string(
            backup_run(&home.join(".gemini/config/agent-hub-backup")).join("rules/agent-hub.md")
        )
        .unwrap(),
        "edited by hand\n"
    );
}

#[test]
fn a_workspace_install_stays_in_the_workspace() {
    let home = TempDir::new("setup-agy-ws-home");
    let workspace = TempDir::new("setup-agy-ws");
    let out = setup(
        &home,
        &["antigravity", "--workspace", workspace.to_str().unwrap()],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        workspace
            .join(".agents/plugins/agent-hub/plugin.json")
            .is_file()
    );
    assert!(!plugin_dir(&home).exists());
}

#[test]
fn a_workspace_install_backs_the_global_config_up_under_the_home_config() {
    let home = TempDir::new("setup-agy-wsb-home");
    let workspace = TempDir::new("setup-agy-wsb");
    let config_dir = home.join(".gemini/config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.json"), r#"{"theme":"dark"}"#).unwrap();
    let out = setup(
        &home,
        &["antigravity", "--workspace", workspace.to_str().unwrap()],
    );
    assert!(out.status.success(), "{out:?}");
    let saved = backup_run(&config_dir.join("agent-hub-backup")).join("config.json");
    assert_eq!(read_json(&saved)["theme"], "dark");
    assert!(!workspace.join(".agents/agent-hub-backup").exists());
}

#[test]
fn a_planted_symlink_at_the_old_temp_name_is_never_followed() {
    let home = TempDir::new("setup-agy-tmp");
    let outside = TempDir::new("setup-agy-tmp-out");
    let victim = outside.join("victim");
    fs::write(&victim, "keep").unwrap();
    let config_dir = home.join(".gemini/config");
    fs::create_dir_all(&config_dir).unwrap();
    std::os::unix::fs::symlink(&victim, config_dir.join(".config.json.agent-hub-tmp")).unwrap();
    let out = setup(&home, &["antigravity"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert_eq!(
        read_json(&config_dir.join("config.json"))["plugins"]["agent-hub"]["enabled"],
        true
    );
}

#[test]
fn a_config_that_is_not_json_is_left_alone() {
    let home = TempDir::new("setup-agy-bad");
    let config_dir = home.join(".gemini/config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.json"), "{ not json").unwrap();

    let out = setup(&home, &["antigravity"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(
        fs::read_to_string(config_dir.join("config.json")).unwrap(),
        "{ not json"
    );
}

#[test]
fn a_symlinked_plugin_directory_is_not_written_through() {
    let home = TempDir::new("setup-agy-link");
    let source = TempDir::new("setup-agy-link-src");
    let plugins = home.join(".gemini/config/plugins");
    fs::create_dir_all(&plugins).unwrap();
    std::os::unix::fs::symlink(&*source, plugins.join("agent-hub")).unwrap();

    let out = setup(&home, &["antigravity"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(fs::read_dir(&*source).unwrap().count(), 0);
}

#[test]
fn a_symlinked_agents_directory_in_the_workspace_is_refused() {
    let home = TempDir::new("setup-agy-agents-home");
    let workspace = TempDir::new("setup-agy-agents-ws");
    let outside = TempDir::new("setup-agy-agents-out");
    std::os::unix::fs::symlink(&*outside, workspace.join(".agents")).unwrap();

    let out = setup(
        &home,
        &["antigravity", "--workspace", workspace.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(fs::read_dir(&*outside).unwrap().count(), 0);
}

#[test]
fn a_target_that_links_to_a_file_outside_is_refused_and_left_unchanged() {
    let home = TempDir::new("setup-agy-file-link");
    let outside = TempDir::new("setup-agy-file-out");
    let secret = outside.join("secret.txt");
    fs::write(&secret, "do not touch\n").unwrap();
    let rules = plugin_dir(&home).join("rules");
    fs::create_dir_all(&rules).unwrap();
    std::os::unix::fs::symlink(&secret, rules.join("agent-hub.md")).unwrap();

    let out = setup(&home, &["antigravity"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(fs::read_to_string(&secret).unwrap(), "do not touch\n");
    assert!(!home.join(".gemini/config/agent-hub-backup").exists());
}

#[test]
fn usage_errors_exit_2() {
    let home = TempDir::new("setup-agy-usage");
    assert_eq!(setup(&home, &[]).status.code(), Some(2));
    assert_eq!(setup(&home, &["nowhere"]).status.code(), Some(2));
    assert_eq!(
        setup(&home, &["antigravity", "--workspace"]).status.code(),
        Some(2)
    );
}

#[test]
fn the_installed_plugin_passes_the_antigravity_validator() {
    if Command::new("agy").arg("--version").output().is_err() {
        eprintln!("agy not on PATH, skipping");
        return;
    }
    let home = TempDir::new("setup-agy-validate");
    assert!(setup(&home, &["antigravity"]).status.success());
    let out = Command::new("agy")
        .args(["plugin", "validate", "--strict"])
        .arg(plugin_dir(&home))
        .env("HOME", &*home)
        .output()
        .expect("run agy");
    assert!(out.status.success(), "{out:?}");
}
