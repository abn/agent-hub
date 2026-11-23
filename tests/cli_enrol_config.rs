//! The enrolment client leaves the next command configured.
//!
//! A shared approval saves both the hub url and the token, so the command the
//! client prints as its next step runs with none of the hub environment set.

use std::path::{Path, PathBuf};
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
        // The system-wide file is pointed at a path that is not one, so the
        // hub under test is the one this test started rather than whatever
        // this machine is configured to talk to.
        .env(
            "AGENT_HUB_SYSTEM_CONFIG",
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-system-config.toml"),
        )
        .env_remove("HUB_URL")
        .env_remove("HUB_TOKEN")
        .env_remove("HUB_CONFIG")
        .env_remove("HUB_AGENT_ID")
        .env_remove("HUB_MEMBER")
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
            "--member",
            "live-agent",
            "--display-name",
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
        contents.contains("[[credential]]"),
        "no credential entry: {contents}"
    );
    assert!(contents.contains(&url_line), "no url line: {contents}");
    assert!(
        contents.contains("member = \"live-agent\""),
        "no member line: {contents}"
    );
    assert!(contents.contains("token = \""), "no token line: {contents}");
    // A raw append would duplicate the entry on a second run, so a fresh file
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
        value.get("member").and_then(|member| member.as_str()),
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
    assert!(stdout.contains("HUB_CONFIG"), "{stdout}");
}

/// Enrolling as a member whose token is already stored is refused with both
/// ways out, and the stored token is left as it was.
#[test]
fn a_token_already_stored_for_the_member_is_refused_with_both_remedies() {
    let hub = Hub::start("enrol-collision");
    let home = hub.config_path().parent().unwrap().to_path_buf();
    let config_file = home.join(".config").join("agent-hub").join("config.toml");
    std::fs::create_dir_all(config_file.parent().expect("the config directory"))
        .expect("the directory");
    std::fs::write(
        &config_file,
        format!(
            "[[credential]]\nurl = \"{}\"\nmember = \"second\"\ntoken = \"the-first-token\"\n",
            hub.url()
        ),
    )
    .expect("write the stored credential");

    let output = clean_client(&home)
        .env("HUB_URL", hub.url())
        .args(["enrol", "--member", "second", "--display-name", "Second"])
        .output()
        .expect("run agent-hub enrol");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("a token is already stored for member 'second'"),
        "the refusal says what is in the way: {stdout}"
    );
    assert!(
        stdout.contains("--force"),
        "the refusal names re-enrolling over it: {stdout}"
    );
    assert!(
        stdout.contains("HUB_CONFIG"),
        "the refusal names a config file of its own: {stdout}"
    );
    assert!(
        !stdout.contains("requesting enrolment"),
        "the refusal must not reach the hub: {stdout}"
    );
    let after = std::fs::read_to_string(&config_file).expect("config still there");
    assert!(
        after.contains("the-first-token"),
        "the stored token is untouched: {after}"
    );
}

/// A second member on one machine is another entry in the same file, and the
/// first one is left alone.
#[test]
fn a_second_member_enrols_into_the_same_file_beside_the_first() {
    let hub = Hub::start("enrol-second-member");
    let home = hub.config_path().parent().unwrap().to_path_buf();
    let file = home.join(".config").join("agent-hub").join("config.toml");
    std::fs::create_dir_all(file.parent().expect("the config directory")).expect("the directory");
    std::fs::write(
        &file,
        "[[credential]]\nurl = \"http://hub.example\"\nmember = \"first\"\ntoken = \"the-first-token\"\n",
    )
    .expect("write the first member's credential");
    let approver = approve_when_pending(hub.port, "second-agent", None);

    let output = clean_client(&home)
        .env("HUB_URL", hub.url())
        .args([
            "enrol",
            "--member",
            "second-agent",
            "--display-name",
            "Second",
        ])
        .output()
        .expect("run agent-hub enrol");
    approver.join().expect("approver thread");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Enrolment approved"), "{stdout}");
    let written = std::fs::read_to_string(&file).expect("config");
    assert!(written.contains("the-first-token"), "{written}");
    assert!(written.contains("member = \"second-agent\""), "{written}");
}

/// An approval under another name re-keys the stored entry to it.
#[test]
fn an_approval_under_another_name_renames_the_stored_credential() {
    let hub = Hub::start("enrol-rename");
    let home = hub.config_path().parent().unwrap().to_path_buf();
    let file = home.join(".config").join("agent-hub").join("config.toml");
    let approver = approve_when_pending(hub.port, "asked-for", Some("approved-as"));

    let output = clean_client(&home)
        .env("HUB_URL", hub.url())
        .args([
            "enrol",
            "--member",
            "asked-for",
            "--display-name",
            "Renamed",
        ])
        .output()
        .expect("run agent-hub enrol");
    approver.join().expect("approver thread");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Enrolment approved for member 'approved-as'"),
        "{stdout}"
    );
    let written = std::fs::read_to_string(&file).expect("config");
    assert!(written.contains("member = \"approved-as\""), "{written}");
    assert!(!written.contains("asked-for"), "{written}");
}

/// Approve `member` as soon as its enrolment is pending, optionally under
/// another name.
fn approve_when_pending(
    port: u16,
    member: &'static str,
    rename: Option<&'static str>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut body = serde_json::json!({ "share": true });
            if let Some(rename) = rename {
                body["member"] = serde_json::json!(rename);
            }
            let res = wire::rest(
                port,
                "POST",
                &format!("/api/v1/enrol/{member}/approve"),
                Some(ADMIN_TOKEN),
                Some(&body.to_string()),
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
                "the pending member never appeared: {}",
                res.raw
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    })
}
