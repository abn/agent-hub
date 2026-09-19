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
    let content = called["result"]["content"]
        .as_array()
        .expect("tools/call returns content");
    let text = content[0]["text"].as_str().expect("text content");
    assert!(
        text.starts_with("agent-hub "),
        "the version tool reports the hub version, got {text:?}"
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
