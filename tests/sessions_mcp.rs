//! Session ownership and pickup over MCP, with two real agents.
//!
//! Each test starts one hub on loopback and drives it as two authenticated
//! agents, which is the only way to prove that a session belongs to its owner:
//! the stdio transport is the human admin and never sees the ownership rules.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_hub::principal::Trust;
use agent_hub::store::{identity, migrate, open_engine, projects};
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2025-06-18";
const ADMIN_TOKEN: &str = "sessions-mcp-admin";
const PROJECT: &str = "homelab";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-sessions-mcp-{}-{nanos}-{tag}",
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

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One agent's authenticated MCP connection.
#[derive(Clone)]
struct Agent {
    port: u16,
    token: String,
    session: String,
}

impl Agent {
    /// Call a tool and return its structured result, failing on an error.
    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = self.raw(name, arguments);
        let result = response
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} returned no structured content: {response}"));
        assert!(
            !response
                .pointer("/result/isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "{name} failed: {response}"
        );
        result
    }

    /// Call a tool and return the error code and message of its refusal.
    fn refusal(&self, name: &str, arguments: Value) -> (String, String) {
        let response = self.raw(name, arguments);
        let error = response
            .get("error")
            .cloned()
            .unwrap_or_else(|| panic!("{name} was expected to fail: {response}"));
        let code = error
            .pointer("/data/error/code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (code, message)
    }

    fn raw(&self, name: &str, arguments: Value) -> Value {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string();
        let raw = post(self.port, &body, Some(&self.token), Some(&self.session));
        parse_body(&raw)
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    listener.local_addr().expect("local addr").port()
}

fn spawn(data_dir: &Path, port: u16) -> ChildGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_agent-hub"))
        .env("RUST_LOG", "error")
        .env("HUB_DATA_DIR", data_dir)
        .env("HUB_BIND", format!("127.0.0.1:{port}"))
        .env("HUB_ADMIN_TOKEN", ADMIN_TOKEN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the hub");
    let guard = ChildGuard(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return guard;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the hub did not start on port {port}");
}

fn post(port: u16, body: &str, token: Option<&str>, session: Option<&str>) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(session) = session {
        request.push_str(&format!(
            "mcp-session-id: {session}\r\nmcp-protocol-version: {PROTOCOL_VERSION}\r\n"
        ));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut raw = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => raw.push_str(&String::from_utf8_lossy(&buffer[..n])),
            Err(_) => break,
        }
    }
    raw
}

/// The JSON-RPC message in a response, whether it came as JSON or as one event.
fn parse_body(raw: &str) -> Value {
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default();
    // A streamed response arrives as chunked events, so the message is the
    // first `data:` line that parses; a plain JSON body has no such line.
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find_map(|payload| serde_json::from_str(payload.trim()).ok())
        .or_else(|| serde_json::from_str(body.trim()).ok())
        .unwrap_or_else(|| panic!("no JSON-RPC message in the response: {raw}"))
}

fn header(raw: &str, name: &str) -> Option<String> {
    raw.lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
}

fn connect(port: u16, token: &str) -> Agent {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "sessions-mcp", "version": "0.0.0"},
        },
    })
    .to_string();
    let raw = post(port, &body, Some(token), None);
    let session = header(&raw, "mcp-session-id").expect("session id");
    post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(token),
        Some(&session),
    );
    Agent {
        port,
        token: token.to_string(),
        session,
    }
}

/// A hub with a project and the named agents, each already connected.
struct Fleet {
    _dir: TempDir,
    _child: ChildGuard,
    port: u16,
    data_dir: PathBuf,
    agents: Vec<Agent>,
}

impl Fleet {
    async fn new(tag: &str, agents: &[&str]) -> Self {
        let trusted: Vec<(&str, Trust)> = agents
            .iter()
            .map(|agent| (*agent, Trust::Trusted))
            .collect();
        Self::with_trust(tag, &trusted).await
    }

