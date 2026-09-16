//! End-to-end tests for the MCP session and brain tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, starts a session, writes both
//! brain namespaces, reads them back, and proves the search write-through by
//! opening the same hub store directly. Any non-JSON stdout line fails the test
//! because it would corrupt the protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_hub::store::open_engine;
use serde_json::{Value, json};

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
            "agent-hub-brain-mcp-{}-{nanos}-{tag}",
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
                "clientInfo": {"name": "mcp-brain", "version": "0.0.0"},
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

fn error_code(response: &Value) -> &str {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"))
}

#[test]
fn session_and_brain_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "named"}),
    );
    let result = structured(&started);
    let session_id = result["session_id"]
        .as_str()
        .expect("session_start returns a session id")
        .to_string();
    let brain_root = result["brain_root"]
        .as_str()
        .expect("session_start returns a brain root");
    assert!(
        brain_root.ends_with(".db"),
        "the brain root names the session file, got {brain_root:?}"
    );

    let put_kv = server.call_tool(
        "brain_put",
        json!({"path": "/kv/note", "content": "kv recovery note"}),
    );
    assert_eq!(structured(&put_kv)["ok"], true, "brain_put /kv/note");

    let put_fs = server.call_tool(
        "brain_put",
        json!({"path": "/fs/RECOVERY.md", "content": "terminal recovery marker"}),
    );
    assert_eq!(structured(&put_fs)["ok"], true, "brain_put /fs/RECOVERY.md");

    let put_scratch = server.call_tool(
        "brain_put",
        json!({"path": "/kv/scratch", "content": "scratch recovery line"}),
    );
    assert_eq!(
        structured(&put_scratch)["ok"],
        true,
        "brain_put /kv/scratch"
    );
    let deleted = server.call_tool("brain_delete", json!({"path": "/kv/scratch"}));
    assert_eq!(structured(&deleted)["ok"], true, "brain_delete /kv/scratch");
    let gone = server.call_tool("brain_get", json!({"path": "/kv/scratch"}));
    assert_eq!(
        error_code(&gone),
        "not_found",
        "a deleted brain value is absent"
    );

    let get_kv = server.call_tool("brain_get", json!({"path": "/kv/note"}));
    assert_eq!(structured(&get_kv)["content"], "kv recovery note");
    assert_eq!(structured(&get_kv)["path"], "/kv/note");

    let get_fs = server.call_tool("brain_get", json!({"path": "/fs/RECOVERY.md"}));
    assert_eq!(structured(&get_fs)["content"], "terminal recovery marker");

    let missing = server.call_tool("brain_get", json!({"path": "/kv/absent"}));
    assert_eq!(
        error_code(&missing),
        "not_found",
        "an absent brain value maps to not_found"
    );

    let listed = server.call_tool("brain_list", json!({}));
    let entries = structured(&listed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert!(
        entries.iter().any(|entry| entry == "/kv/note"),
        "brain_list reports the kv entry, got {entries:?}"
    );
    assert!(
        entries.iter().any(|entry| entry == "/fs/RECOVERY.md"),
        "brain_list reports the file entry, got {entries:?}"
    );

    let prefixed = server.call_tool("brain_list", json!({"path": "/kv"}));
    let kv_entries = structured(&prefixed)["entries"]
        .as_array()
        .expect("brain_list returns entries");
    assert_eq!(
        kv_entries.len(),
        1,
        "a /kv prefix lists only keys, got {kv_entries:?}"
    );
    assert_eq!(kv_entries[0], "/kv/note");

    let ended = server.call_tool("session_end", json!({"session_id": session_id}));
    assert_eq!(structured(&ended)["ok"], true, "session_end returns ok");

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events");
    assert!(
        events
            .iter()
            .any(|event| { event["kind"] == "session" && event["payload"]["action"] == "started" }),
        "a session start event is on the feed, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| { event["kind"] == "session" && event["payload"]["action"] == "ended" }),
        "a session end event is on the feed, got {events:?}"
    );

    // The search write-through lives in the same hub store. The server holds
    // the store lock while it runs, so stop it first, then read the rows it
    // committed.
    drop(server);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    let hits = runtime.block_on(async {
        let db = open_engine(&data_dir.0.join("hub.db"))
            .await
            .expect("open the hub store");
        let conn = db.connect().expect("connect");
        let mut rows = conn
            .query(
                "SELECT doc_id FROM search_docs WHERE fts_match(body, 'recovery')",
                (),
            )
            .await
            .expect("fts query");
        let mut hits = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            hits.push(row.get::<String>(0).expect("doc id"));
        }
        hits
    });
    assert!(
        hits.contains(&format!("brain:{session_id}:/kv/note")),
        "the kv brain put is findable by content, got {hits:?}"
    );
    assert!(
        hits.contains(&format!("brain:{session_id}:/fs/RECOVERY.md")),
        "the file brain put is findable by content, got {hits:?}"
    );
    assert!(
        !hits.contains(&format!("brain:{session_id}:/kv/scratch")),
        "the deleted brain row is removed from the corpus, got {hits:?}"
    );
}

#[test]
fn brain_tools_require_an_active_session() {
    let data_dir = TempDir::new("no-session");
    let mut server = McpServer::spawn(&data_dir.0);
    server.initialize();

    let response = server.call_tool("brain_get", json!({"path": "/kv/note"}));
    assert_eq!(
        error_code(&response),
        "conflict",
        "brain_get without a started session is a conflict"
    );
}
