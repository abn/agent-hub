//! The release checks for members, agents and sessions.
//!
//! Each test is one named behaviour a client relies on, driven the way a
//! client reaches it: a hub started as a process, raw HTTP to `/mcp` and the
//! REST routes, the stdio proxy, and the launcher and setup scripts as shipped.
//! Nothing here calls into the store, so a test passes or fails on what a
//! caller of the built binary can see.
//!
//! None of these tests is ignored, skipped or conditional on a tool being
//! installed. `make gate/identity` lists this file and fails when a name below
//! is missing or filtered out.
#![cfg(unix)]

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{self, HttpResponse};

const ADMIN: &str = "release-identity-admin";
const PROJECT: &str = "homelab";

// ----------------------------------------------------------------- the hub

/// A hub serving as a process, with one project, over a directory of its own.
struct World {
    // The process is reaped before the directory it holds the engine lock in
    // is removed, so it is declared first.
    hub: HubProcess,
    _dir: TempDir,
}

impl World {
    fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let hub = HubProcess::serve(dir.path(), ADMIN, &[]);
        let world = Self { hub, _dir: dir };
        world.project(PROJECT, false);
        world
    }

    fn port(&self) -> u16 {
        self.hub.port()
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    /// One REST request: the status and the JSON body (null when there is none).
    fn rest(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let text = body.map(|body| body.to_string());
        let response = wire::rest(self.port(), method, path, token, text.as_deref());
        let parsed = serde_json::from_str(response.body().trim()).unwrap_or(Value::Null);
        (response.status, parsed)
    }

    fn admin(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.rest(method, path, Some(ADMIN), body)
    }

    fn project(&self, id: &str, confidential: bool) {
        let (status, body) = self.admin(
            "POST",
            "/api/v1/projects",
            Some(json!({"id": id, "display_name": id, "confidential": confidential})),
        );
        assert!(
            status == 200 || status == 201,
            "the project {id} was created: {body}"
        );
    }

    /// Create a member and issue its token.
    fn member(&self, id: &str) -> String {
        let (status, body) = self.admin(
            "POST",
            "/api/v1/members",
            Some(json!({"id": id, "display_name": id})),
        );
        assert_eq!(status, 201, "the member {id} was created: {body}");
        self.reissue(id)
    }

    /// Issue a member's token again; the previous one stops working.
    fn reissue(&self, id: &str) -> String {
        let (status, body) = self.admin("POST", &format!("/api/v1/members/{id}/token"), None);
        assert_eq!(status, 201, "a token was issued for {id}: {body}");
        body["token"]
            .as_str()
            .expect("the issued token is returned once")
            .to_string()
    }

    fn sessions(&self, project: &str) -> Vec<Value> {
        let (status, body) =
            self.admin("GET", &format!("/api/v1/sessions?project={project}"), None);
        assert_eq!(status, 200, "sessions are listed: {body}");
        body["sessions"].as_array().cloned().unwrap_or_default()
    }

    fn session(&self, project: &str, name: &str) -> Value {
        self.sessions(project)
            .into_iter()
            .find(|row| row["session_name"] == name)
            .unwrap_or_else(|| panic!("no session {name} in {project}"))
    }

    fn feed(&self, project: &str) -> Vec<Value> {
        let (status, body) = self.admin("GET", &format!("/api/v1/projects/{project}/feed"), None);
        assert_eq!(status, 200, "the feed is read: {body}");
        body["events"].as_array().cloned().unwrap_or_default()
    }

    fn answer(&self, question_id: &str, text: &str) {
        let (status, body) = self.admin(
            "POST",
            &format!("/api/v1/questions/{question_id}/answer"),
            Some(json!({"body": text})),
        );
        assert_eq!(status, 200, "the question was answered: {body}");
    }
}

/// The `{project_id, name}` form of the `session` argument.
fn named(name: &str) -> Value {
    json!({"project_id": PROJECT, "name": name})
}

