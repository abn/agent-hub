//! A running hub, and the pieces the client tests drive it with.
//!
//! The stdio proxy and the one-shot call tests share this: each starts a real
//! hub on an ephemeral port, seeds an agent and its token, and then runs the
//! built binary against it. Each test crate uses a part of the module, so what
//! one of them leaves unused is allowed here rather than duplicated there.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_hub::principal::Trust;
use agent_hub::store::{identity, migrate, open_engine, projects};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const ADMIN_TOKEN: &str = "hub-client-admin";
pub const PROJECT: &str = "homelab";
pub const AGENT: &str = "my-agent";

const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// A temp directory removed when the test ends.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-client-{}-{nanos}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

/// Start a hub process over a data directory and wait until it listens.
fn serve(dir: &TempDir, port: u16) -> ChildGuard {
    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .env("RUST_LOG", "error")
            .env("HUB_DATA_DIR", &dir.0)
            .env("HUB_BIND", format!("127.0.0.1:{port}"))
            .env("HUB_ADMIN_TOKEN", ADMIN_TOKEN)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the hub"),
    );
    wait_for_port(port);
    child
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A hub serving on loopback, with one project, one agent, and its token.
pub struct Hub {
    pub port: u16,
    pub agent_token: String,
    dir: TempDir,
    child: ChildGuard,
}

impl Hub {
    /// Seed a data directory and start the hub over it.
    pub fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let agent_token = seed(&dir);
        let port = free_port();
        let child = serve(&dir, port);
        Self {
            port,
            agent_token,
            dir,
            child,
        }
    }

    /// Stop the hub and start it again on the same port and data directory,
    /// the way an upgrade or a reboot of the node looks to a client.
    pub fn restart(&mut self) {
        self.stop();
        self.start_again();
    }

    /// Stop the hub and wait for it to be gone, so the engine's lock on the
    /// data directory is free for the next process.
    pub fn stop(&mut self) {
        let _ = self.child.0.kill();
        let _ = self.child.0.wait();
    }

    /// Start a stopped hub on the same port and data directory.
    pub fn start_again(&mut self) {
        self.child = serve(&self.dir, self.port);
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// A config file path inside the test's own directory.
    ///
    /// Every spawned binary points at one, so no test ever reads the config
    /// file of whoever is running the suite.
    pub fn config_path(&self) -> PathBuf {
        self.dir.0.join("client-config")
    }

    /// Write a config file for the spawned binaries to read.
    pub fn write_config(&self, contents: &str) {
        std::fs::write(self.config_path(), contents).expect("write client config");
    }

    /// Read a path on the admin API and return its body.
    pub fn admin_get(&self, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect to the hub");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {ADMIN_TOKEN}\r\n\
             Connection: close\r\n\r\n",
            self.port
        );
        stream.write_all(request.as_bytes()).expect("write request");
        stream.flush().expect("flush request");
        let mut raw = String::new();
        stream.read_to_string(&mut raw).expect("read response");
        raw
    }
}

/// Create the project, the agent, and the agent's token before the hub starts.
///
/// Only one process may hold the engine, so the store is opened and closed
/// here rather than reached through the running hub.
fn seed(dir: &TempDir) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    runtime.block_on(async {
        let db = open_engine(&dir.0.join("hub.db"))
            .await
            .expect("open engine");
        migrate(&db).await.expect("migrate");
        projects::create(&db, PROJECT, "Homelab")
            .await
            .expect("create project");
        identity::create_agent(&db, AGENT, "My Agent", Trust::Trusted)
            .await
            .expect("create agent");
        identity::issue_token(&db, AGENT)
            .await
            .expect("issue token")
            .token
    })
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    listener.local_addr().expect("local addr").port()
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("the hub did not start on port {port}");
}

/// A JSON-RPC client speaking to a child process over its stdin and stdout.
pub struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl StdioClient {
    /// Run the built binary with the given arguments and environment.
    pub fn spawn(args: &[&str], env: &[(&str, String)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command.args(args).env("RUST_LOG", "error");
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
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

    fn send(&mut self, message: &Value) {
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

    /// Complete the handshake and return the initialize result.
    pub fn initialize(&mut self) -> Value {
        let init = self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "hub-client-test", "version": "0.0.0"},
            }),
        );
        self.notify("notifications/initialized");
        init
    }

    /// Call one tool and return the whole JSON-RPC message.
    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
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
