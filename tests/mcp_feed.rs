//! End-to-end tests for the MCP feed tools and the streamable HTTP transport.
//!
//! The stdio tests spawn the built binary as `agent-hub mcp` and speak
//! line-delimited JSON-RPC. The HTTP test spawns the hub, which serves MCP at
//! `/mcp`, and speaks HTTP/1.1 over a raw socket so no new client dependency is
//! needed.

use std::path::Path;

use serde_json::json;

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;
use common::wire::mcp_post as http_post;

const ADMIN_TOKEN: &str = "mcp-feed-test-token";

/// Start a hub over a data directory, on a port of its own.
fn serve(data_dir: &Path) -> (HubProcess, u16) {
    let hub = HubProcess::serve(data_dir, ADMIN_TOKEN, &[]);
    let port = hub.port();
    (hub, port)
}

#[test]
fn signal_append_round_trips_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed::seed_project(data_dir.path(), "p1");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);
    server.initialize();

    let appended = server.call_tool(
        "signal_append",
        json!({
            "project_id": "p1",
            "kind": "signal",
            "summary": "first signal",
            "payload": {"n": 1},
            "actor": "attacker",
        }),
    );
    let event_id = structured(&appended)["event_id"]
        .as_str()
        .expect("signal_append returns an event id")
        .to_string();

    let read = server.call_tool("feed_read", json!({"project_id": "p1"}));
    let result = structured(&read);
    let events = result["events"].as_array().expect("events is an array");
    assert_eq!(events.len(), 1, "one event is stored");
    assert_eq!(events[0]["id"], event_id);
    assert_eq!(events[0]["summary"], "first signal");
    assert_eq!(events[0]["kind"], "signal");
    assert_eq!(events[0]["payload"]["n"], 1);
    assert_eq!(
        events[0]["actor"], "stdio-agent",
        "the actor is server-set, not taken from arguments"
    );
    assert_eq!(result["next_since"], event_id);
}

#[test]
fn signal_append_refuses_hub_owned_kinds() {
    let data_dir = TempDir::new("hub-kinds");
    common::seed::seed_project(data_dir.path(), "p1");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);
    server.initialize();

    for kind in ["system", "session", "artifact", "question", "answer"] {
        let response = server.call_tool(
            "signal_append",
            json!({"project_id": "p1", "kind": kind, "summary": "forged"}),
        );
        assert_eq!(
            response["error"]["data"]["error"]["code"], "invalid_argument",
            "kind {kind} must not be writable through signal_append: {response}"
        );
    }

    // The refused writes left no events behind.
    let read = server.call_tool("feed_read", json!({"project_id": "p1"}));
    assert_eq!(
        structured(&read)["events"]
            .as_array()
            .expect("events")
            .len(),
        0,
        "a refused kind is not recorded"
    );
}

#[test]
fn signal_append_validates_thread_id() {
    let data_dir = TempDir::new("thread-validate");
    common::seed::seed_project(data_dir.path(), "p1");
    common::seed::seed_project(data_dir.path(), "p2");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);
    server.initialize();

    let root_res = server.call_tool(
        "signal_append",
        json!({"project_id": "p1", "kind": "signal", "summary": "root"}),
    );
    let root_id = structured(&root_res)["event_id"]
        .as_str()
        .expect("root id")
        .to_string();

    // Unknown thread_id refused with not_found
    let bad_res = server.call_tool(
        "signal_append",
        json!({
            "project_id": "p1",
            "kind": "signal",
            "summary": "bad",
            "thread_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"
        }),
    );
    assert_eq!(
        bad_res["error"]["data"]["error"]["code"], "not_found",
        "{bad_res}"
    );

    // A reply is not the start of a thread, and the refusal says which is.
    let child_res = server.call_tool(
        "signal_append",
        json!({"project_id": "p1", "kind": "signal", "summary": "child", "thread_id": root_id}),
    );
    let child_id = structured(&child_res)["event_id"]
        .as_str()
        .expect("child id")
        .to_string();
    let nested_res = server.call_tool(
        "signal_append",
        json!({"project_id": "p1", "kind": "signal", "summary": "nested", "thread_id": child_id}),
    );
    assert_eq!(
        nested_res["error"]["data"]["error"]["code"], "invalid_argument",
        "{nested_res}"
    );
    assert!(
        nested_res["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(&root_id)),
        "{nested_res}"
    );

    // Cross-project thread_id refused with not_found
    let cross_res = server.call_tool(
        "signal_append",
        json!({
            "project_id": "p2",
            "kind": "signal",
            "summary": "cross",
            "thread_id": root_id
        }),
    );
    assert_eq!(
        cross_res["error"]["data"]["error"]["code"], "not_found",
        "{cross_res}"
    );
}

