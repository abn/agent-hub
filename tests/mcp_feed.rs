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
        // A caller that guessed an intuitive wrong kind learns the allowed set
        // from the refusal rather than having to fetch the guide.
        let message = response["error"]["data"]["error"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(
            message.contains("signal, finished, approval"),
            "the refusal names the writable kinds: {response}"
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

/// Seed two active agents and return their tokens, in a data directory that no
/// process holds yet.
fn seed_two_agents(data_dir: &Path) -> (String, String) {
    common::seed::block_on(async {
        let db = common::store::open(data_dir).await;
        let first = common::seed::agent_token(&db, "agent-a", "Agent A").await;
        let second = common::seed::agent_token(&db, "agent-b", "Agent B").await;
        (first, second)
    })
}

/// Open an MCP session over the hub's streamable HTTP transport with a token.
fn mcp_session(port: u16, token: &str) -> String {
    let response = http_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-feed-cursor", "version": "0.0.0"},
            },
        })
        .to_string(),
        Some(token),
        None,
    );
    assert_eq!(
        response.status, 200,
        "initialize with an agent token: {}",
        response.raw
    );
    let session = response
        .header("mcp-session-id")
        .expect("initialize assigns a session id");
    http_post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(token),
        Some(&session),
    );
    session
}

/// Call a tool over the hub's MCP transport and return the structured result.
fn tool_call(
    port: u16,
    token: &str,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = http_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string(),
        Some(token),
        Some(session),
    );
    assert_eq!(response.status, 200, "{name}: {}", response.raw);
    structured(&response.message()).clone()
}

/// Append an event directly, before the hub over the directory is started.
fn seed_event(data_dir: &Path, project_id: &str, summary: &str) -> String {
    common::seed::block_on(async {
        let db = common::store::open(data_dir).await;
        agent_hub::store::events::append(
            &db,
            0,
            "seeder",
            None,
            agent_hub::store::events::NewEvent {
                project_id: project_id.to_string(),
                kind: "signal".to_string(),
                summary: summary.to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("seed event")
    })
}

#[test]
fn feed_read_without_since_advances_a_per_agent_project_cursor() {
    let data_dir = TempDir::new("feed-cursor");
    common::seed::seed_project(data_dir.path(), "p1");
    common::seed::seed_project(data_dir.path(), "p2");
    let first_p1 = seed_event(data_dir.path(), "p1", "p1 first");
    let second_p1 = seed_event(data_dir.path(), "p1", "p1 second");
    let only_p2 = seed_event(data_dir.path(), "p2", "p2 only");
    let (token_a, token_b) = seed_two_agents(data_dir.path());
    let (_child, port) = serve(data_dir.path());

    let session_a = mcp_session(port, &token_a);
    // A first read with no `since` has no cursor to start from: it returns the
    // newest page and records where it got to.
    let first = tool_call(
        port,
        &token_a,
        &session_a,
        "feed_read",
        json!({"project_id": "p1"}),
    );
    let events = first["events"].as_array().expect("events");
    assert_eq!(events.len(), 2, "the first read sees the whole feed");
    assert_eq!(
        first["next_since"].as_str(),
        Some(second_p1.as_str()),
        "the cursor is the newest event on the page"
    );

    // A second read with no `since` resumes from the stored cursor: nothing is
    // new, and it reports the same place rather than starting over.
    let second = tool_call(
        port,
        &token_a,
        &session_a,
        "feed_read",
        json!({"project_id": "p1"}),
    );
    assert!(
        second["events"].as_array().expect("events").is_empty(),
        "the second poll is past the stored cursor: {second}"
    );
    assert_eq!(
        second["next_since"].as_str(),
        Some(second_p1.as_str()),
        "an empty forward poll keeps its place: {second}"
    );
    let _ = &first_p1;

    // The cursor is per project: reading p2 starts from p2's own feed, not from
    // p1's position.
    let p2 = tool_call(
        port,
        &token_a,
        &session_a,
        "feed_read",
        json!({"project_id": "p2"}),
    );
    assert_eq!(
        p2["events"].as_array().expect("events").len(),
        1,
        "p2 has its own cursor: {p2}"
    );
    assert_eq!(p2["next_since"].as_str(), Some(only_p2.as_str()));

    // A second agent starts fresh: agent A's cursor in p1 does not hide the
    // feed from agent B.
    let session_b = mcp_session(port, &token_b);
    let agent_b = tool_call(
        port,
        &token_b,
        &session_b,
        "feed_read",
        json!({"project_id": "p1"}),
    );
    assert_eq!(
        agent_b["events"].as_array().expect("events").len(),
        2,
        "a second agent has its own cursor: {agent_b}"
    );

    // An explicit `since` wins over the stored cursor and is honoured as given.
    let explicit = tool_call(
        port,
        &token_b,
        &session_b,
        "feed_read",
        json!({"project_id": "p1", "since": first_p1}),
    );
    let events = explicit["events"].as_array().expect("events");
    assert_eq!(
        events
            .iter()
            .map(|event| event["summary"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["p1 second"],
        "an explicit since is honoured rather than the stored cursor: {explicit}"
    );
    assert_eq!(
        explicit["next_since"].as_str(),
        Some(second_p1.as_str()),
        "an explicit read also records progress"
    );

    // Agent B's cursor moved to the explicit page's end, so a follow-up poll
    // resumes from there.
    let follow_up = tool_call(
        port,
        &token_b,
        &session_b,
        "feed_read",
        json!({"project_id": "p1"}),
    );
    assert!(
        follow_up["events"].as_array().expect("events").is_empty(),
        "the explicit read advanced agent B's cursor: {follow_up}"
    );
    assert_eq!(follow_up["next_since"].as_str(), Some(second_p1.as_str()));
}
