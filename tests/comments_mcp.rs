//! End-to-end tests for the MCP comment tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, publishes an artifact, posts a
//! comment, lists it, resolves it, and deletes it. Any non-JSON stdout line
//! fails the test because it would corrupt the protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

mod common;

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const PROTOCOL_VERSION: &str = "2025-06-18";

/// A temp data directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-comments-mcp-{}-{nanos}-{tag}",
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
    fn spawn(data_dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .arg("mcp")
            .env("RUST_LOG", "error")
            .env("HUB_DATA_DIR", data_dir)
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
                "clientInfo": {"name": "mcp-comments", "version": "0.0.0"},
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

fn tool_error_code(response: &Value) -> String {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"))
        .to_string()
}

fn publish(server: &mut McpServer) -> String {
    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Report",
            "kind": "html",
            "content": "<h1>draft</h1>",
        }),
    );
    structured(&published)["artifact_id"]
        .as_str()
        .expect("artifact_publish returns an id")
        .to_string()
}

#[test]
fn comment_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let artifact_id = publish(&mut server);

    let posted = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Needs a second look"}),
    );
    let posted = structured(&posted);
    let comment_id = posted["comment"]["id"]
        .as_str()
        .expect("comment_post returns an id")
        .to_string();
    let token = posted["delete_token"]
        .as_str()
        .expect("a first post returns a delete token")
        .to_string();
    assert_eq!(posted["comment"]["body"], "Needs a second look");
    assert_eq!(posted["comment"]["done"], false);
    assert!(
        posted["comment"].get("delete_token_hash").is_none(),
        "the token hash never leaves the server"
    );

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    let items = structured(&listed)["comments"]
        .as_array()
        .expect("comment_list returns comments");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], comment_id);
    assert!(
        items[0].get("delete_token_hash").is_none(),
        "listings carry no token hash"
    );

    let resolved = server.call_tool(
        "comment_resolve",
        json!({"artifact_id": artifact_id, "comment_id": comment_id, "done": true}),
    );
    assert_eq!(structured(&resolved)["comment"]["done"], true);

    let deleted = server.call_tool(
        "comment_delete",
        json!({"artifact_id": artifact_id, "comment_id": comment_id, "delete_token": token}),
    );
    assert_eq!(structured(&deleted)["ok"], true);

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    assert!(
        structured(&listed)["comments"]
            .as_array()
            .expect("comments array")
            .is_empty(),
        "a deleted comment is gone"
    );
}

#[test]
fn comment_post_rejects_an_unknown_anchor_mode() {
    let data_dir = TempDir::new("anchor");
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let artifact_id = publish(&mut server);

    let response = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Here", "anchor": {"mode": "region"}}),
    );
    assert_eq!(tool_error_code(&response), "invalid_argument");

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    assert!(
        structured(&listed)["comments"]
            .as_array()
            .expect("comments array")
            .is_empty(),
        "a rejected post stores nothing"
    );
}

#[test]
fn comment_post_replays_an_idempotency_key_without_a_second_token() {
    let data_dir = TempDir::new("idem");
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let artifact_id = publish(&mut server);

    let first = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Once", "idempotency_key": "key-one"}),
    );
    let first = structured(&first);
    assert!(
        first.get("delete_token").is_some(),
        "the first post returns a token"
    );
    let comment_id = first["comment"]["id"]
        .as_str()
        .expect("comment id")
        .to_string();

    let replay = server.call_tool(
        "comment_post",
        json!({"artifact_id": artifact_id, "body": "Once", "idempotency_key": "key-one"}),
    );
    let replay = structured(&replay);
    assert_eq!(replay["comment"]["id"], comment_id);
    assert!(
        replay.get("delete_token").is_none(),
        "a replay returns no second token"
    );

    let listed = server.call_tool("comment_list", json!({"artifact_id": artifact_id}));
    assert_eq!(
        structured(&listed)["comments"]
            .as_array()
            .expect("comments array")
            .len(),
        1,
        "the replay stores no duplicate"
    );
}

#[test]
fn comment_resolve_across_artifacts_is_not_found() {
    let data_dir = TempDir::new("cross");
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();
    let first = publish(&mut server);
    let second = publish(&mut server);

    let posted = server.call_tool(
        "comment_post",
        json!({"artifact_id": first, "body": "On the first"}),
    );
    let comment_id = structured(&posted)["comment"]["id"]
        .as_str()
        .expect("comment id")
        .to_string();

    let response = server.call_tool(
        "comment_resolve",
        json!({"artifact_id": second, "comment_id": comment_id, "done": true}),
    );
    assert_eq!(
        tool_error_code(&response),
        "not_found",
        "a comment of another artifact does not resolve"
    );

    let response = server.call_tool(
        "comment_delete",
        json!({"artifact_id": second, "comment_id": comment_id}),
    );
    assert_eq!(
        tool_error_code(&response),
        "not_found",
        "a comment of another artifact does not delete"
    );
}