    async fn with_trust(tag: &str, agents: &[(&str, Trust)]) -> Self {
        let dir = TempDir::new(tag);
        let db = open_engine(&dir.0.join("hub.db"))
            .await
            .expect("open engine");
        migrate(&db).await.expect("migrate");
        projects::create(&db, PROJECT, "Homelab")
            .await
            .expect("create project");
        let mut tokens = Vec::new();
        for (agent, trust) in agents {
            identity::create_agent(&db, agent, agent, *trust)
                .await
                .expect("create agent");
            tokens.push(
                identity::issue_token(&db, agent)
                    .await
                    .expect("issue token")
                    .token,
            );
        }
        drop(db);

        let port = free_port();
        let child = spawn(&dir.0, port);
        let agents = tokens.iter().map(|token| connect(port, token)).collect();
        let data_dir = dir.0.clone();
        Self {
            _dir: dir,
            _child: child,
            port,
            data_dir,
            agents,
        }
    }

    fn agent(&self, index: usize) -> &Agent {
        &self.agents[index]
    }

    fn brain_file(&self, session_id: &str) -> PathBuf {
        self.data_dir
            .join("sessions")
            .join(PROJECT)
            .join(format!("{session_id}.db"))
    }
}

/// Call an admin REST route, the way the human's surface does.
fn admin(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let payload = body.unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {ADMIN_TOKEN}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    request.push_str(payload);
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok();
    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
    (status, body)
}

/// Soft-delete a session through the human's prune route.
fn prune(port: u16, session_id: &str) {
    let (status, body) = admin(
        port,
        "DELETE",
        &format!("/api/v1/storage/sessions/{session_id}"),
        None,
    );
    assert_eq!(status, 200, "prune: {body}");
}

fn create_project(port: u16, id: &str) {
    let (status, body) = admin(
        port,
        "POST",
        "/api/v1/projects",
        Some(&json!({"id": id, "display_name": id}).to_string()),
    );
    assert_eq!(status, 200, "create project: {body}");
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing '{key}' in {value}"))
        .to_string()
}

#[tokio::test]
async fn two_agents_under_one_name_keep_separate_brains() {
    let fleet = Fleet::new("two-agents", &["agent-one", "agent-two"]).await;

    let one = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    let two = fleet.agent(1).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    assert_ne!(
        text(&one, "session_id"),
        text(&two, "session_id"),
        "one name under two agents is two sessions"
    );
    assert_eq!(text(&one, "agent"), "agent-one");
    assert_eq!(text(&two, "agent"), "agent-two");

    fleet.agent(0).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/plan", "content": "mine"}),
    );
    let (code, _) = fleet
        .agent(1)
        .refusal("brain_get", json!({"path": "/kv/plan"}));
    assert_eq!(
        code, "not_found",
        "the other agent's brain holds nothing of the first's"
    );

    // The result carries what an agent needs to reach its brain, and no
    // filesystem path.
    let namespaces = one.get("namespaces").expect("namespaces").clone();
    assert_eq!(namespaces["kv"], "/kv");
    assert_eq!(namespaces["fs"], "/fs");
    assert!(
        !one.to_string().contains("sessions/"),
        "no filesystem path reaches an agent: {one}"
    );
    let listed = fleet.agent(0).call(
        "brain_list",
        json!({"path": namespaces["fs"].as_str().expect("fs namespace")}),
    );
    assert!(listed.get("entries").is_some(), "the fs namespace lists");
}

#[tokio::test]
async fn an_ended_session_is_adopted_by_the_next_agent() {
    let fleet = Fleet::new("adopt", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "migration"}),
    );
    let session_id = text(&started, "session_id");
    fleet.agent(0).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/step", "content": "half applied"}),
    );
    fleet.agent(0).call(
        "session_end",
        json!({"session_id": session_id, "handoff": "left the migration half applied"}),
    );

    let picked = fleet.agent(1).call(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "pickup",
            "from": {"agent": "agent-one", "name": "migration"},
        }),
    );
    assert_eq!(
        text(&picked, "session_id"),
        session_id,
        "an adopted session keeps its id and its brain"
    );
    let pickup = picked.get("pickup").expect("pickup").clone();
    assert_eq!(pickup["mode"], "adopt");
    assert_eq!(pickup["from_agent"], "agent-one");
    assert_eq!(pickup["handoff"], "left the migration half applied");
    assert_eq!(text(&picked, "agent"), "agent-two");

    let value = fleet
        .agent(1)
        .call("brain_get", json!({"path": "/kv/step"}));
    assert_eq!(value["content"], "half applied");

    let listed = fleet
        .agent(1)
        .call("session_list", json!({"project_id": PROJECT}));
    let sessions = listed["sessions"].as_array().expect("sessions").clone();
    let entry = sessions
        .iter()
        .find(|entry| entry["session_id"] == session_id.as_str())
        .expect("the adopted session is listed");
    assert_eq!(entry["agent"], "agent-two");
    assert_eq!(entry["status"], "active");
    assert_eq!(entry["session_name"], "pickup");
    assert_eq!(entry["adopted_from"], session_id.as_str());

    let feed = fleet
        .agent(1)
        .call("feed_read", json!({"project_id": PROJECT}));
    let adopted = feed["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| {
            event["summary"]
                .as_str()
                .unwrap_or_default()
                .contains("picked up")
        })
        .count();
    assert_eq!(adopted, 1, "one adoption, one event: {feed}");
}