/// The `participants` of a session row as `(member, agent)` pairs, sorted.
fn participants(row: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = row["participants"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|p| {
                    (
                        p["member"].as_str().unwrap_or_default().to_string(),
                        p["agent"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

fn pair(member: &str, agent: &str) -> (String, String) {
    (member.to_string(), agent.to_string())
}

// -------------------------------------------------------------- raw MCP

/// The `details.reason` of a refusal, from either shape it comes in: the
/// problem body of a refused request, or the error of a refused tool call.
fn reason_of(message: &Value) -> Option<String> {
    [
        "/error/details/reason",
        "/error/data/error/details/reason",
        "/error/data/details/reason",
        "/result/structuredContent/error/details/reason",
    ]
    .iter()
    .find_map(|pointer| message.pointer(pointer))
    .and_then(Value::as_str)
    .map(str::to_string)
}

/// The JSON-RPC message with `id` in a response body, whether it came as a
/// plain JSON body or as server-sent events.
fn message_for(response: &HttpResponse, id: u64) -> Value {
    let body = response.body();
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .find(|message| message["id"] == id)
        .or_else(|| serde_json::from_str(body.trim()).ok())
        .unwrap_or_else(|| panic!("no answer to request {id}: {}", response.raw))
}

/// One MCP connection over HTTP, as a client that holds no binary opens it.
#[derive(Debug)]
struct Mcp {
    port: u16,
    bearer: Option<String>,
    agent_header: Option<String>,
    session: String,
    next: u64,
}

impl Mcp {
    /// Open a connection as the named client, or return the refusal.
    fn open(
        port: u16,
        bearer: Option<&str>,
        client: &str,
        agent_header: Option<&str>,
    ) -> Result<Self, HttpResponse> {
        let mut mcp = Self {
            port,
            bearer: bearer.map(str::to_string),
            agent_header: agent_header.map(str::to_string),
            session: String::new(),
            next: 0,
        };
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": client, "version": "0.0.0"},
            },
        });
        let response = mcp.post(&initialize);
        if response.status != 200 {
            return Err(response);
        }
        mcp.session = response
            .header("mcp-session-id")
            .expect("an opened connection carries a session id");
        let acknowledged =
            mcp.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        assert!(
            acknowledged.status == 202 || acknowledged.status == 200,
            "initialized was accepted: {}",
            acknowledged.raw
        );
        Ok(mcp)
    }

    fn connect(port: u16, bearer: Option<&str>, client: &str) -> Self {
        Self::open(port, bearer, client, None)
            .unwrap_or_else(|refused| panic!("the connection was refused: {}", refused.raw))
    }

    fn post(&self, body: &Value) -> HttpResponse {
        let bearer = self.bearer.as_ref().map(|token| format!("Bearer {token}"));
        let mut headers = vec![("Accept", "application/json, text/event-stream")];
        if let Some(bearer) = &bearer {
            headers.push(("Authorization", bearer));
        }
        if let Some(agent) = &self.agent_header {
            headers.push(("X-Agent-Id", agent));
        }
        if !self.session.is_empty() {
            headers.push(("mcp-session-id", &self.session));
            headers.push(("mcp-protocol-version", PROTOCOL_VERSION));
        }
        wire::send(self.port, "POST", "/mcp", &headers, Some(&body.to_string()))
    }

    /// One request: the HTTP status and the JSON-RPC message (or the problem
    /// body of a refused request).
    fn rpc_status(&mut self, method: &str, params: Value) -> (u16, Value) {
        self.next += 1;
        let id = self.next;
        let response =
            self.post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if response.status == 200 {
            (200, message_for(&response, id))
        } else {
            let body = serde_json::from_str(response.body().trim()).unwrap_or(Value::Null);
            (response.status, body)
        }
    }

    fn call_status(&mut self, tool: &str, arguments: Value) -> (u16, Value) {
        self.rpc_status("tools/call", json!({"name": tool, "arguments": arguments}))
    }

    /// A tool call: the whole JSON-RPC message.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let (status, message) = self.call_status(tool, arguments);
        assert_eq!(status, 200, "{tool} reached the hub: {message}");
        message
    }

    /// A tool call that must succeed: its structured result.
    fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let message = self.call(tool, arguments);
        match message.pointer("/result/structuredContent") {
            Some(result) if message.pointer("/result/isError") != Some(&Value::Bool(true)) => {
                result.clone()
            }
            _ => panic!("{tool} did not succeed: {message}"),
        }
    }

    /// A tool call that must be refused: the `details.reason`.
    fn refused(&mut self, tool: &str, arguments: Value) -> String {
        let (_, message) = self.call_status(tool, arguments);
        reason_of(&message)
            .unwrap_or_else(|| panic!("{tool} was not refused with a reason: {message}"))
    }

    fn tool_names(&mut self) -> Vec<String> {
        let (status, message) = self.rpc_status("tools/list", json!({}));
        assert_eq!(status, 200, "tools were listed: {message}");
        let mut names: Vec<String> = message["result"]["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Write to the session brain under `name`, the way every client does:
    /// the `session` argument names the session and creates it on first use.
    fn put(&mut self, name: &str, path: &str, content: &str) -> Value {
        self.ok(
            "brain_put",
            json!({
                "store": "session",
                "path": path,
                "content": content,
                "session": named(name),
            }),
        )
    }

    /// The ids a result delivered in its `notifications` member.
    fn drain(&mut self, session: &str) -> Vec<Value> {
        let told = self.ok(
            "notify_wait",
            json!({"wait_seconds": 0, "session": named(session)}),
        );
        told["notifications"]["pending"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

/// An open event stream, read in short slices so a test can tell a stream the
/// hub closed from one that is still open.
struct Stream {
    socket: TcpStream,
    head: String,
    /// The last bytes read, to spot a chunked body's terminating chunk.
    tail: Vec<u8>,
}

impl Stream {
    fn open(port: u16, path: &str, headers: &[(&str, &str)]) -> Self {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let mut request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        socket.write_all(request.as_bytes()).expect("send");
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("timeout");
        let mut stream = Self {
            socket,
            head: String::new(),
            tail: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline && !stream.head.contains("\r\n\r\n") {
            match stream.socket.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => stream.head.push_str(&String::from_utf8_lossy(&buffer[..n])),
                Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
        }
        stream
    }

    /// The stream a connection holds for server messages.
    fn of(mcp: &Mcp) -> Self {
        let bearer = mcp.bearer.as_ref().map(|token| format!("Bearer {token}"));
        let mut headers = vec![
            ("mcp-session-id", mcp.session.as_str()),
            ("mcp-protocol-version", PROTOCOL_VERSION),
        ];
        if let Some(bearer) = &bearer {
            headers.push(("Authorization", bearer));
        }
        Self::open(mcp.port, "/mcp", &headers)
    }

    /// The freshness stream the PWA holds.
    fn freshness(port: u16, token: &str) -> Self {
        let bearer = format!("Bearer {token}");
        Self::open(port, "/api/v1/stream", &[("Authorization", &bearer)])
    }

    fn status(&self) -> u16 {
        self.head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0)
    }

    /// Whether the hub has closed the stream within the time: the socket
    /// ended, or the chunked body did (the connection may stay up after it).
    fn closed_within(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut buffer = [0u8; 4096];
        let head_end = self.head.find("\r\n\r\n").map_or(0, |at| at + 2);
        let ended = |tail: &mut Vec<u8>, bytes: &[u8]| {
            tail.extend_from_slice(bytes);
            let keep = tail.len().saturating_sub(16);
            tail.drain(..keep);
            tail.ends_with(b"\r\n0\r\n\r\n")
        };
        if self.tail.is_empty() && ended(&mut self.tail, &self.head.as_bytes()[head_end..]) {
            return true;
        }
        while Instant::now() < deadline {
            match self.socket.read(&mut buffer) {
                Ok(0) => return true,
                Ok(n) => {
                    if ended(&mut self.tail, &buffer[..n]) {
                        return true;
                    }
                }
                Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }
}

// ------------------------------------------------------- no-binary setup

#[test]
fn no_binary_http() {
    let world = World::start("release-no-binary");
    let token = world.member("laptop");

    // A client with no binary: a bearer header and its own handshake.
    let mut mcp = Mcp::connect(world.port(), Some(&token), "claude-code");
    let who = mcp.ok("whoami", json!({}));
    assert_eq!(who["state"], "active", "{who}");
    assert_eq!(who["member"], "laptop", "{who}");
    assert_eq!(
        who["agent"], "claude-code",
        "the agent comes from clientInfo: {who}"
    );
    assert_eq!(who["kind"], "participant", "{who}");

    // A write that names a session creates it, and no session was started.
    mcp.put("feat-x", "/kv/plan", "# Plan");
    let row = world.session(PROJECT, "feat-x");
    assert_eq!(row["status"], "active", "{row}");
    assert_eq!(
        participants(&row),
        vec![pair("laptop", "claude-code")],
        "{row}"
    );

    // The agent header beats the handshake name.
    let mut headed = Mcp::open(world.port(), Some(&token), "claude-code", Some("opencode"))
        .unwrap_or_else(|refused| panic!("a header refused the connection: {}", refused.raw));
    assert_eq!(headed.ok("whoami", json!({}))["agent"], "opencode");

    // A client that sends neither a known name nor a header is still a caller.
    let mut unknown = Mcp::connect(world.port(), Some(&token), "some-new-client");
    let who = unknown.ok("whoami", json!({}));
    assert_eq!(who["member"], "laptop");
    assert_eq!(
        who["agent"], "some-new-client",
        "an unknown name stays as sent: {who}"
    );
}

#[test]
fn enrol_via_mcp() {
    let world = World::start("release-enrol-mcp");

    // No bearer: the enrol surface and nothing else.
    let mut anonymous = Mcp::connect(world.port(), None, "claude-code");
    assert_eq!(anonymous.tool_names(), vec!["enrol", "whoami"]);
    assert_eq!(anonymous.ok("whoami", json!({}))["state"], "anonymous");
    assert_eq!(
        anonymous.refused(
            "brain_get",
            json!({"store": "project", "path": "/fs/index.md", "project_id": PROJECT})
        ),
        "token_missing"
    );

    let asked = anonymous.ok(
        "enrol",
        json!({"member": "laptop", "display_name": "Laptop", "reason": "release check", "os": "linux", "host": "laptop"}),
    );
    assert_eq!(asked["status"], "pending", "{asked}");
    assert_eq!(asked["member"], "laptop", "{asked}");
    let pending = asked["token"]
        .as_str()
        .expect("a pending bearer")
        .to_string();
    let status_url = asked["status_url"]
        .as_str()
        .expect("a status url")
        .to_string();
    assert!(status_url.ends_with("/api/v1/enrol/status"), "{status_url}");

    // Pending: still the enrol surface, and a write is refused with its reason.
    let mut waiting = Mcp::connect(world.port(), Some(&pending), "claude-code");
    assert_eq!(waiting.tool_names(), vec!["enrol", "whoami"]);
    let who = waiting.ok("whoami", json!({}));
    assert_eq!(who["state"], "pending", "{who}");
    assert_eq!(who["member"], "laptop", "{who}");
    assert_eq!(
        waiting.refused(
            "brain_put",
            json!({"store": "session", "path": "/kv/a", "content": "x", "session": named("feat-x")})
        ),
        "token_pending"
    );
    let (status, polled) = world.rest("GET", "/api/v1/enrol/status?wait=0", Some(&pending), None);
    assert_eq!(
        (status, polled["status"].as_str()),
        (200, Some("pending")),
        "{polled}"
    );

    // The operator approves, and the same token now works.
    let (status, approved) = world.admin("POST", "/api/v1/enrol/laptop/approve", Some(json!({})));
    assert!(status < 300, "approved: {approved}");
    let (status, polled) = world.rest("GET", "/api/v1/enrol/status?wait=0", Some(&pending), None);
    assert_eq!(status, 200);
    assert_eq!(polled["status"], "approved", "{polled}");
    assert_eq!(polled["member"], "laptop", "{polled}");

    let mut member = Mcp::connect(world.port(), Some(&pending), "claude-code");
    let tools = member.tool_names();
    for needed in ["brain_put", "session_start", "signal_append", "whoami"] {
        assert!(
            tools.iter().any(|name| name == needed),
            "{needed} is offered: {tools:?}"
        );
    }
    assert!(
        !tools.iter().any(|name| name == "enrol"),
        "an active member is not offered enrol"
    );
    assert_eq!(member.ok("whoami", json!({}))["state"], "active");
    member.put("feat-x", "/kv/plan", "# Plan");
}

// ------------------------------------------------ shared sessions and delivery

#[test]
fn two_agents_one_session() {
    let world = World::start("release-two-agents");
    let token = world.member("laptop");
    let mut claude = Mcp::connect(world.port(), Some(&token), "claude-code");
    let mut opencode = Mcp::connect(world.port(), Some(&token), "opencode");

    claude.put("feat-x", "/kv/claude", "from claude");
    opencode.put("feat-x", "/kv/opencode", "from opencode");

    let rows: Vec<Value> = world
        .sessions(PROJECT)
        .into_iter()
        .filter(|row| row["session_name"] == "feat-x")
        .collect();
    assert_eq!(rows.len(), 1, "one session holds both writers: {rows:?}");
    assert_eq!(
        participants(&rows[0]),
        vec![pair("laptop", "claude-code"), pair("laptop", "opencode")]
    );
    // Either agent reads what the other wrote: one shared brain.
    let seen = claude.ok(
        "brain_get",
        json!({"store": "session", "path": "/kv/opencode", "session": named("feat-x")}),
    );
    assert_eq!(seen["content"], "from opencode", "{seen}");

    // A reply to a question asked in the session reaches each agent once.
    let asked = claude.ok(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Which branch?", "session": named("feat-x")}),
    );
    let question = asked["question_id"]
        .as_str()
        .expect("a question id")
        .to_string();
    world.answer(&question, "main");
    for (who, mcp) in [("claude-code", &mut claude), ("opencode", &mut opencode)] {
        assert_eq!(mcp.drain("feat-x").len(), 1, "{who} is told of the reply");
        assert!(mcp.drain("feat-x").is_empty(), "{who} is told once");
    }
}

#[test]
fn child_session() {
    let world = World::start("release-child");
    let token = world.member("laptop");
    let mut mcp = Mcp::connect(world.port(), Some(&token), "claude-code");

    mcp.ok(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "feat-x"}),
    );
    mcp.put("feat-x", "/kv/parent", "the parent's page");

    // A child of a live parent starts, and an exclusive start finds no other.
    let child = "feat-x/review-ab12cd34";
    mcp.ok(
        "session_start",
        json!({"project_id": PROJECT, "session_name": child, "exclusive": true}),
    );
    assert_eq!(
        mcp.refused(
            "session_start",
            json!({"project_id": PROJECT, "session_name": child, "exclusive": true})
        ),
        "session_name_taken"
    );
    // Without a live parent there is no child, and the message offers a flat name.
    let (_, refusal) = mcp.call_status(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "ghost/task", "exclusive": true}),
    );
    assert_eq!(
        reason_of(&refusal).as_deref(),
        Some("session_parent_missing"),
        "{refusal}"
    );
    assert!(
        refusal.to_string().contains("ghost-task"),
        "the refusal suggests a flat name: {refusal}"
    );

    // The child writes under its own name and the parent's brain is untouched.
    mcp.put(child, "/kv/notes", "the child's page");
    let parent_pages = mcp.ok(
        "brain_list",
        json!({"store": "session", "session": named("feat-x")}),
    );
    let text = parent_pages.to_string();
    assert!(text.contains("/kv/parent"), "{text}");
    assert!(
        !text.contains("/kv/notes"),
        "the child's page is not in the parent: {text}"
    );
    let child_pages = mcp.ok(
        "brain_list",
        json!({"store": "session", "session": named(child)}),
    );
    let text = child_pages.to_string();
    assert!(
        text.contains("/kv/notes") && !text.contains("/kv/parent"),
        "{text}"
    );
    let parent = mcp.ok(
        "brain_get",
        json!({"store": "session", "path": "/kv/parent", "session": named("feat-x")}),
    );
    assert_eq!(parent["content"], "the parent's page", "{parent}");

    // Ending the parent ends its live children with it.
    mcp.ok("session_end", json!({"session": named("feat-x")}));
    assert_eq!(world.session(PROJECT, child)["status"], "ended");
    assert_eq!(world.session(PROJECT, "feat-x")["status"], "ended");
}

#[test]
fn reply_after_switch() {
    let world = World::start("release-switch");
    let token = world.member("laptop");
    let mut claude = Mcp::connect(world.port(), Some(&token), "claude-code");
    let mut opencode = Mcp::connect(world.port(), Some(&token), "opencode");

    let asked = claude.ok(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Which branch?", "session": named("feat-x")}),
    );
    // The member moves to another harness before the answer arrives.
    opencode.ok(
        "feed_read",
        json!({"project_id": PROJECT, "session": named("feat-x")}),
    );
    world.answer(asked["question_id"].as_str().expect("question"), "main");

    assert_eq!(
        claude.drain("feat-x").len(),
        1,
        "the asking harness is told"
    );
    assert_eq!(
        opencode.drain("feat-x").len(),
        1,
        "the harness it moved to is told too"
    );
    assert!(claude.drain("feat-x").is_empty());
    assert!(opencode.drain("feat-x").is_empty());
}

#[test]
fn answer_before_first_drain() {
    let world = World::start("release-first-drain");
    let asker = world.member("laptop");
    let other = world.member("nas");
    let mut a = Mcp::connect(world.port(), Some(&asker), "claude-code");
    let mut b = Mcp::connect(world.port(), Some(&other), "opencode");

    let asked = a.ok(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Roll the logs?", "session": named("feat-x")}),
    );
    // B joins before the answer exists and has drained nothing yet.
    b.ok(
        "feed_read",
        json!({"project_id": PROJECT, "session": named("feat-x")}),
    );
    world.answer(asked["question_id"].as_str().expect("question"), "yes");

    assert_eq!(
        b.drain("feat-x").len(),
        1,
        "the newcomer is told of the answer"
    );
    assert!(b.drain("feat-x").is_empty(), "once");
    assert_eq!(a.drain("feat-x").len(), 1, "the asker is told too");
    assert!(a.drain("feat-x").is_empty());
}

#[test]
fn concurrent_drains_deliver_once() {
    let world = World::start("release-concurrent");
    let token = world.member("laptop");
    let mut asker = Mcp::connect(world.port(), Some(&token), "claude-code");
    let asked = asker.ok(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Go?", "session": named("feat-x")}),
    );
    world.answer(asked["question_id"].as_str().expect("question"), "go");

    let port = world.port();
    let ready = Arc::new(Barrier::new(8));
    let drains: Vec<_> = (0..8)
        .map(|_| {
            let token = token.clone();
            let ready = Arc::clone(&ready);
            thread::spawn(move || {
                let mut mcp = Mcp::connect(port, Some(&token), "claude-code");
                ready.wait();
                mcp.drain("feat-x").len()
            })
        })
        .collect();
    let total: usize = drains
        .into_iter()
        .map(|d| d.join().expect("a drain ran"))
        .sum();
    assert_eq!(
        total, 1,
        "eight drains of one participant deliver the reply once"
    );
}

#[test]
fn addressed_comment_without_subscription() {
    let world = World::start("release-comment");
    let author = world.member("laptop");
    let reader = world.member("nas");
    let mut a = Mcp::connect(world.port(), Some(&author), "claude-code");
    let mut b = Mcp::connect(world.port(), Some(&reader), "opencode");

    let published = a.ok(
        "artifact_publish",
        json!({
            "project_id": PROJECT,
            "title": "Plan",
            "kind": "markdown",
            "content": "# Plan",
            "session": named("feat-x"),
        }),
    );
    b.ok(
        "comment_post",
        json!({
            "artifact_id": published["artifact_id"],
            "body": "Looks right",
            "session": named("review"),
        }),
    );

    // The author never subscribed to anything.
    let told = a.drain("feat-x");
    assert!(
        told.iter()
            .any(|item| item["kind"] == "comment" && item["direct"] == true),
        "the comment names the session that wrote the artifact: {told:?}"
    );
    assert!(a.drain("feat-x").is_empty(), "and only once");
}

// ------------------------------------------------ the credential store and trust

/// A config file path, a git working tree with a project file, and a shell
/// environment that reads only them.
struct Machine {
    root: TempDir,
}

impl Machine {
    fn new(tag: &str) -> Self {
        Self {
            root: TempDir::new(tag),
        }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// A repository whose project file selects a member for the hub.
    fn repo(&self, project: &str, url: &str, member: &str) -> PathBuf {
        let repo = self.root.join("repo");
        fs::create_dir_all(repo.join(".git")).expect("make the repository");
        fs::write(
            repo.join(".agent-hub.toml"),
            format!("project = \"{project}\"\nurl = \"{url}\"\nmember = \"{member}\"\n"),
        )
        .expect("write the project file");
        repo
    }

    /// The built binary, reading only this machine's config.
    fn agent_hub(&self, cwd: &Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_agent-hub"))
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &*self.root)
            .env("HUB_CONFIG", self.config())
            .env("RUST_LOG", "error")
            .output()
            .expect("run agent-hub")
    }

    fn headers(&self, cwd: &Path) -> Value {
        let out = self.agent_hub(cwd, &["token", "--headers", "--agent", "claude-code"]);
        assert!(out.status.success(), "token --headers exits 0: {out:?}");
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|err| panic!("token --headers prints one JSON object ({err}): {out:?}"))
    }
}

#[test]
fn confidential_pairing() {
    let world = World::start("release-pairing");
    world.project("secret", true);
    let ordinary = world.member("laptop");
    let granted = world.member("laptop-secret");
    let (status, body) = world.admin(
        "POST",
        "/api/v1/members/laptop-secret/grants",
        Some(json!({"project_id": "secret"})),
    );
    assert!(status < 300, "the grant was made: {body}");

    let machine = Machine::new("release-pairing-machine");
    let url = world.url();
    fs::write(
        machine.config(),
        format!(
            "[client]\nurl = \"{url}\"\nmember = \"laptop\"\n\n\
             [[credential]]\nurl = \"{url}\"\nmember = \"laptop\"\ntoken = \"{ordinary}\"\n\n\
             [[credential]]\nurl = \"{url}\"\nmember = \"laptop-secret\"\ntoken = \"{granted}\"\n"
        ),
    )
    .expect("write the config");
    let repo = machine.repo("secret", &url, "laptop-secret");

    // The repository asks for a member of its own, and nothing has allowed it.
    let before = machine.headers(&repo);
    assert!(
        before.get("Authorization").is_none(),
        "an untrusted pairing carries no token: {before}"
    );
    let state = machine.agent_hub(&repo, &["token"]);
    let state: Value = serde_json::from_slice(&state.stdout).expect("token prints JSON");
    assert_eq!(state["state"], "needs_trust", "{state}");

    // Allowing it records the pairing for this path, url and member.
    let trusted = machine.agent_hub(&repo, &["trust", "--path", repo.to_str().expect("utf-8")]);
    assert!(trusted.status.success(), "{trusted:?}");
    let after = machine.headers(&repo);
    assert_eq!(
        after["Authorization"],
        format!("Bearer {granted}"),
        "the trusted pairing selects the granted member: {after}"
    );
    assert_eq!(after["X-Agent-Id"], "claude-code");

    // The hub agrees: the paired member reads the confidential project and the
    // default one cannot see it.
    let mut paired = Mcp::connect(world.port(), Some(&granted), "claude-code");
    paired.ok(
        "brain_put",
        json!({"store": "project", "path": "/fs/index.md", "content": "# Secret", "project_id": "secret"}),
    );
    let mut default = Mcp::connect(world.port(), Some(&ordinary), "claude-code");
    let (_, refused) = default.call_status(
        "brain_get",
        json!({"store": "project", "path": "/fs/index.md", "project_id": "secret"}),
    );
    let text = refused.to_string();
    assert!(
        text.contains("not_found") || text.contains("not found"),
        "a confidential project is not found without a grant: {text}"
    );
}

// ---------------------------------------------------- revoke and reissue

#[test]
fn revoke_reissue() {
    let world = World::start("release-revoke");
    let port = world.port();
    let first = world.member("laptop");

    // Revoke: the token's streams close in two seconds and its next call is refused.
    let mut mcp = Mcp::connect(port, Some(&first), "claude-code");
    let mut events = Stream::of(&mcp);
    let mut fresh = Stream::freshness(port, &first);
    assert_eq!(
        (events.status(), fresh.status()),
        (200, 200),
        "{} {}",
        events.head,
        fresh.head
    );
    let (status, _) = world.admin("DELETE", "/api/v1/members/laptop/token", None);
    assert_eq!(status, 204);
    let revoked_at = Instant::now();
    assert!(
        events.closed_within(Duration::from_secs(2)),
        "the MCP stream is still open"
    );
    assert!(
        fresh.closed_within(
            Duration::from_secs(2)
                .saturating_sub(revoked_at.elapsed())
                .max(Duration::from_millis(300))
        ),
        "the freshness stream is still open"
    );
    let (status, message) = mcp.call_status("whoami", json!({}));
    assert_eq!(status, 401, "{message}");
    assert_eq!(
        reason_of(&message).as_deref(),
        Some("token_revoked"),
        "{message}"
    );
    let refused = match Mcp::open(port, Some(&first), "claude-code", None) {
        Ok(_) => panic!("a revoked token opened a connection"),
        Err(refused) => refused,
    };
    assert_eq!(refused.status, 401, "{}", refused.raw);
    assert!(refused.raw.contains("token_revoked"), "{}", refused.raw);

    // Reissue: the old token is replaced, its streams close, the next call is refused.
    let second = world.reissue("laptop");
    let mut mcp = Mcp::connect(port, Some(&second), "claude-code");
    let mut events = Stream::of(&mcp);
    let mut fresh = Stream::freshness(port, &second);
    assert_eq!((events.status(), fresh.status()), (200, 200));
    let third = world.reissue("laptop");
    let reissued_at = Instant::now();
    assert!(
        events.closed_within(Duration::from_secs(2)),
        "the replaced token's MCP stream is open"
    );
    assert!(
        fresh.closed_within(
            Duration::from_secs(2)
                .saturating_sub(reissued_at.elapsed())
                .max(Duration::from_millis(300))
        ),
        "the replaced token's freshness stream is open"
    );
    let (status, message) = mcp.call_status("whoami", json!({}));
    assert_eq!(status, 401, "{message}");
    assert_eq!(
        reason_of(&message).as_deref(),
        Some("token_revoked"),
        "{message}"
    );
    // The member, not the token, owns what it wrote: the new token carries on.
    let mut moved = Mcp::connect(port, Some(&third), "claude-code");
    assert_eq!(moved.ok("whoami", json!({}))["member"], "laptop");
}

#[test]
fn replacement_stream_survives() {
    let world = World::start("release-replacement");
    let port = world.port();
    let old = world.member("laptop");
    let old_mcp = Mcp::connect(port, Some(&old), "claude-code");
    let mut old_events = Stream::of(&old_mcp);
    assert_eq!(old_events.status(), 200);

    let replacement = world.reissue("laptop");
    let mut mcp = Mcp::connect(port, Some(&replacement), "claude-code");
    let mut events = Stream::of(&mcp);
    let mut fresh = Stream::freshness(port, &replacement);
    assert_eq!((events.status(), fresh.status()), (200, 200));

    assert!(
        old_events.closed_within(Duration::from_secs(2)),
        "the old token's stream is open"
    );
    assert!(
        !events.closed_within(Duration::from_secs(3)),
        "the replacement's MCP stream stays open"
    );
    assert!(
        !fresh.closed_within(Duration::from_millis(500)),
        "the replacement's freshness stream stays open"
    );
    assert_eq!(mcp.ok("whoami", json!({}))["state"], "active");
}

// ------------------------------------------------------------- the proxy

/// The stdio proxy as a client sees it: every line it prints is kept, so a
/// notification is as visible as an answer.
struct Proxy {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    seen: Vec<Value>,
    next: u64,
}

impl Proxy {
    fn start(url: &str, config: &Path, home: &Path, token: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command
            .arg("mcp")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("RUST_LOG", "error")
            .env("HUB_URL", url)
            .env("HUB_CONFIG", config);
        if let Some(token) = token {
            command.env("HUB_TOKEN", token);
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
        thread::spawn(move || {
            use std::io::{BufRead, BufReader};
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
            seen: Vec::new(),
            next: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to the proxy");
        self.stdin.flush().expect("flush");
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let value = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no answer to {method} within 30s"));
            self.seen.push(value.clone());
            if value["id"] == id {
                return value;
            }
        }
    }

    fn initialize(&mut self, client: &str) -> Value {
        let answer = self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": client, "version": "0.0.0"},
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        answer
    }

    fn tools(&mut self) -> Vec<String> {
        let answer = self.call("tools/list", json!({}));
        let mut names: Vec<String> = answer["result"]["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name": name, "arguments": arguments}))
    }

    /// Wait until the proxy has printed a notification with this method.
    fn notified(&mut self, method: &str, within: Duration) -> bool {
        if self.seen.iter().any(|m| m["method"] == method) {
            return true;
        }
        let deadline = Instant::now() + within;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(value) => {
                    let hit = value["method"] == method;
                    self.seen.push(value);
                    if hit {
                        return true;
                    }
                }
                Err(_) => return false,
            }
        }
        false
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Poll until `check` holds, and say so when it does not within the time.
fn eventually(what: &str, within: Duration, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(check(), "{what} did not hold within {within:?}");
}

#[test]
fn proxy_enrol_rename_hot_swap() {
    let world = World::start("release-hot-swap");
    let machine = Machine::new("release-hot-swap-machine");
    let config = machine.config();
    fs::write(&config, format!("[client]\nurl = \"{}\"\n", world.url())).expect("write the config");

    let mut proxy = Proxy::start(&world.url(), &config, &machine.root, None);
    let init = proxy.initialize("claude-code");
    assert!(
        init["result"].is_object(),
        "the proxy answers initialize: {init}"
    );
    assert_eq!(
        proxy.tools(),
        vec!["enrol", "whoami"],
        "a machine with no credential sees the enrol surface"
    );

    // Enrol through the proxy under one name; the credential is stored pending.
    let asked = proxy.tool(
        "enrol",
        json!({"member": "laptop", "display_name": "Laptop", "reason": "release check", "os": "linux", "host": "laptop"}),
    );
    let body = &asked["result"]["structuredContent"];
    assert_eq!(body["status"], "pending", "{asked}");
    eventually(
        "the pending token is stored",
        Duration::from_secs(5),
        || fs::read_to_string(&config).is_ok_and(|text| text.contains("member = \"laptop\"")),
    );

    // The operator approves it under another name.
    let (status, approved) = world.admin(
        "POST",
        "/api/v1/enrol/laptop/approve",
        Some(json!({"member": "workstation"})),
    );
    assert!(status < 300, "approved: {approved}");

    // The stored credential is re-keyed to the approved name, with the same token.
    eventually(
        "the credential is re-keyed",
        Duration::from_secs(15),
        || {
            fs::read_to_string(&config).is_ok_and(|text| {
                text.contains("member = \"workstation\"") && !text.contains("member = \"laptop\"")
            })
        },
    );
    assert!(
        proxy.notified("notifications/tools/list_changed", Duration::from_secs(15)),
        "the proxy says its tool list changed"
    );

    // The same proxy process lists the full tools and writes as the new member.
    assert!(
        proxy.child.try_wait().expect("poll the proxy").is_none(),
        "the proxy kept running"
    );
    let tools = proxy.tools();
    for needed in ["brain_put", "session_start", "signal_append"] {
        assert!(
            tools.iter().any(|name| name == needed),
            "{needed} is listed after the swap: {tools:?}"
        );
    }
    let who = proxy.tool("whoami", json!({}));
    assert_eq!(
        who["result"]["structuredContent"]["member"], "workstation",
        "{who}"
    );
    assert_eq!(
        who["result"]["structuredContent"]["agent"], "claude-code",
        "{who}"
    );
    let wrote = proxy.tool(
        "brain_put",
        json!({"store": "session", "path": "/kv/swap", "content": "after the swap", "session": named("feat-x")}),
    );
    assert!(
        wrote["result"]["isError"] != json!(true),
        "the write after the swap succeeded: {wrote}"
    );
    assert_eq!(
        participants(&world.session(PROJECT, "feat-x")),
        vec![pair("workstation", "claude-code")]
    );
}

#[test]
fn proxy_parity() {
    let world = World::start("release-parity");
    let token = world.member("laptop");

    // The same write, once straight to the hub and once through `agent-hub mcp`.
    let mut direct = Mcp::connect(world.port(), Some(&token), "claude-code");
    direct.ok(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "signal", "summary": "written directly", "session": named("feat-x")}),
    );

    let machine = Machine::new("release-parity-machine");
    let mut proxy = Proxy::start(&world.url(), &machine.config(), &machine.root, Some(&token));
    proxy.initialize("claude-code");
    let wrote = proxy.tool(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "signal", "summary": "written through the proxy", "session": named("feat-x")}),
    );
    assert!(wrote["result"]["isError"] != json!(true), "{wrote}");

    let feed = world.feed(PROJECT);
    let author_of = |summary: &str| -> (Value, Value) {
        let event = feed
            .iter()
            .find(|event| event["summary"] == summary)
            .unwrap_or_else(|| panic!("no event '{summary}' in the feed"));
        (event["author"].clone(), event["origin"].clone())
    };
    let (direct_author, direct_origin) = author_of("written directly");
    let (proxied_author, proxied_origin) = author_of("written through the proxy");
    assert_eq!(direct_author["kind"], "participant", "{direct_author}");
    assert_eq!(direct_author["member"], "laptop");
    assert_eq!(direct_author["agent"], "claude-code");
    assert_eq!(
        direct_author, proxied_author,
        "the proxy writes as the same participant in the same session"
    );
    assert_eq!(direct_origin, proxied_origin);
}

