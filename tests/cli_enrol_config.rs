//! The enrolment client leaves the next command configured.
//!
//! A shared approval saves both the hub url and the token, so the command the
//! client prints as its next step runs with none of the hub environment set.

use std::path::Path;
use std::process::Command;

mod common;

use common::hub::{ADMIN_TOKEN, Hub};
use common::wire;

/// The built binary with a clean client environment: one `HOME`, and nothing
/// else that names a hub or a token.
///
/// This is the point of the test. The default config discovery has to find the
/// file enrolment wrote, with no `HUB_CONFIG` shortcut.
fn clean_client(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
    command
        .env("RUST_LOG", "error")
        .env("HOME", home)
        .env_remove("HUB_URL")
        .env_remove("HUB_TOKEN")
        .env_remove("HUB_CONFIG")
        .env_remove("HUB_AGENT_ID")
        .env_remove("HUB_PROJECT")
        .env_remove("XDG_CONFIG_HOME");
    command
}

/// The binary an operator first reaches a hub with: `HUB_URL` set, and no
/// token yet, which is the state enrolment exists to fix.
fn enrol_client(home: &Path, url: &str) -> Command {
    let mut command = clean_client(home);
    command.env("HUB_URL", url);
    command
}

#[test]
fn shared_approval_saves_url_and_token_and_next_call_needs_no_environment() {
    let hub = Hub::start("enrol-config-live");
    let home = hub.config_path().parent().unwrap().to_path_buf();
    let config_file = home.join(".config").join("agent-hub").join("config.toml");
    let url_line = format!("url = \"http://127.0.0.1:{}\"", hub.port);

    let port = hub.port;
    let approver = std::thread::spawn(move || {
        // The approval has to land after the enrol request creates the pending
        // agent, so it is retried until the agent is there rather than guessed
        // at with a fixed sleep.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let body = serde_json::json!({ "share": true }).to_string();
            let res = wire::rest(
                port,
                "POST",
                "/api/v1/enrol/live-agent/approve",
                Some(ADMIN_TOKEN),
                Some(&body),
            );
            if res.status == 200 {
                return;
            }
            assert!(
                res.status == 404,
                "unexpected approval response: {}",
                res.raw
            );
            assert!(
                std::time::Instant::now() < deadline,
                "the pending agent never appeared: {}",
                res.raw
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });

    let output = enrol_client(&home, &hub.url())
        .args([
            "enrol",
            "--id",
            "live-agent",
            "--name",
            "LiveAgent",
            "--why",
            "live enrolment test",
        ])
        .output()
        .expect("run agent-hub enrol");
    approver.join().expect("approver thread");

    assert!(output.status.success(), "enrol failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Enrolment approved"), "{stdout}");
    assert!(stdout.contains("Token saved to"), "{stdout}");
    assert!(
        stdout.contains("agent-hub call whoami"),
        "the next command is named: {stdout}"
    );

    let contents = std::fs::read_to_string(&config_file).expect("config written");
    assert!(
        contents.contains("[client]"),
        "no [client] table: {contents}"
    );
    assert!(contents.contains(&url_line), "no url line: {contents}");
    assert!(contents.contains("token = \""), "no token line: {contents}");
    // A raw append would duplicate the key on a second run, so a fresh file
    // holds each exactly once.
    assert_eq!(contents.matches("url =").count(), 1, "{contents}");
    assert_eq!(contents.matches("token =").count(), 1, "{contents}");

    // The command the client just recommended, with none of the hub
    // environment set at all: only HOME, and the file enrolment wrote.
    let called = clean_client(&home)
        .args(["call", "whoami"])
        .output()
        .expect("run agent-hub call whoami");
    assert!(
        called.status.success(),
        "call whoami failed: {}",
        String::from_utf8_lossy(&called.stderr)
    );
    let value = common::hub::stdout_json(&called);
    assert_eq!(
        value.get("actor").and_then(|actor| actor.as_str()),
        Some("live-agent"),
        "{value}"
    );
}

#[test]
fn enrol_help_prints_its_usage_without_touching_the_hub() {
    let dir = common::temp::TempDir::new("enrol-help");
    let output = clean_client(dir.path())
        .args(["enrol", "--help"])
        .output()
        .expect("run agent-hub enrol --help");

    assert!(output.status.success(), "help should exit zero: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage:"), "{stdout}");
    assert!(stdout.contains("agent-hub enrol"), "{stdout}");
}
