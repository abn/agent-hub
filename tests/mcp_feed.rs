//! End-to-end tests for the MCP feed tools and the streamable HTTP transport.
//!
//! The stdio tests spawn the built binary as `agent-hub mcp` and speak
//! line-delimited JSON-RPC. The HTTP test spawns `agent-hub mcp-http` and
//! speaks HTTP/1.1 over a raw socket so no new client dependency is needed.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const PROTOCOL_VERSION: &str = "2025-06-18";
const ADMIN_TOKEN: &str = "mcp-feed-test-token";

/// A temp data directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-mcp-feed-{}-{nanos}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp data dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stdio MCP client: spawn, send, and wait for one response id.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn spawn(data_dir: &Path, agent_id: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .arg("mcp")
            .env("RUST_LOG", "error")
            .env("HUB_DATA_DIR", data_dir)
            .env("HUB_AGENT_ID", agent_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn the agent-hub binary");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).expect("serialise message");
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to child stdin");
        self.stdin.flush().expect("flush child stdin");
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.wait_for(id)
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn wait_for(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(READ_TIMEOUT)
                .unwrap_or_else(|_| panic!("timed out waiting for a response to request {id}"));
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|err| {
                panic!(
                    "stdout carried a non-JSON line, which corrupts the protocol: {err}: {line:?}"
                )
            });
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    fn initialize(&mut self) {
        let init = self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-feed", "version": "0.0.0"},
            }),
        );
        assert!(init.get("result").is_some(), "initialize returns a result");
        self.notify("notifications/initialized");
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name": name, "arguments": arguments}))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn structured(response: &Value) -> &Value {
    response
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        .unwrap_or_else(|| panic!("tool result carries structured content: {response}"))
}

#[test]
fn signal_append_round_trips_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    let mut server = McpServer::spawn(&data_dir.0, "stdio-agent");
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
fn idempotency_key_yields_one_event() {
    let data_dir = TempDir::new("idempotency");
    let mut server = McpServer::spawn(&data_dir.0, "stdio-agent");
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
    let mut server = McpServer::spawn(&data_dir.0, "stdio-agent");
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
        .arg("mcp-http")
        .env("RUST_LOG", "error")
        .env("HUB_DATA_DIR", data_dir)
        .env("HUB_BIND", format!("127.0.0.1:{port}"))
        .env("HUB_ADMIN_TOKEN", ADMIN_TOKEN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn agent-hub mcp-http");
    ChildGuard(child)
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("agent-hub mcp-http did not start on port {port}");
}

/// One raw HTTP/1.1 POST, read until the server closes or the read stalls.
fn http_post(port: u16, body: &str, token: Option<&str>, session: Option<&str>) -> HttpResponse {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).expect("connect to the mcp-http listener");
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
    let port = free_port();
    let _child = spawn_http(&data_dir.0, port);
    wait_for_port(port);

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
