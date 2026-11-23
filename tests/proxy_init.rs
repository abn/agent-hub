//! The stdio proxy's start and its swaps, against a hub that is only a script.
//!
//! The fake hub answers the MCP calls the proxy makes, records the handshake it
//! was given, and refuses tokens it does not know. What is asserted is what a
//! client sees: the handshake, the tool list, and the notification that says
//! the list changed.
#![cfg(feature = "client")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

mod common;
use common::temp::TempDir;

#[derive(Default)]
struct Fake {
    /// The `initialize` params each handshake carried.
    initializes: Vec<Value>,
    /// token -> (state, member)
    tokens: HashMap<String, (&'static str, String)>,
    /// The reason an unknown token is refused with.
    reason: &'static str,
    /// Set once the operator approves: the name the member is approved under.
    approved_as: Option<String>,
    port: u16,
}

type Shared = Arc<Mutex<Fake>>;

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

fn refusal(reason: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": {
            "code": "unauthenticated",
            "message": "the token is refused",
            "retryable": false,
            "details": {"reason": reason},
        }})),
    )
        .into_response()
}

fn rpc(id: &Value, result: Value) -> Response {
    let mut response = Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response();
    response
        .headers_mut()
        .insert("mcp-session-id", "fake-session".parse().expect("header"));
    response
}

fn text(body: &Value) -> Value {
    json!({
        "content": [{"type": "text", "text": body.to_string()}],
        "structuredContent": body,
    })
}

async fn mcp(State(fake): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let message: Value = serde_json::from_str(&body).unwrap_or_default();
    let mut fake = fake.lock().expect("fake hub");
    let token = bearer(&headers);
    let held = match &token {
        Some(token) => match fake.tokens.get(token) {
            Some(entry) => Some(entry.clone()),
            None => return refusal(fake.reason),
        },
        None => None,
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    match message["method"].as_str().unwrap_or_default() {
        "initialize" => {
            fake.initializes.push(message["params"].clone());
            rpc(
                &id,
                json!({
                    "protocolVersion": message["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-hub", "version": "1"},
                    "instructions": "the fake hub",
                }),
            )
        }
        "tools/list" => {
            let active = held.as_ref().is_some_and(|(state, _)| *state == "active");
            let names: &[&str] = if active {
                &["whoami", "session_start", "brain_put"]
            } else {
                &["enrol", "whoami"]
            };
            let tools: Vec<Value> = names
                .iter()
                .map(|name| json!({"name": name, "description": "t", "inputSchema": {"type": "object"}}))
                .collect();
            rpc(&id, json!({"tools": tools}))
        }
        "tools/call" => match message["params"]["name"].as_str().unwrap_or_default() {
            "whoami" => {
                let who = match held {
                    Some((state, member)) => json!({"state": state, "member": member}),
                    None => json!({"state": "anonymous"}),
                };
                rpc(&id, text(&who))
            }
            "enrol" => {
                let port = fake.port;
                fake.tokens.insert(
                    "pending-token".to_string(),
                    ("pending", "laptop".to_string()),
                );
                rpc(
                    &id,
                    text(&json!({
                        "status": "pending",
                        "member": "laptop",
                        "token": "pending-token",
                        "status_url": format!("http://127.0.0.1:{port}/api/v1/enrol/status"),
                    })),
                )
            }
            other => rpc(&id, text(&json!({"called": other}))),
        },
        _ if message.get("id").is_none() => StatusCode::ACCEPTED.into_response(),
        _ => rpc(&id, json!({})),
    }
}

async fn status(State(fake): State<Shared>, headers: HeaderMap) -> Response {
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut fake = fake.lock().expect("fake hub");
    let Some(token) = bearer(&headers) else {
        return refusal("token_missing");
    };
    if !fake.tokens.contains_key(&token) {
        return refusal(fake.reason);
    }
    match fake.approved_as.clone() {
        Some(member) => {
            fake.tokens.insert(token, ("active", member.clone()));
            Json(json!({"status": "approved", "member": member, "share": true})).into_response()
        }
        None => Json(json!({"status": "pending", "member": "laptop"})).into_response(),
    }
}

/// A fake hub on a thread of its own.
struct FakeHub {
    url: String,
    state: Shared,
}

impl FakeHub {
    fn start(reason: &'static str, tokens: &[(&str, &'static str, &str)]) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let state: Shared = Arc::new(Mutex::new(Fake {
            reason,
            port,
            tokens: tokens
                .iter()
                .map(|(token, state, member)| (token.to_string(), (*state, member.to_string())))
                .collect(),
            ..Fake::default()
        }));
        let app = Router::new()
            .route(
                "/mcp",
                post(mcp)
                    .get(|| async { StatusCode::METHOD_NOT_ALLOWED })
                    .delete(|| async { StatusCode::OK }),
            )
            .route("/api/v1/enrol/status", get(status))
            .with_state(state.clone());
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                    axum::serve(listener, app).await.expect("serve");
                });
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            state,
        }
    }

    fn approve_as(&self, member: &str) {
        self.state.lock().expect("fake").approved_as = Some(member.to_string());
    }

    fn initializes(&self) -> Vec<Value> {
        self.state.lock().expect("fake").initializes.clone()
    }
}

