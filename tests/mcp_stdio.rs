//! End-to-end smoke for the MCP server over the stdio transport.
//!
//! Spawns the built binary as `agent-hub mcp` and speaks line-delimited
//! JSON-RPC over its stdin and stdout: initialize, tools/list, tools/call.
//! Reads are bounded so a broken server fails the test instead of hanging it.

use serde_json::json;

mod common;

use common::stdio::{PROTOCOL_VERSION, StdioClient as McpServer};
use common::temp::TempDir;

#[test]
fn initialize_list_and_call_over_stdio() {
    let data_dir = TempDir::new("mcp-stdio");
    let mut server = McpServer::mcp(&data_dir, &[]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-smoke", "version": "0.0.0"},
        }),
    );
    let result = init.get("result").expect("initialize returns a result");
    assert_eq!(result["serverInfo"]["name"], "agent-hub");
    assert!(
        !result["capabilities"]["tools"].is_null(),
        "the tools capability is advertised"
    );

    server.notify("notifications/initialized");

    let listed = server.call("tools/list", json!({}));
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    assert!(
        tools.iter().any(|tool| tool["name"] == "version"),
        "the version tool is listed"
    );

    let called = server.call("tools/call", json!({"name": "version", "arguments": {}}));
    // Like every other tool, version returns a structured object rather than a
    // bare string, so a hook reads a named field instead of special-casing it.
    let version = called["result"]["structuredContent"]["version"]
        .as_str()
        .unwrap_or_else(|| panic!("version returns a structured result: {called}"));
    assert!(
        version.starts_with("agent-hub "),
        "the version tool reports the hub version, got {version:?}"
    );
}

#[test]
fn stdout_stays_pure_json_with_logging_enabled() {
    // With logging on, any record written to stdout would break the protocol;
    // the reader fails the test on any non-JSON line, so this guards the log
    // destination.
    let data_dir = TempDir::new("mcp-stdio-log");
    let mut server = McpServer::mcp(&data_dir, &[("RUST_LOG", "info")]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-purity", "version": "0.0.0"},
        }),
    );
    assert!(init.get("result").is_some(), "initialize returns a result");

    server.notify("notifications/initialized");

    let listed = server.call("tools/list", json!({}));
    assert!(
        listed["result"]["tools"].is_array(),
        "tools/list returns an array"
    );
}

#[test]
fn embedded_stdio_against_running_hub_refuses() {
    use common::process::HubProcess;
    use std::process::{Command, Stdio};

    let data_dir = TempDir::new("mcp-stdio-conflict");
    let _hub = HubProcess::serve(&data_dir, "test-token", &[]);

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("mcp")
        .env("RUST_LOG", "error")
        .env("HUB_DATA_DIR", data_dir.path())
        .env_remove("HUB_URL")
        .env_remove("HUB_CONFIG")
        .stdin(Stdio::null())
        .output()
        .expect("run the binary");

    assert!(!output.status.success(), "process must fail: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("standalone against the local data directory, as the local admin"),
        "the mode says out loud what it is: {stderr}"
    );
    assert!(
        stderr.contains("a hub is already using this directory; set HUB_URL to reach it instead"),
        "the refusal names the situation and what to do: {stderr}"
    );
}

#[test]
fn resources_list_and_read_the_agent_guide() {
    // An agent wired only to MCP must be able to find the guide, not only a
    // human who knows the HTTP address. The handshake advertises resources, the
    // guide is listed, and reading it returns the document itself.
    let data_dir = TempDir::new("mcp-stdio-resources");
    let mut server = McpServer::mcp(&data_dir, &[]);

    let init = server.call(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-stdio-resources", "version": "0.0.0"},
        }),
    );
    let result = init.get("result").expect("initialize returns a result");
    assert!(
        !result["capabilities"]["resources"].is_null(),
        "the resources capability is advertised"
    );

    server.notify("notifications/initialized");

    let listed = server.call("resources/list", json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .expect("resources/list returns an array");
    assert!(
        resources
            .iter()
            .any(|resource| resource["uri"] == "agenthub://skill"),
        "the agent guide is listed as agenthub://skill"
    );

    let read = server.call("resources/read", json!({"uri": "agenthub://skill"}));
    let contents = read["result"]["contents"]
        .as_array()
        .expect("resources/read returns contents");
    let text = contents[0]["text"].as_str().expect("text contents");
    assert!(
        text.contains("Arriving without a token"),
        "the guide carries the arrival section"
    );
    assert!(text.contains("inbox_wait"), "the guide names the wait tool");
}