#[tokio::test]
async fn a_connection_that_lost_its_session_stops_writing_to_it() {
    let fleet = Fleet::new("stale-writer", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "long-run"}),
    );
    let session_id = text(&started, "session_id");
    fleet.agent(0).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/step", "content": "one"}),
    );

    // The human ends it from the PWA, so the first agent's connection still
    // believes the session is its own, and the next agent picks the work up.
    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/end"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let picked = fleet.agent(1).call(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "taken-over",
            "from": {"session_id": session_id},
        }),
    );
    assert_eq!(picked["pickup"]["mode"], "adopt", "{picked}");

    // One brain, one writer. The connection that lost the session is told who
    // has it now and what to do, instead of writing on beside the new owner.
    let (code, message) = fleet.agent(0).refusal(
        "brain_put",
        json!({"store": "session", "path": "/kv/step", "content": "clobbered"}),
    );
    assert_eq!(code, "conflict", "{message}");
    assert!(
        message.contains("agent-two") && message.contains("session_start"),
        "the refusal names the new owner and the way forward: {message}"
    );
    let kept = fleet
        .agent(1)
        .call("brain_get", json!({"path": "/kv/step"}));
    assert_eq!(
        kept["content"], "one",
        "the new owner's brain was not written"
    );

    // The same holds when the human reassigns a session that is still active.
    let other = fleet.agent(1).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "second"}),
    );
    let other_id = text(&other, "session_id");
    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{other_id}/reassign"),
        Some(r#"{"agent":"agent-one"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let (code, _) = fleet.agent(1).refusal(
        "brain_put",
        json!({"store": "session", "path": "/kv/x", "content": "late"}),
    );
    assert_eq!(
        code, "conflict",
        "a reassigned session is no longer written by its old owner"
    );
}

#[tokio::test]
async fn ending_a_session_that_never_wrote_creates_no_brain() {
    let fleet = Fleet::new("end-no-brain", &["agent-one"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "quiet"}),
    );
    let session_id = text(&started, "session_id");
    fleet.agent(0).call(
        "session_end",
        json!({"session_id": session_id, "handoff": "nothing to hand over"}),
    );

    assert!(
        !fleet.brain_file(&session_id).exists(),
        "a session that never wrote gets no brain file when it ends"
    );
}

