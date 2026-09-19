//! End-to-end tests for the MCP feed tools and the streamable HTTP transport.
//!
//! The stdio tests spawn the built binary as `agent-hub mcp` and speak
//! line-delimited JSON-RPC. The HTTP test spawns the hub, which serves MCP at
//! `/mcp`, and speaks HTTP/1.1 over a raw socket so no new client dependency is
//! needed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

mod common;

use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

const PROTOCOL_VERSION: &str = "2025-06-18";
const ADMIN_TOKEN: &str = "mcp-feed-test-token";

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

/// A child process killed when the test ends.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    listener.local_addr().expect("local addr").port()
}

fn spawn_http(data_dir: &Path, port: u16) -> ChildGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .env("RUST_LOG", "error")
        .env("HUB_DATA_DIR", data_dir)
        .env("HUB_BIND", format!("127.0.0.1:{port}"))
        .env("HUB_ADMIN_TOKEN", ADMIN_TOKEN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn the hub");
    ChildGuard(child)
}

/// How many ports to try before calling a hub start a failure.
///
/// `free_port` hands back a port it has already released, so another process
/// can take it in the gap before the child binds. The child then exits at
/// once, and a fresh port is a retry rather than a lost test run.
const START_ATTEMPTS: usize = 3;

/// Start a hub on a port of its own, retrying if the port was taken under it.
fn serve(data_dir: &Path) -> (ChildGuard, u16) {
    let mut lost = Vec::new();
    for _ in 0..START_ATTEMPTS {
        let port = free_port();
        let mut guard = spawn_http(data_dir, port);
        match wait_for_port(&mut guard, port) {
            Ok(()) => return (guard, port),
            Err(status) => lost.push(format!("port {port}: {status}")),
        }
    }
    panic!(
        "the hub exited on every one of {START_ATTEMPTS} ports: {}",
        lost.join("; ")
    );
}

/// Wait until the hub answers, or say how it exited before it could.
///
/// Watching the child is what turns "address already in use" from a twenty
/// second wait and a panic naming the wrong cause into an answer the caller
/// can act on.
fn wait_for_port(child: &mut ChildGuard, port: u16) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        // The child first: a port that answers is not this hub when this hub
        // is already gone, and whatever did answer is another test's.
        if let Ok(Some(status)) = child.0.try_wait() {
            return Err(status.to_string());
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("the hub did not start on port {port}");
}

/// One raw HTTP/1.1 POST, read until the server closes or the read stalls.
fn http_post(port: u16, body: &str, token: Option<&str>, session: Option<&str>) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub listener");
    stream
        .set_read_timeout(Some(Duration::from_millis(1500)))
        .expect("set read timeout");

    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(session) = session {
        request.push_str(&format!(
            "mcp-session-id: {session}\r\nmcp-protocol-version: {PROTOCOL_VERSION}\r\n"
        ));
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut raw = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => raw.push_str(&String::from_utf8_lossy(&buffer[..n])),
            Err(_) => break,
        }
    }

    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("response has no HTTP status line: {raw:?}"));
    HttpResponse { status, raw }
}

struct HttpResponse {
    status: u16,
    raw: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<String> {
        self.raw
            .lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
    }
}

#[test]
fn streamable_http_requires_a_bearer_token() {
    let data_dir = TempDir::new("http");
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
