//! Trust enforcement over MCP: an agent reaches what its trust and grants
//! allow, and the actor is the authenticated identity.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_hub::principal::Trust;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{identity, migrate, open_engine, projects};
use serde_json::json;

const PROTOCOL_VERSION: &str = "2025-06-18";
const ADMIN_TOKEN: &str = "mcp-identity-admin";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-mcp-identity-{}-{nanos}-{tag}",
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

struct HttpResponse {
    status: u16,
    raw: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<String> {
        self.raw
            .lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
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
        .expect("failed to spawn the hub");
    ChildGuard(child)
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the hub did not start on port {port}");
}

fn http_post(port: u16, body: &str, token: Option<&str>, session: Option<&str>) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_millis(1500)))
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

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut raw = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => raw.push_str(&String::from_utf8_lossy(&buffer[..n])),
            Err(_) => break,
        }
    }
    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    HttpResponse { status, raw }
}

fn initialize(port: u16, token: &str) -> String {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-identity", "version": "0.0.0"},
        },
    })
    .to_string();
    let response = http_post(port, &body, Some(token), None);
    assert_eq!(response.status, 200, "initialize: {}", response.raw);
    let session = response.header("mcp-session-id").expect("session id");
    let ready = http_post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(token),
        Some(&session),
    );
    assert_eq!(ready.status, 202, "initialized: {}", ready.raw);
    session
}

fn call(
    port: u16,
    token: &str,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> HttpResponse {
    http_post(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string(),
        Some(token),
        Some(session),
    )
}

fn signal(project_id: &str, summary: &str) -> NewEvent {
    NewEvent {
        project_id: project_id.to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
    }
}

#[tokio::test]
async fn an_agent_reaches_only_what_its_trust_allows() {
    let data_dir = TempDir::new("trust");

    let db = open_engine(&data_dir.0.join("hub.db"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    let strict = identity::create_agent(&db, "strict", "Strict", Trust::Untrusted)
        .await
        .expect("create strict");
    identity::create_agent(&db, "trust", "Trust", Trust::Trusted)
        .await
        .expect("create trust");
    projects::create(&db, "shared", "Shared")
        .await
        .expect("shared project");
    let strict_token = identity::issue_token(&db, "strict")
        .await
        .expect("strict token")
        .token;
    let trust_token = identity::issue_token(&db, "trust")
        .await
        .expect("trust token")
        .token;
    events::append(&db, "human", None, signal("shared", "needle in shared"))
        .await
        .expect("shared event");
    events::append(
        &db,
        "human",
        None,
        signal(&strict.personal_project_id, "needle in personal"),
    )
    .await
    .expect("personal event");
    drop(db);

    let port = free_port();
    let _child = spawn(&data_dir.0, port);
    wait_for_port(port);

    let strict_session = initialize(port, &strict_token);

    let own = call(
        port,
        &strict_token,
        &strict_session,
        "signal_append",
        json!({"project_id": strict.personal_project_id.clone(), "kind": "signal", "summary": "mine"}),
    );
    assert_eq!(
        own.status, 200,
        "an untrusted agent writes its own space: {}",
        own.raw
    );
    assert!(!own.raw.contains("forbidden"), "{}", own.raw);
    let own_feed = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": strict.personal_project_id.clone()}),
    );
    assert!(
        own_feed.raw.contains("mine"),
        "the write lands in the agent's own space: {}",
        own_feed.raw
    );

    let stranger = call(
        port,
        &strict_token,
        &strict_session,
        "signal_append",
        json!({"project_id": "shared", "kind": "signal", "summary": "not mine"}),
    );
    assert!(
        stranger.raw.contains("forbidden"),
        "an untrusted agent cannot write a shared project: {}",
        stranger.raw
    );

    let read = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": "shared"}),
    );
    assert!(
        read.raw.contains("forbidden"),
        "an untrusted agent cannot read a shared project: {}",
        read.raw
    );

    let missing = call(
        port,
        &strict_token,
        &strict_session,
        "feed_read",
        json!({"project_id": "ghost"}),
    );
    assert!(
        missing.raw.contains("forbidden"),
        "a missing project is forbidden, not a not-found oracle: {}",
        missing.raw
    );
    assert!(
        !missing.raw.contains("not_found"),
        "the error code does not reveal whether a project exists: {}",
        missing.raw
    );
    assert!(
        missing.raw.contains("not found or not permitted"),
        "a missing project carries the shared denial message: {}",
        missing.raw
    );

    // The denied existing project and the missing one must be indistinguishable.
    assert!(
        read.raw.contains("not found or not permitted"),
        "a denied project carries the same message as a missing one: {}",
        read.raw
    );
    assert!(
        !read.raw.contains("may not"),
        "the denial does not name the project or the access: {}",
        read.raw
    );

    let missing_artifact = call(
        port,
        &strict_token,
        &strict_session,
        "artifact_get",
        json!({"artifact_id": "ghost-artifact"}),
    );
    assert!(
        missing_artifact.raw.contains("forbidden"),
        "a missing artifact is forbidden for an agent: {}",
        missing_artifact.raw
    );
    assert!(
        missing_artifact.raw.contains("not found or not permitted"),
        "a missing artifact carries the shared denial message: {}",
        missing_artifact.raw
    );

    let me = call(port, &strict_token, &strict_session, "whoami", json!({}));
    assert!(
        me.raw.contains(&strict.personal_project_id),
        "whoami reports the personal space: {}",
        me.raw
    );

    let scoped = call(
        port,
        &strict_token,
        &strict_session,
        "search",
        json!({"query": "needle", "scope": "global"}),
    );
    assert!(
        scoped.raw.contains("needle in personal"),
        "a scoped search finds its own content: {}",
        scoped.raw
    );
    assert!(
        !scoped.raw.contains("needle in shared"),
        "a scoped search does not leak another project: {}",
        scoped.raw
    );

    let trust_session = initialize(port, &trust_token);
    let shared_read = call(
        port,
        &trust_token,
        &trust_session,
        "feed_read",
        json!({"project_id": "shared"}),
    );
    assert_eq!(
        shared_read.status, 200,
        "a trusted agent reads a shared project: {}",
        shared_read.raw
    );
    assert!(
        shared_read.raw.contains("needle in shared"),
        "the shared event is visible: {}",
        shared_read.raw
    );

    let write = call(
        port,
        &trust_token,
        &trust_session,
        "signal_append",
        json!({
            "project_id": "shared",
            "kind": "signal",
            "summary": "trusted write",
            "actor": "attacker",
            "agent": "attacker",
        }),
    );
    assert_eq!(write.status, 200, "{}", write.raw);
    let after = call(
        port,
        &trust_token,
        &trust_session,
        "feed_read",
        json!({"project_id": "shared"}),
    );
    assert!(
        after.raw.contains("trusted write"),
        "the event is on the feed: {}",
        after.raw
    );
    assert!(
        !after.raw.contains("attacker"),
        "client identity fields are ignored: {}",
        after.raw
    );
    assert!(
        after.raw.contains("\"trust\""),
        "the recorded actor is the token identity: {}",
        after.raw
    );

    drop(_child);
    drop(data_dir);
}