#[tokio::test]
async fn only_one_of_two_adopters_takes_an_ended_session() {
    let fleet = Fleet::new("adopt-race", &["agent-one", "agent-two", "agent-three"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "handover"}),
    );
    let session_id = text(&started, "session_id");
    fleet
        .agent(0)
        .call("session_end", json!({"session_id": session_id}));

    // Two agents reach for one ended session at the same time.
    let (second, third) = (fleet.agent(1).clone(), fleet.agent(2).clone());
    let source = json!({"agent": "agent-one", "name": "handover"});
    let arguments = json!({
        "project_id": PROJECT,
        "session_name": "handover",
        "from": source,
    });
    let one_args = arguments.clone();
    let other_args = arguments.clone();
    let first = std::thread::spawn(move || second.raw("session_start", one_args));
    let other = std::thread::spawn(move || third.raw("session_start", other_args));
    let results = [
        first.join().expect("first adopter"),
        other.join().expect("second adopter"),
    ];

    // Exactly one of them takes the session itself. The other is told what
    // happened one of two ways, and neither is a second adoption: it is
    // refused, or, if it arrives after the first committed, the session it
    // asked for is running and it gets a fork of its own.
    let adopters: Vec<_> = results
        .iter()
        .filter_map(|response| response.pointer("/result/structuredContent"))
        .filter(|result| result["pickup"]["mode"] == "adopt")
        .collect();
    assert_eq!(
        adopters.len(),
        1,
        "one adoption of one ended session: {results:?}"
    );
    assert_eq!(
        adopters[0]["session_id"],
        session_id.as_str(),
        "the adopter holds the source session"
    );
    for result in &results {
        if let Some(error) = result.get("error") {
            assert!(
                error.to_string().contains("owner="),
                "a refusal names the current owner: {error}"
            );
        } else if let Some(other) = result.pointer("/result/structuredContent")
            && other["pickup"]["mode"] != "adopt"
        {
            assert_eq!(other["pickup"]["mode"], "fork");
            assert_ne!(
                other["session_id"],
                session_id.as_str(),
                "the loser never gets the session it did not adopt"
            );
        }
    }

    let listed = fleet
        .agent(0)
        .call("session_list", json!({"project_id": PROJECT}));
    let owners: Vec<String> = listed["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .filter(|entry| entry["session_id"] == session_id.as_str())
        .map(|entry| entry["agent"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(owners.len(), 1, "the session has one row and one owner");

    let feed = fleet
        .agent(0)
        .call("feed_read", json!({"project_id": PROJECT}));
    let adoptions = feed["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| {
            event["summary"]
                .as_str()
                .unwrap_or_default()
                .contains("picked up")
        })
        .count();
    assert_eq!(adoptions, 1, "one adoption, one event");
}

#[tokio::test]
async fn a_running_session_is_forked_and_left_alone() {
    let fleet = Fleet::new("fork", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "live"}),
    );
    let source_id = text(&started, "session_id");
    fleet.agent(0).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/step", "content": "before the fork"}),
    );

    let forked = fleet.agent(1).call(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "branch",
            "from": {"agent": "agent-one", "name": "live"},
        }),
    );
    let fork_id = text(&forked, "session_id");
    assert_ne!(fork_id, source_id, "a fork is a new session");
    assert_eq!(forked["pickup"]["mode"], "fork");
    // A fork looks like an adoption unless it says otherwise, and a caller that
    // asked for finished work has to know the original is still being worked.
    let pickup = &forked["pickup"];
    assert_eq!(pickup["source_active"], true, "{pickup}");
    assert!(
        pickup["note"]
            .as_str()
            .is_some_and(|note| note.contains("copy") && note.contains("agent-one")),
        "the result says this is a copy and who holds the original: {pickup}"
    );
    assert_eq!(forked["pickup"]["from_session_id"], source_id.as_str());
    assert_eq!(
        fleet
            .agent(1)
            .call("brain_get", json!({"path": "/kv/step"}))["content"],
        "before the fork",
        "the fork starts from the source's state"
    );

    // The source is untouched: still active, still its owner's, and its later
    // writes stay out of the fork.
    fleet.agent(0).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/step", "content": "after the fork"}),
    );
    assert_eq!(
        fleet
            .agent(1)
            .call("brain_get", json!({"path": "/kv/step"}))["content"],
        "before the fork",
        "the source's later write does not reach the fork"
    );
    let (code, _) = fleet.agent(1).refusal(
        "brain_get",
        json!({"path": "/kv/step", "session": {"agent": "agent-two", "name": "missing"}}),
    );
    assert_eq!(code, "not_found");

    let listed = fleet
        .agent(1)
        .call("session_list", json!({"project_id": PROJECT}));
    let sessions = listed["sessions"].as_array().expect("sessions");
    let source = sessions
        .iter()
        .find(|entry| entry["session_id"] == source_id.as_str())
        .expect("the source is listed");
    assert_eq!(source["agent"], "agent-one");
    assert_eq!(source["status"], "active");
    assert_eq!(source["session_name"], "live");
    let fork = sessions
        .iter()
        .find(|entry| entry["session_id"] == fork_id.as_str())
        .expect("the fork is listed");
    assert_eq!(fork["forked_from"], source_id.as_str());
    assert_eq!(fork["agent"], "agent-two");

    // The fork's entries are searchable under the fork, and the source keeps
    // its own rows.
    let hits = fleet.agent(1).call(
        "search",
        json!({"query": "before", "project_id": PROJECT, "session_id": fork_id}),
    );
    assert!(
        hits.to_string().contains("/kv/step"),
        "the forked entries are searchable under the fork: {hits}"
    );
    let source_hits = fleet.agent(0).call(
        "search",
        json!({"query": "after", "project_id": PROJECT, "session_id": source_id}),
    );
    assert!(
        source_hits.to_string().contains("/kv/step"),
        "the source keeps its own rows: {source_hits}"
    );
}

