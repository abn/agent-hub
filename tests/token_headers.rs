//! `agent-hub token --headers` as Claude Code's header helper runs it: from
//! outside the project, given only the hub's URL, choosing from `config.toml`.

use std::process::Command;

use serde_json::{Value, json};

mod common;

use common::temp::TempDir;

const HUB: &str = "https://hub.example.net";

fn entry(member: &str, token: &str, default: bool) -> String {
    let default = if default { "default = true\n" } else { "" };
    format!(
        "[[credential]]\nurl = \"{HUB}\"\nmember = \"{member}\"\ntoken = \"{token}\"\n{default}\n"
    )
}

/// What the helper prints for `config`, with the project's member in the
/// environment as it would be for a hook, which the helper must not follow.
fn headers(tag: &str, config: &str) -> Value {
    let dir = TempDir::new(tag);
    let path = dir.join("config.toml");
    std::fs::write(&path, config).expect("write the config");
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["token", "--headers", "--agent", "claude-code"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &*dir)
        .env("HUB_CONFIG", &path)
        .env("HUB_MEMBER", "a")
        .env("CLAUDE_CODE_MCP_SERVER_URL", format!("{HUB}/mcp"))
        .current_dir(&*dir)
        .output()
        .expect("run agent-hub token");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    serde_json::from_slice(&output.stdout).expect("the helper prints JSON")
}

#[test]
fn the_only_credential_for_the_hub_is_sent() {
    assert_eq!(
        headers("headers-one", &entry("b", "tok-b", false)),
        json!({"Authorization": "Bearer tok-b", "X-Agent-Id": "claude-code"})
    );
}

#[test]
fn with_several_the_one_marked_default_is_sent_whatever_the_project_selects() {
    let config = format!(
        "{}{}",
        entry("a", "tok-a", false),
        entry("b", "tok-b", true)
    );
    assert_eq!(
        headers("headers-default", &config),
        json!({"Authorization": "Bearer tok-b", "X-Agent-Id": "claude-code"})
    );
}

#[test]
fn with_several_and_no_default_no_authorization_is_sent() {
    let config = format!(
        "{}{}",
        entry("a", "tok-a", false),
        entry("b", "tok-b", false)
    );
    assert_eq!(
        headers("headers-none", &config),
        json!({"X-Agent-Id": "claude-code"})
    );
}