/// The built binary as a stdio MCP server, read line by line.
struct Proxy {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    next_id: u64,
    seen: Vec<Value>,
}

impl Proxy {
    fn spawn(env: &[(&str, String)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command
            .arg("mcp")
            .env("RUST_LOG", "error")
            .env_remove("HUB_URL")
            .env_remove("HUB_TOKEN")
            .env_remove("HUB_MEMBER")
            .env_remove("HUB_AGENT_ID")
            .env_remove("HUB_PROJECT");
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn agent-hub mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && sender.send(value).is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 0,
            seen: Vec::new(),
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write");
        self.stdin.flush().expect("flush");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(at) = self.seen.iter().position(|m| m["id"] == json!(id)) {
                return self.seen.remove(at);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) => self.seen.push(value),
                Err(_) => panic!("no answer to {method}"),
            }
        }
    }

    fn initialize(&mut self, client: &str) -> Value {
        let init = self.request(
            "initialize",
            json!({
                "protocolVersion": common::stdio::PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": client, "version": "9.9"},
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        init
    }

    fn tool_names(&mut self) -> Vec<String> {
        let listed = self.request("tools/list", json!({}));
        listed["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list: {listed}"))
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect()
    }

    fn whoami(&mut self) -> Value {
        let answer = self.request("tools/call", json!({"name": "whoami", "arguments": {}}));
        answer["result"]["structuredContent"].clone()
    }

    /// Wait for a notification, whether it arrived already or arrives now.
    fn notification(&mut self, method: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if let Some(at) = self.seen.iter().position(|m| m["method"] == json!(method)) {
                self.seen.remove(at);
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) => self.seen.push(value),
                Err(_) => return false,
            }
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn config_in(dir: &TempDir) -> String {
    dir.join("config.toml").display().to_string()
}

#[test]
fn the_proxy_opens_the_hub_with_the_clients_own_handshake() {
    let dir = TempDir::new("proxy-init-info");
    let hub = FakeHub::start("token_unknown", &[("good", "active", "laptop")]);
    let mut proxy = Proxy::spawn(&[
        ("HUB_URL", hub.url.clone()),
        ("HUB_TOKEN", "good".to_string()),
        ("HUB_CONFIG", config_in(&dir)),
    ]);

    let init = proxy.initialize("claude-code");
    assert_eq!(init["result"]["serverInfo"]["name"], "fake-hub", "{init}");
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        json!(true),
        "{init}"
    );
    let upstream = hub.initializes();
    assert_eq!(upstream.len(), 1, "{upstream:?}");
    assert_eq!(upstream[0]["clientInfo"]["name"], "claude-code");
    assert_eq!(upstream[0]["clientInfo"]["version"], "9.9");
    assert!(proxy.tool_names().contains(&"session_start".to_string()));
    assert_eq!(proxy.whoami()["member"], "laptop");
}

#[test]
fn a_refused_token_still_gets_a_handshake_and_a_local_whoami() {
    let dir = TempDir::new("proxy-init-refused");
    let hub = FakeHub::start("token_revoked", &[]);
    let mut proxy = Proxy::spawn(&[
        ("HUB_URL", hub.url.clone()),
        ("HUB_TOKEN", "stale".to_string()),
        ("HUB_CONFIG", config_in(&dir)),
    ]);

    let init = proxy.initialize("opencode");
    assert_eq!(init["result"]["serverInfo"]["name"], "agent-hub", "{init}");
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        json!(true),
        "{init}"
    );
    assert_eq!(proxy.tool_names(), ["whoami"]);
    let who = proxy.whoami();
    assert_eq!(who["state"], "token_revoked", "{who}");
    assert_eq!(who["hub_url"], hub.url, "{who}");
    let other = proxy.request("tools/call", json!({"name": "brain_put", "arguments": {}}));
    assert_eq!(other["result"]["isError"], json!(true), "{other}");
}

