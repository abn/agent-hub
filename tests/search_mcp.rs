//! End-to-end search over MCP stdio.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const PROTOCOL_VERSION: &str = "2025-06-18";

struct McpServer {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn spawn(data_dir: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .arg("mcp")
            .env("RUST_LOG", "error")
            .env("HUB_DATA_DIR", data_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
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
        let mut line = serde_json::to_string(message).expect("serialise");
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).expect("write");
        self.stdin.flush().expect("flush");
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(READ_TIMEOUT)
                .unwrap_or_else(|_| panic!("timed out waiting for {id}"));
            let value: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("non-JSON stdout line: {err}: {line:?}"));
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
                "clientInfo": {"name": "mcp-search", "version": "0.0.0"},
            }),
        );
        assert!(init.get("result").is_some());
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
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
        .unwrap_or_else(|| panic!("structured content: {response}"))
}

#[test]
fn search_finds_appended_content() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-search-mcp-{nanos}"));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let mut server = McpServer::spawn(&dir);
    server.initialize();

    let appended = server.call_tool(
        "signal_append",
        json!({
            "project_id": "proj",
            "kind": "signal",
            "summary": "engine groundwork",
            "payload": {"body": "the engine keeps session state"},
        }),
    );
    assert!(structured(&appended).get("event_id").is_some());

    let found = server.call_tool("search", json!({"query": "engine"}));
    let results = structured(&found)["results"]
        .as_array()
        .expect("results array")
        .clone();
    assert!(
        results.iter().any(|hit| hit["kind"] == "feed"),
        "the appended signal is found"
    );

    let empty = server.call_tool("search", json!({"query": "   "}));
    assert!(
        empty.get("error").is_some(),
        "an empty query is a tool error"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
