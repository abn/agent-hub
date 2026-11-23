//! A hub that is only a script, for the hook tests that are about what a hook
//! asks of the hub and not about what the hub does with it.
//!
//! It speaks the streamable HTTP MCP a hook's client opens, answers the few
//! tools a hook calls, keeps the session names it was asked to start, and
//! records every call with the headers that came with it. A refusal is shaped
//! as the hub shapes one: a JSON-RPC error whose data holds the hub's error
//! object.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

/// One tool call a hook made.
#[derive(Debug, Clone)]
pub struct Call {
    pub tool: String,
    pub args: Value,
    pub agent: Option<String>,
    pub token: Option<String>,
}

#[derive(Default)]
struct Fake {
    /// token -> (state, member). A token not listed is refused.
    tokens: HashMap<String, (&'static str, String)>,
    /// The reason an unlisted token is refused with.
    reason: &'static str,
    /// Session names the hub already holds.
    names: BTreeSet<String>,
    calls: Vec<Call>,
    requests: usize,
    next_id: u64,
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

fn failure(id: &Value, code: &str, reason: &str, message: &str, extra: Value) -> Response {
    let mut details = json!({"reason": reason});
    if let (Some(details), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        details.extend(extra.clone());
    }
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": message,
            "data": {"error": {
                "code": code,
                "message": message,
                "retryable": false,
                "details": details,
            }},
        },
    }))
    .into_response()
}

fn text(body: &Value) -> Value {
    json!({
        "content": [{"type": "text", "text": body.to_string()}],
        "structuredContent": body,
    })
}