#[test]
fn with_no_hub_configured_mcp_exits_78_and_says_what_to_do() {
    let dir = TempDir::new("proxy-init-nohub");
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .arg("mcp")
        .env("RUST_LOG", "error")
        .env("HUB_CONFIG", config_in(&dir))
        .env_remove("HUB_URL")
        .env_remove("HUB_TOKEN")
        .env_remove("HUB_MEMBER")
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(78), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no hub configured"), "{stderr}");
    assert!(stderr.contains("--embedded"), "{stderr}");
}

#[test]
fn the_proxy_flag_is_accepted_and_noted() {
    let dir = TempDir::new("proxy-init-flag");
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["mcp", "--proxy"])
        .env("RUST_LOG", "error")
        .env("HUB_CONFIG", config_in(&dir))
        .env_remove("HUB_URL")
        .stdin(Stdio::null())
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--proxy is the default now and is ignored"),
        "{stderr}"
    );
}

fn wait_for(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Enrol through the proxy, approve under another name, and the same process
/// lists the full tools as the new member, with the config file re-keyed.
#[test]
fn an_approval_under_a_new_name_rekeys_the_config_and_swaps_the_same_proxy() {
    let dir = TempDir::new("proxy-init-swap");
    let config = dir.join("config.toml");
    let hub = FakeHub::start("token_unknown", &[]);
    let mut proxy = Proxy::spawn(&[
        ("HUB_URL", hub.url.clone()),
        ("HUB_CONFIG", config.display().to_string()),
    ]);

    let init = proxy.initialize("opencode");
    assert_eq!(init["result"]["serverInfo"]["name"], "fake-hub", "{init}");
    assert_eq!(proxy.tool_names(), ["enrol", "whoami"]);

    let enrolled = proxy.request("tools/call", json!({"name": "enrol", "arguments": {}}));
    assert_eq!(
        enrolled["result"]["structuredContent"]["status"], "pending",
        "{enrolled}"
    );
    wait_for("the pending credential", Duration::from_secs(10), || {
        let text = read(&config);
        text.contains("member = \"laptop\"") && text.contains("pending-token")
    });

    hub.approve_as("laptop-2");
    wait_for("the re-keyed credential", Duration::from_secs(20), || {
        read(&config).contains("member = \"laptop-2\"")
    });
    assert!(
        proxy.notification("notifications/tools/list_changed", Duration::from_secs(20)),
        "the client is told the list changed"
    );
    let mut names = Vec::new();
    wait_for("the full tool list", Duration::from_secs(20), || {
        names = proxy.tool_names();
        names.contains(&"session_start".to_string())
    });
    assert!(names.contains(&"brain_put".to_string()), "{names:?}");
    assert_eq!(proxy.whoami()["member"], "laptop-2");
    assert!(!read(&config).contains("member = \"laptop\"\n"));
}
