//! The harness snippets bootstrap.md quotes equal what the plugins ship.
//!
//! An agent copies these entries by hand, so a plugin that changes its MCP
//! entry must fail here until the guide follows.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The JSON code blocks of the section whose heading begins with `**name**`.
fn snippet(guide: &str, name: &str) -> Value {
    let start = guide
        .find(&format!("**{name}**"))
        .unwrap_or_else(|| panic!("bootstrap.md has no {name} snippet"));
    let rest = &guide[start..];
    let open = rest.find("```json\n").expect("a json block") + "```json\n".len();
    let body = &rest[open..];
    let close = body.find("```").expect("a closed block");
    serde_json::from_str(&body[..close]).expect("the snippet parses as JSON")
}

fn guide() -> String {
    fs::read_to_string(root().join("skills/agent-hub/bootstrap.md")).expect("bootstrap.md")
}

fn read_json(path: &str) -> Value {
    let text = fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn claude_snippet_is_the_plugin_mcp_json() {
    assert_eq!(
        snippet(&guide(), "Claude Code"),
        read_json("plugins/claude/.mcp.json")
    );
}

/// The OpenCode plugin registers `{type: remote, url: <hub>/mcp, headers}` with a
/// bearer token and `X-Agent-Id` set to its default agent name. The entry is built in
/// code, so the shape is stated here and the guide must match it.
#[test]
fn opencode_snippet_is_the_entry_the_plugin_registers() {
    let guide = guide();
    let actual = snippet(&guide, "OpenCode");
    let expected = json!({
        "mcp": { "agent-hub": {
            "type": "remote",
            "url": "{{base_url}}/mcp",
            "headers": {
                "Authorization": "Bearer {file:~/.config/agent-hub/token}",
                "X-Agent-Id": "opencode"
            }
        }}
    });
    assert_eq!(actual, expected);

    let config =
        fs::read_to_string(root().join("plugins/opencode/config.mjs")).expect("config.mjs");
    let default = config
        .lines()
        .find_map(|l| l.strip_prefix("export const DEFAULT_AGENT_NAME = \""))
        .and_then(|l| l.split('"').next())
        .expect("the plugin's default agent name");
    assert_eq!(actual["mcp"]["agent-hub"]["headers"]["X-Agent-Id"], default);
}

/// The Antigravity plugin is not on this base. The comparison runs once its
/// `mcp_config.json` exists.
#[test]
fn antigravity_snippet_is_the_plugin_mcp_config() {
    let path = root().join("plugins/antigravity/mcp_config.json");
    if !path.is_file() {
        eprintln!("plugins/antigravity/mcp_config.json is absent; comparing the shape only");
        let snip = snippet(&guide(), "Antigravity");
        let server = &snip["mcpServers"]["agent-hub"];
        assert_eq!(server["command"], "agent-hub");
        assert_eq!(server["args"], json!(["mcp", "--proxy"]));
        assert_eq!(server["env"]["HUB_AGENT_ID"], "antigravity");
        return;
    }
    assert_eq!(
        snippet(&guide(), "Antigravity"),
        read_json("plugins/antigravity/mcp_config.json")
    );
}
