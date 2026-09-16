//! End-to-end smoke for the MCP server over the stdio transport.
//!
//! Spawns the built binary as `agent-hub mcp` and speaks line-delimited
//! JSON-RPC over its stdin and stdout: initialize, tools/list, tools/call.
//! Reads are bounded so a broken server fails the test instead of hanging it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const PROTOCOL_VERSION: &str = "2025-06-18";

struct McpServer {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn spawn() -> Self {
        Self::spawn_with_log("error")
    }

    fn spawn_with_log(rust_log: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("agent-hub-mcp-{}-{nanos}", std::process::id()));
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .arg("mcp")
            .env("RUST_LOG", rust_log)
            .env("HUB_DATA_DIR", &data_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn the agent-hub binary");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        // stdout is blocking, so a reader thread forwards every line; the test
        // thread then bounds each wait with a timeout.
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
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn initialize_list_and_call_over_stdio() {
    let mut server = McpServer::spawn();

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
    let mut server = McpServer::spawn_with_log("info");

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