#[test]
fn idempotency_key_yields_one_event() {
    let data_dir = TempDir::new("idempotency");
    common::seed::seed_project(data_dir.path(), "p1");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);
    server.initialize();

    let arguments = json!({
        "project_id": "p1",
        "kind": "signal",
        "summary": "retry me",
        "idempotency_key": "key-1",
    });
    let first = server.call_tool("signal_append", arguments.clone());
    let second = server.call_tool("signal_append", arguments);
    assert_eq!(
        structured(&first)["event_id"],
        structured(&second)["event_id"],
        "a repeated key returns the original event id"
    );

    let read = server.call_tool("feed_read", json!({"project_id": "p1"}));
    let events = structured(&read)["events"]
        .as_array()
        .expect("events is an array")
        .len();
    assert_eq!(events, 1, "the repeated key does not create a second event");
}

#[test]
fn payload_over_cap_returns_payload_too_large() {
    let data_dir = TempDir::new("overcap");
    common::seed::seed_project(data_dir.path(), "p1");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "stdio-agent")]);
    server.initialize();

    let oversized = "a".repeat(300 * 1024);
    let response = server.call_tool(
        "signal_append",
        json!({
            "project_id": "p1",
            "kind": "signal",
            "summary": "too big",
            "payload": {"blob": oversized},
        }),
    );
    assert_eq!(
        response["error"]["data"]["error"]["code"], "payload_too_large",
        "an oversized payload maps to the hub code: {response}"
    );
}

#[test]
fn streamable_http_requires_a_bearer_token() {
    let data_dir = TempDir::new("http");
    common::seed::seed_project(data_dir.path(), "http");
    let (_child, port) = serve(data_dir.path());

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-feed-http", "version": "0.0.0"},
        },
    })
    .to_string();

    let denied = http_post(port, &initialize, None, None);
    assert_eq!(
        denied.status, 401,
        "a request without a token is rejected: {}",
        denied.raw
    );

    let initialized = http_post(port, &initialize, Some(ADMIN_TOKEN), None);
    assert_eq!(
        initialized.status, 200,
        "initialize succeeds with a token: {}",
        initialized.raw
    );
    assert!(
        initialized.raw.contains("serverInfo"),
        "initialize returns server info: {}",
        initialized.raw
    );
    let session = initialized
        .header("mcp-session-id")
        .expect("initialize assigns a session id");

    let ready = http_post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(
        ready.status, 202,
        "the initialized notification is accepted: {}",
        ready.raw
    );

    let appended = http_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "signal_append",
                "arguments": {
                    "project_id": "http",
                    "kind": "signal",
                    "summary": "over http",
                },
            },
        })
        .to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(
        appended.status, 200,
        "signal_append succeeds over http: {}",
        appended.raw
    );
    assert!(
        appended.raw.contains("event_id"),
        "signal_append returns an event id: {}",
        appended.raw
    );

    let read = http_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "feed_read", "arguments": {"project_id": "http"}},
        })
        .to_string(),
        Some(ADMIN_TOKEN),
        Some(&session),
    );
    assert_eq!(
        read.status, 200,
        "feed_read succeeds over http: {}",
        read.raw
    );
    assert!(
        read.raw.contains("over http"),
        "the appended event round-trips over http: {}",
        read.raw
    );
    assert!(
        read.raw.contains("\"human\""),
        "the http actor is the resolved principal: {}",
        read.raw
    );
}