#[tokio::test]
async fn a_pickup_into_a_name_the_caller_holds_is_refused() {
    let fleet = Fleet::new("collision", &["agent-one", "agent-two"]).await;

    let held = fleet.agent(1).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "busy"}),
    );
    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "done"}),
    );
    let source_id = text(&started, "session_id");
    fleet
        .agent(0)
        .call("session_end", json!({"session_id": source_id}));

    let (code, message) = fleet.agent(1).refusal(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "busy",
            "from": {"agent": "agent-one", "name": "done"},
        }),
    );
    assert_eq!(code, "conflict");
    assert!(
        message.contains(&format!(
            "existing_session_id={}",
            text(&held, "session_id")
        )),
        "the refusal names the session already holding the name: {message}"
    );

    let listed = fleet
        .agent(1)
        .call("session_list", json!({"project_id": PROJECT}));
    let sessions = listed["sessions"].as_array().expect("sessions");
    let source = sessions
        .iter()
        .find(|entry| entry["session_id"] == source_id.as_str())
        .expect("the source is listed");
    assert_eq!(source["agent"], "agent-one", "nothing moved");
    assert_eq!(source["status"], "ended");
}

#[tokio::test]
async fn only_the_owner_ends_a_session() {
    let fleet = Fleet::new("end-owner", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "mine"}),
    );
    let session_id = text(&started, "session_id");

    let (code, message) = fleet
        .agent(1)
        .refusal("session_end", json!({"session_id": session_id}));
    assert_eq!(code, "forbidden");
    assert!(
        message.contains("owner=agent-one"),
        "the refusal names the owner: {message}"
    );

    let listed = fleet
        .agent(0)
        .call("session_list", json!({"project_id": PROJECT}));
    let entry = listed["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session_id"] == session_id.as_str())
        .expect("the session is listed")
        .clone();
    assert_eq!(entry["status"], "active", "a refused end changes nothing");
}

#[tokio::test]
async fn a_pruned_session_is_not_picked_up() {
    let fleet = Fleet::new("pruned-source", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "old"}),
    );
    let session_id = text(&started, "session_id");
    fleet
        .agent(0)
        .call("session_end", json!({"session_id": session_id}));
    prune(fleet.port, &session_id);

    let (code, message) = fleet.agent(1).refusal(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "pickup",
            "from": {"session_id": session_id},
        }),
    );
    assert_eq!(code, "conflict");
    assert!(
        message.contains(&format!("session_id={session_id}")),
        "the refusal names the pruned session: {message}"
    );
}

#[tokio::test]
async fn a_pickup_from_another_project_is_refused() {
    let fleet = Fleet::new("cross-project", &["agent-one", "agent-two"]).await;
    let other = "workshop";
    create_project(fleet.port, other);

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": other, "session_name": "elsewhere"}),
    );
    let source_id = text(&started, "session_id");
    fleet
        .agent(0)
        .call("session_end", json!({"session_id": source_id}));

    let (code, message) = fleet.agent(1).refusal(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "pickup",
            "from": {"session_id": source_id},
        }),
    );
    assert_eq!(code, "invalid_argument");
    assert!(
        message.contains(&format!("from_project_id={other}")),
        "the refusal names the project the source is in: {message}"
    );
}

#[tokio::test]
async fn a_session_reference_names_one_session() {
    let fleet = Fleet::new("bad-reference", &["agent-one"]).await;

    for reference in [
        json!({}),
        json!({"session_id": "01J", "agent": "agent-one"}),
    ] {
        let (code, _) = fleet.agent(0).refusal(
            "session_start",
            json!({
                "project_id": PROJECT,
                "session_name": "pickup",
                "from": reference,
            }),
        );
        assert_eq!(code, "invalid_argument");
    }

    let (code, _) = fleet.agent(0).refusal(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "pickup",
            "from": {"agent": "agent-one", "name": "never-existed"},
        }),
    );
    assert_eq!(
        code, "not_found",
        "a name that resolves to nothing inside a writable project is plainly missing"
    );
}