async fn mcp(State(shared): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let message: Value = serde_json::from_str(&body).unwrap_or_default();
    let mut fake = shared.lock().expect("fake hub");
    fake.requests += 1;
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
        "initialize" => rpc(
            &id,
            json!({
                "protocolVersion": message["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-hub", "version": "1"},
            }),
        ),
        "tools/call" => {
            let tool = message["params"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let args = message["params"]["arguments"].clone();
            fake.calls.push(Call {
                tool: tool.clone(),
                args: args.clone(),
                agent: headers
                    .get("x-agent-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                token: token.clone(),
            });
            match tool.as_str() {
                "whoami" => {
                    // The hub names the agent the caller says it is.
                    let agent = headers
                        .get("x-agent-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("cli")
                        .to_string();
                    let who = match held {
                        Some(("active", member)) => json!({
                            "state": "active",
                            "member": member,
                            "agent": agent,
                            "kind": "participant",
                            "personal_project": "space-1",
                            "default_project": "space-1",
                            "suggested_session": "claude-code-2026-11-23",
                            "actor": member,
                        }),
                        Some((state, member)) => json!({"state": state, "member": member}),
                        None => json!({"state": "anonymous"}),
                    };
                    rpc(&id, text(&who))
                }
                "session_start" => {
                    let name = args["session_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let exclusive = args["exclusive"].as_bool().unwrap_or(false);
                    if exclusive && fake.names.contains(&name) {
                        return failure(
                            &id,
                            "conflict",
                            "session_name_taken",
                            &format!("session '{name}' already exists"),
                            json!({"session_id": "01JOLD"}),
                        );
                    }
                    fake.names.insert(name.clone());
                    fake.next_id += 1;
                    let session_id = format!("01J{:04}", fake.next_id);
                    rpc(
                        &id,
                        text(&json!({
                            "session_id": session_id,
                            "project_id": args["project_id"],
                            "session_name": name,
                            "status": "active",
                            "handoff": "",
                            "anchor": null,
                        })),
                    )
                }
                "session_leave" => rpc(&id, text(&json!({"ok": true}))),
                "notify_wait" => rpc(&id, text(&json!({"notifications": {"pending": []}}))),
                _ => rpc(&id, text(&json!({}))),
            }
        }
        _ if message.get("id").is_none() => StatusCode::ACCEPTED.into_response(),
        _ => rpc(&id, json!({})),
    }
}

async fn pending(State(shared): State<Shared>) -> Response {
    shared.lock().expect("fake hub").requests += 1;
    Json(json!({"count": 0, "items": []})).into_response()
}

async fn projects(State(shared): State<Shared>) -> Response {
    shared.lock().expect("fake hub").requests += 1;
    Json(json!({"projects": [{"id": "homelab"}]})).into_response()
}

/// A fake hub on a thread of its own.
pub struct FakeHub {
    pub url: String,
    state: Shared,
}

impl FakeHub {
    /// A hub that knows `tokens`, each `(token, state, member)` with the state
    /// `active` or `pending`, and refuses any other with `reason`.
    pub fn start(reason: &'static str, tokens: &[(&str, &'static str, &str)]) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let state: Shared = Arc::new(Mutex::new(Fake {
            reason,
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
            .route("/api/v1/me/pending", get(pending))
            .route("/api/v1/projects", get(projects))
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

    /// A session name the hub already holds.
    pub fn hold(&self, name: &str) {
        self.state
            .lock()
            .expect("fake")
            .names
            .insert(name.to_string());
    }

    /// A session name the hub no longer holds.
    pub fn release(&self, name: &str) {
        self.state.lock().expect("fake").names.remove(name);
    }

    /// Every tool call made so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().expect("fake").calls.clone()
    }

    /// The calls of one tool.
    pub fn calls_of(&self, tool: &str) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| call.tool == tool)
            .collect()
    }

    /// How many requests of any kind the hub has answered.
    pub fn requests(&self) -> usize {
        self.state.lock().expect("fake").requests
    }
}

/// The built binary as a harness runs it, against a [`FakeHub`], with a state
/// directory, a home and a working directory of its own.
pub struct FakeRig {
    pub hub: FakeHub,
    dir: crate::common::temp::TempDir,
}

impl FakeRig {
    pub fn new(hub: FakeHub, tag: &str) -> Self {
        let dir = crate::common::temp::TempDir::new(&format!("{tag}-fake-rig"));
        std::fs::create_dir_all(dir.join("work")).expect("the working directory");
        std::fs::create_dir_all(dir.join("home")).expect("the home directory");
        Self { hub, dir }
    }

    pub fn work_dir(&self) -> std::path::PathBuf {
        self.dir.join("work")
    }

    pub fn home_dir(&self) -> std::path::PathBuf {
        self.dir.join("home")
    }

    /// The state file of one harness session.
    pub fn state_file(&self, harness: &str, id: &str) -> std::path::PathBuf {
        self.dir
            .join("state/agent-hub/hooks")
            .join(harness)
            .join(format!("{id}.json"))
    }

    pub fn state(&self, harness: &str, id: &str) -> Value {
        let text = std::fs::read_to_string(self.state_file(harness, id))
            .unwrap_or_else(|err| panic!("the state file of {id}: {err}"));
        serde_json::from_str(&text).expect("the state is JSON")
    }

    /// The binary with the hub, the token, the project and the session pinned,
    /// and nothing else of the caller's environment configured.
    pub fn command(&self, args: &[&str], token: &str) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("RUST_LOG", "error")
            .env("HOME", self.home_dir())
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("HUB_CONFIG", self.dir.join("none.toml"))
            .env("HUB_URL", &self.hub.url)
            .env("HUB_TOKEN", token)
            .env("HUB_PROJECT", "homelab")
            .env("HUB_SESSION", "feat-x")
            .env("GIT_CEILING_DIRECTORIES", &*self.dir)
            .current_dir(self.work_dir());
        command
    }

    /// Run `agent-hub hook <event> --harness <harness>` with `stdin` on its input.
    pub fn hook(&self, harness: &str, event: &str, stdin: &Value) -> std::process::Output {
        self.hook_with(harness, event, "tok", stdin, |_| {})
    }

    pub fn hook_with(
        &self,
        harness: &str,
        event: &str,
        token: &str,
        stdin: &Value,
        adjust: impl FnOnce(&mut std::process::Command),
    ) -> std::process::Output {
        let mut command = self.command(&["hook", event, "--harness", harness], token);
        adjust(&mut command);
        super::run_with_stdin(command, stdin)
    }
}
