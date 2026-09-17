//! End-to-end tests for the MCP artifact tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, publishes a public artifact,
//! reads it back, updates it, lists the project's artifacts, publishes a
//! protected one with an envelope, and proves the publish and update events
//! land on the feed. Any non-JSON stdout line fails the test because it would
//! corrupt the protocol.

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
            "agent-hub-artifacts-mcp-{}-{nanos}-{tag}",
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
                "clientInfo": {"name": "mcp-artifacts", "version": "0.0.0"},
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
fn artifact_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let published = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Report",
            "kind": "html",
            "content": "<h1>first draft</h1>",
        }),
    );
    let result = structured(&published);
    let artifact_id = result["artifact_id"]
        .as_str()
        .expect("artifact_publish returns an id")
        .to_string();
    assert_eq!(result["version"], 1, "a fresh publish is version 1");

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["content"], "<h1>first draft</h1>");
    assert_eq!(got["title"], "Report");
    assert_eq!(got["kind"], "html");
    assert_eq!(got["version"], 1);
    assert_eq!(got["protected"], false);

    let updated = server.call_tool(
        "artifact_update",
        json!({"artifact_id": artifact_id, "content": "<h1>second draft</h1>"}),
    );
    assert_eq!(
        structured(&updated)["version"],
        2,
        "an update increments the version"
    );

    let got = server.call_tool("artifact_get", json!({"artifact_id": artifact_id}));
    let got = structured(&got);
    assert_eq!(got["content"], "<h1>second draft</h1>");
    assert_eq!(got["version"], 2);

    let listed = server.call_tool("artifact_list", json!({"project_id": "proj"}));
    let artifacts = structured(&listed)["artifacts"]
        .as_array()
        .expect("artifact_list returns artifacts");
    assert_eq!(artifacts.len(), 1, "the project has one artifact");
    assert_eq!(artifacts[0]["id"], artifact_id);
    assert_eq!(artifacts[0]["version"], 2);

    let envelope = json!({
        "alg": "AES-GCM",
        "kdf": "PBKDF2-SHA256",
        "iterations": 600000,
        "salt": "c2FsdA==",
        "iv": "aXY=",
    });
    let protected = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Secret report",
            "kind": "markdown",
            "content": "ciphertextbase64",
            "envelope": envelope,
        }),
    );
    let protected_id = structured(&protected)["artifact_id"]
        .as_str()
        .expect("a protected publish returns an id")
        .to_string();

    let got = server.call_tool("artifact_get", json!({"artifact_id": protected_id}));
    let got = structured(&got);
    assert_eq!(got["protected"], true, "an envelope marks it protected");
    assert_eq!(got["content"], "ciphertextbase64");

    let listed = server.call_tool("artifact_list", json!({"project_id": "proj"}));
    let artifacts = structured(&listed)["artifacts"]
        .as_array()
        .expect("artifact_list returns artifacts");
    let secret = artifacts
        .iter()
        .find(|artifact| artifact["id"] == protected_id)
        .expect("the protected artifact is listed");
    assert!(
        secret["envelope"] == envelope,
        "the stored envelope round-trips, got {}",
        secret["envelope"]
    );

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events");
    assert!(
        events.iter().any(|event| {
            event["kind"] == "artifact"
                && event["payload"]["action"] == "published"
                && event["payload"]["artifact_id"] == artifact_id
        }),
        "a publish event lands on the feed, got {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            event["kind"] == "artifact"
                && event["payload"]["action"] == "updated"
                && event["payload"]["artifact_id"] == artifact_id
        }),
        "an update event lands on the feed, got {events:?}"
    );
}

#[test]
fn artifact_get_of_an_absent_id_is_not_found() {
    let data_dir = TempDir::new("absent");
    common::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let response = server.call_tool("artifact_get", json!({"artifact_id": "missing"}));
    let code = response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"));
    assert_eq!(code, "not_found");
}