#[tokio::test]
async fn a_listing_summarises_a_long_handoff_note() {
    let fleet = Fleet::new("handoff-summary", &["agent-one"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "verbose"}),
    );
    let session_id = text(&started, "session_id");
    let note = "detail ".repeat(60);
    fleet.agent(0).call(
        "session_end",
        json!({"session_id": session_id, "handoff": note}),
    );

    let listed = fleet
        .agent(0)
        .call("session_list", json!({"project_id": PROJECT}));
    let entry = listed["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session_id"] == session_id.as_str())
        .expect("the session is listed")
        .clone();
    let summary = entry["handoff"].as_str().expect("a handoff summary");
    assert_eq!(
        summary.chars().count(),
        200,
        "a listing carries the first 200 characters"
    );
    assert_eq!(
        entry["handoff_truncated"], true,
        "and says the note goes on"
    );

    // The note itself is kept whole for whoever picks the session up.
    let picked = fleet.agent(0).call(
        "session_start",
        json!({
            "project_id": PROJECT,
            "session_name": "verbose",
            "from": {"agent": "agent-one", "name": "verbose"},
        }),
    );
    assert_eq!(picked["pickup"], Value::Null, "the owner simply resumes");
    let ended = fleet
        .agent(0)
        .call("session_list", json!({"project_id": PROJECT, "limit": 1}));
    assert_eq!(ended["truncated"], true, "a full page says so");
}

#[tokio::test]
async fn a_confined_agent_lists_only_the_sessions_it_may_read() {
    let fleet = Fleet::with_trust(
        "confined-listing",
        &[
            ("agent-one", Trust::Trusted),
            ("stranger", Trust::Untrusted),
        ],
    )
    .await;

    fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "shared-work"}),
    );

    // An untrusted agent reaches its own space and nothing else, so the
    // listing it gets back is the one it may read.
    let listed = fleet.agent(1).call("session_list", json!({}));
    assert_eq!(
        listed["sessions"].as_array().expect("sessions").len(),
        0,
        "a confined agent sees none of another project's sessions: {listed}"
    );

    let (code, _) = fleet
        .agent(1)
        .refusal("session_list", json!({"project_id": PROJECT}));
    assert_eq!(
        code, "forbidden",
        "naming a project it may not read is refused"
    );
}

/// One session's recorded last activity, as its owner's listing reports it.
fn last_activity(agent: &Agent, session_id: &str) -> String {
    let listed = agent.call("session_list", json!({"project_id": PROJECT}));
    listed
        .get("sessions")
        .and_then(Value::as_array)
        .expect("sessions")
        .iter()
        .find(|session| session["session_id"] == session_id)
        .map(|session| text(session, "last_activity"))
        .unwrap_or_else(|| panic!("session {session_id} is not listed: {listed}"))
}

#[tokio::test]
async fn an_agent_that_only_posts_signals_still_counts_as_working() {
    let fleet = Fleet::new("activity", &["agent-one"]).await;
    let agent = fleet.agent(0);

    let started = agent.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "triage"}),
    );
    let session_id = text(&started, "session_id");
    let at_start = last_activity(agent, &session_id);

    // A triage agent that never writes a brain is still at work, so the tool
    // it does use has to move the timestamp the active count reads.
    agent.call(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "signal", "summary": "watched something"}),
    );
    let after_signal = last_activity(agent, &session_id);
    assert!(
        after_signal > at_start,
        "posting a signal is work: {at_start} then {after_signal}"
    );
}

#[tokio::test]
async fn a_session_ended_over_mcp_is_offered_for_pruning_at_once() {
    let fleet = Fleet::new("prunable", &["agent-one"]).await;
    let agent = fleet.agent(0);
    let started = agent.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    let session_id = text(&started, "session_id");
    agent.call(
        "brain_put",
        json!({"store": "session", "path": "/kv/note", "content": "something to reclaim"}),
    );

    // Read the number first, so the memo behind it is warm and only an
    // invalidation can move it.
    let prunable = |port| -> Value {
        let (status, body) = admin(port, "GET", "/api/v1/storage", None);
        assert_eq!(status, 200, "storage: {body}");
        serde_json::from_str::<Value>(&body).expect("storage is JSON")["prunable"].clone()
    };
    assert_eq!(prunable(fleet.port)["sessions"], 0);

    agent.call("session_end", json!({"session_id": session_id}));

    let after = prunable(fleet.port);
    assert_eq!(
        after["sessions"], 1,
        "the human sees the session it can reclaim without waiting the memo out: {after}"
    );
    assert!(
        after["bytes"].as_i64().expect("bytes") > 0,
        "and the bytes it would free: {after}"
    );
}
