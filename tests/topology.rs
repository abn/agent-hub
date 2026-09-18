//! One process serves the REST API, the PWA, MCP, and the prune sweeper on one
//! listener.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::brain::BrainStore;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::principal::Trust;
use agent_hub::store::{identity, migrate, open_engine, prune, sessions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value as Json;
use tower::ServiceExt;
use turso::Value;

const ADMIN_TOKEN: &str = "topology-admin-token";
const PROTOCOL_VERSION: &str = "2025-06-18";

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
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
        .env("HUB_SWEEP_INTERVAL_SECS", "1")
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

fn request(port: u16, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");

    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream.write_all(request.as_bytes()).expect("write request");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read response");
    let response = String::from_utf8_lossy(&bytes).into_owned();
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, response)
}

/// A real MCP initialize over streamable HTTP, returning the status and body.
fn mcp_initialize(port: u16, token: &str) -> (u16, String) {
    let body = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\
         \"protocolVersion\":\"{PROTOCOL_VERSION}\",\"capabilities\":{{}},\
         \"clientInfo\":{{\"name\":\"topology\",\"version\":\"0.0.0\"}}}}}}"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nAuthorization: Bearer {token}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write request");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read response");
    let response = String::from_utf8_lossy(&bytes).into_owned();
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, response)
}

#[tokio::test]
async fn one_process_serves_the_api_pwa_mcp_and_sweeper() {
    let dir = temp_dir("topology");
    let port = free_port();

    // Seed an expired prune with a brain file, so the sweeper has something to
    // commit as soon as the server starts.
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    let agent = identity::create_agent(&db, "probe", "Probe", Trust::Untrusted)
        .await
        .expect("create agent");
    let agent_token = identity::issue_token(&db, &agent.id)
        .await
        .expect("issue token")
        .token;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start session");
    let store = BrainStore::new(dir.join("sessions"));
    let brain = store.open("proj", &session.id).await.expect("open brain");
    brain.put("/kv/note", b"to be pruned").await.expect("put");
    let brain_file = dir.join(&session.brain_path);
    assert!(brain_file.exists());
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end session");
    prune::prune_session(&db, &session.id)
        .await
        .expect("prune session");
    let old = (time::OffsetDateTime::now_utc() - time::Duration::seconds(120))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format time");
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
        vec![Value::Text(old), Value::Text(session.id.clone())],
    )
    .await
    .expect("age the tombstone");
    drop(conn);
    drop(db);

    let child = spawn(&dir, port);
    wait_for_port(port);

    let (status, health) = request(port, "GET", "/healthz", None, "");
    assert_eq!(status, 200);
    assert!(health.contains("ok"));

    let (status, ready) = request(port, "GET", "/readyz", None, "");
    assert_eq!(status, 200);
    assert!(ready.contains("schema_version"));

    let (status, pwa) = request(port, "GET", "/", None, "");
    assert_eq!(status, 200, "the PWA is on the listener");
    assert!(pwa.contains("skip-link"));

    let (status, _) = request(port, "GET", "/api/v1/projects", Some(ADMIN_TOKEN), "");
    assert_eq!(status, 200, "the REST API is on the same listener");
    let (status, _) = request(port, "GET", "/api/v1/projects", None, "");
    assert_eq!(status, 401);

    let (status, _) = mcp_initialize(port, "");
    assert_eq!(status, 401, "MCP is gated on the same listener");
    let (status, admin_body) = mcp_initialize(port, ADMIN_TOKEN);
    assert_eq!(status, 200, "admin reaches MCP: {admin_body}");
    assert!(admin_body.contains("serverInfo"));
    let (status, agent_body) = mcp_initialize(port, &agent_token);
    assert_eq!(status, 200, "an agent token reaches MCP: {agent_body}");
    let (status, _) = request(port, "GET", "/api/v1/projects", Some(&agent_token), "");
    assert_eq!(status, 401, "an agent token is rejected on REST");

    // The sweeper runs in this process: the aged tombstone commits and the
    // brain file is removed.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && brain_file.exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        !brain_file.exists(),
        "the sweeper committed the expired prune"
    );

    drop(child);
    std::fs::remove_dir_all(&dir).ok();
}

/// State over a fresh data directory, for the in-process probe tests.
async fn probe_state(tag: &str) -> AppState {
    AppState::open(Config {
        data_dir: temp_dir(tag),
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some(ADMIN_TOKEN.to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
    })
    .await
    .expect("open state")
}

async fn probe(state: AppState) -> (StatusCode, Option<String>, Json) {
    let request = Request::builder()
        .uri("/readyz")
        .body(Body::empty())
        .expect("build request");
    let response = router(state).oneshot(request).await.expect("request");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (
        status,
        content_type,
        serde_json::from_slice(&bytes).expect("json body"),
    )
}

#[tokio::test]
async fn readyz_reports_ready_when_the_engine_answers() {
    let state = probe_state("topology-ready").await;
    let schema_version = state.schema_version;

    let (status, _, body) = probe(state).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["schema_version"], schema_version);
}

#[tokio::test]
async fn readyz_reports_unavailable_when_the_engine_does_not_answer() {
    let state = probe_state("topology-unready").await;

    // The store stops answering the readiness query, as it would if the data
    // volume went away under a running process.
    let conn = state.db.connect().expect("connect");
    conn.execute("DROP TABLE schema_version", ())
        .await
        .expect("drop the version table");
    drop(conn);

    let (status, content_type, body) = probe(state).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["code"], "unavailable");
}

#[tokio::test]
async fn readyz_reports_unavailable_when_the_store_moved_schema() {
    let state = probe_state("topology-unready-schema").await;
    let opened = state.schema_version;

    // Another process migrated the same store: the engine still answers, but
    // not at the version this process opened.
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "INSERT INTO schema_version(version) VALUES (?1)",
        (opened + 1,),
    )
    .await
    .expect("record a later version");
    drop(conn);

    let (status, content_type, body) = probe(state).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["code"], "unavailable");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains(&(opened + 1).to_string()) && detail.contains(&opened.to_string()),
        "the problem names both versions: {detail}"
    );
}
