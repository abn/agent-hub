//! A JSON-RPC client speaking to the built binary over its stdin and stdout.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// The binary as a child process, killed and reaped when dropped.
///
/// A test that gives it a data directory declares the directory first, so the
/// child is dropped, and the engine released, before the directory goes.
pub struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl StdioClient {
    /// Run the built binary with the given arguments and environment.
    ///
    /// `RUST_LOG` is `error` unless `env` says otherwise.
    pub fn spawn(args: &[&str], env: &[(&str, String)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command.args(args).env("RUST_LOG", "error");
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Nothing reads the log, so it goes nowhere rather than into a
            // pipe that would fill and block the child.
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the agent-hub binary");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        // stdout is blocking, so a reader thread forwards every line and the
        // test thread bounds each wait with a timeout.
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

    /// Run the stdio MCP server over a data directory.
    pub fn mcp(data_dir: &Path, env: &[(&str, &str)]) -> Self {
        let mut all = vec![("HUB_DATA_DIR", data_dir.to_string_lossy().into_owned())];
        // The config file of whoever runs the suite must never decide this
        // binary's mode: a `url` in it makes `mcp` proxy to that hub instead of
        // serving this data directory, and the suite then calls a hub it never
        // seeded. Point HUB_CONFIG at a path inside the test's own directory
        // that does not exist, so no user file is read at all.
        all.push((
            "HUB_CONFIG",
            data_dir.join("client-config").to_string_lossy().into_owned(),
        ));
        all.extend(env.iter().map(|(key, value)| (*key, value.to_string())));
        Self::spawn(&["mcp"], &all)
    }

    pub fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).expect("serialise message");
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to child stdin");
        self.stdin.flush().expect("flush child stdin");
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
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

    pub fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    /// Complete the handshake with a server that must accept it, and return
    /// the initialize answer.
    pub fn initialize(&mut self) -> Value {
        let init = self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "agent-hub-tests", "version": "0.0.0"},
            }),
        );
        assert!(init.get("result").is_some(), "initialize returns a result");
        self.notify("notifications/initialized");
        init
    }

    /// Call one tool and return the whole JSON-RPC message.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name": name, "arguments": arguments}))
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

impl Drop for StdioClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The structured payload of a successful tool call.
pub fn structured(message: &Value) -> &Value {
    message
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        .unwrap_or_else(|| panic!("expected a structured tool result, got {message}"))
}