// ------------------------------------------------------------ launcher and dist

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run a shipped launcher on a host that is not one it supports, so it never
/// downloads anything and has to say hooks are off.
fn launcher_run(launcher: &Path, root: &Path, stdin: &str, args: &[&str]) -> std::process::Output {
    let stub = root.join("bin");
    fs::create_dir_all(&stub).expect("make the stub directory");
    let uname = stub.join("uname");
    fs::write(
        &uname,
        "#!/bin/sh\ncase \"$1\" in -m) echo mips ;; *) echo Plan9 ;; esac\n",
    )
    .expect("write the uname stub");
    fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).expect("chmod the stub");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).expect("make the cache base");
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).expect("chmod the cache base");

    let mut child = Command::new("sh")
        .arg(launcher)
        .args(args)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", stub.display()))
        .env("HOME", root)
        .env("XDG_CACHE_HOME", &cache)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the launcher");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write the hook input");
    child.wait_with_output().expect("wait for the launcher")
}

fn stdout_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn hooks_off_line() {
    let reason = "Hooks are off: unsupported platform Plan9-mips. Use the hub tools directly.";

    // Claude: the line is said once per harness session, then the hook is quiet.
    let root = TempDir::new("release-hooks-off-claude");
    let launcher = repo_root().join("plugins/claude/bin/agent-hub-launcher");
    let hook = ["hook", "session-start", "--harness", "claude"];
    let first = launcher_run(&launcher, &root, r#"{"session_id":"s-1"}"#, &hook);
    assert!(
        first.status.success(),
        "a hook never fails the harness: {first:?}"
    );
    let said: Value = serde_json::from_str(&stdout_text(&first)).expect("a hook prints JSON");
    assert_eq!(
        said["hookSpecificOutput"]["hookEventName"], "SessionStart",
        "{said}"
    );
    assert_eq!(
        said["hookSpecificOutput"]["additionalContext"], reason,
        "{said}"
    );
    let again = launcher_run(&launcher, &root, r#"{"session_id":"s-1"}"#, &hook);
    assert_eq!(stdout_text(&again), "{}", "the same session is told once");
    let other = launcher_run(&launcher, &root, r#"{"session_id":"s-2"}"#, &hook);
    assert!(
        stdout_text(&other).contains(reason),
        "a new session is told: {other:?}"
    );

    // Without a binary the token helper sends the agent and no credential.
    let helper = launcher_run(
        &launcher,
        &root,
        "",
        &["token", "--headers", "--agent", "claude-code"],
    );
    let headers: Value = serde_json::from_str(&stdout_text(&helper)).expect("headers are JSON");
    assert_eq!(
        headers,
        json!({"X-Agent-Id": "claude-code"}),
        "no Authorization without the resolver"
    );

    // Antigravity: the same line, in its hook shape.
    let root = TempDir::new("release-hooks-off-antigravity");
    let launcher = repo_root().join("plugins/antigravity/bin/agent-hub-launcher");
    let prompt = launcher_run(
        &launcher,
        &root,
        r#"{"conversationId":"c-1"}"#,
        &["hook", "prompt", "--harness", "antigravity"],
    );
    let said: Value = serde_json::from_str(&stdout_text(&prompt)).expect("a hook prints JSON");
    assert_eq!(said["injectSteps"][0]["ephemeralMessage"], reason, "{said}");
}

/// The caller's PATH without any directory that holds `agy`, so a test never
/// runs the user's Antigravity client.
fn path_without_agy() -> std::ffi::OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kept: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|dir| !dir.join("agy").exists())
        .collect();
    std::env::join_paths(kept).expect("rejoin PATH")
}

#[test]
fn setup_antigravity_installs_an_executable_launcher() {
    let home = TempDir::new("release-setup-agy");
    let out = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .args(["setup", "antigravity"])
        .env_clear()
        .env("PATH", path_without_agy())
        .env("HOME", &*home)
        .env("XDG_CONFIG_HOME", home.join("xdg/config"))
        .env("XDG_DATA_HOME", home.join("xdg/data"))
        .env("XDG_STATE_HOME", home.join("xdg/state"))
        .env("XDG_CACHE_HOME", home.join("xdg/cache"))
        .env("HUB_DATA_DIR", home.join("data"))
        .env("RUST_LOG", "error")
        .output()
        .expect("run setup");
    assert!(out.status.success(), "{out:?}");

    let plugin = home.join(".gemini/config/plugins/agent-hub");
    let launcher = plugin.join("bin/agent-hub-launcher");
    let mode = fs::metadata(&launcher)
        .expect("the launcher is installed")
        .permissions()
        .mode();
    assert!(
        mode & 0o111 != 0,
        "the installed launcher is executable: {mode:o}"
    );

    let mcp: Value =
        serde_json::from_slice(&fs::read(plugin.join("mcp_config.json")).expect("mcp config"))
            .expect("mcp config is JSON");
    let entry = &mcp["mcpServers"]["agent-hub"];
    let command = Path::new(entry["command"].as_str().expect("a command"));
    assert!(
        command.is_absolute(),
        "the entry names the launcher by absolute path: {entry}"
    );
    assert!(command.ends_with("bin/agent-hub-launcher"), "{entry}");
    assert!(command.exists(), "the named launcher exists: {entry}");
    assert_eq!(entry["args"], json!(["mcp"]), "{entry}");
    assert!(
        entry.get("env").is_none(),
        "no HUB_AGENT_ID or other env: {entry}"
    );
}

#[test]
fn antigravity_archive_holds_an_executable_launcher() {
    let version = env!("CARGO_PKG_VERSION");
    let built = Command::new("make")
        .arg("plugins/antigravity-dist")
        .current_dir(repo_root())
        .env("PATH", path_without_agy())
        .output()
        .expect("run make");
    assert!(
        built.status.success(),
        "the archive was built: {}{}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    let archive = repo_root().join(format!(
        "target/dist/agent-hub-antigravity-{version}.tar.gz"
    ));
    let listing = Command::new("tar")
        .arg("-tvzf")
        .arg(&archive)
        .output()
        .expect("list the archive");
    assert!(listing.status.success(), "{listing:?}");
    let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
    let line_for = |name: &str| {
        listing
            .lines()
            .find(|line| line.trim_end().ends_with(name))
            .unwrap_or_else(|| panic!("{name} is not in the archive:\n{listing}"))
            .to_string()
    };
    let launcher = line_for("agent-hub/bin/agent-hub-launcher");
    assert!(
        launcher.starts_with("-rwx"),
        "the launcher in the archive is an executable file: {launcher}"
    );
    for member in [
        "agent-hub/plugin.json",
        "agent-hub/mcp_config.json",
        "agent-hub/hooks.json",
        "agent-hub/skills/agent-hub/SKILL.md",
    ] {
        line_for(member);
    }
}
