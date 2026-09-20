//! One process serves the REST API, the PWA, MCP, and the prune sweeper on one
//! listener.

use std::time::{Duration, Instant};

use agent_hub::app::AppState;
use agent_hub::brain::BrainStore;
use agent_hub::http::router;
use agent_hub::principal::Trust;
use agent_hub::store::{identity, migrate, open_engine, prune, sessions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value as Json;
use tower::ServiceExt;
use turso::Value;

mod common;

use common::process::HubProcess;
use common::state::TestState;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire;

const ADMIN_TOKEN: &str = "topology-admin-token";
fn request(port: u16, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let body = (!body.is_empty()).then_some(body);
    let response = wire::rest(port, method, path, token, body);
    (response.status, response.raw)
}

/// A real MCP initialize over streamable HTTP, returning the status and body.
fn mcp_initialize(port: u16, token: &str) -> (u16, String) {
    let body = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\
         \"protocolVersion\":\"{PROTOCOL_VERSION}\",\"capabilities\":{{}},\
         \"clientInfo\":{{\"name\":\"topology\",\"version\":\"0.0.0\"}}}}}}"
    );
    let response = wire::mcp_post(port, &body, Some(token), None);
    (response.status, response.raw)
}

#[tokio::test]
async fn one_process_serves_the_api_pwa_mcp_and_sweeper() {
    let dir = TempDir::new("topology");
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

    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[("HUB_SWEEP_INTERVAL_SECS", "1")]);
    let port = child.port();

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
}

/// State over a fresh data directory, for the in-process probe tests.
async fn probe_state(tag: &str) -> TestState {
    common::state::open_with(tag, |config| {
        config.admin_token = Some(ADMIN_TOKEN.to_string());
    })
    .await
}

async fn probe(state: &AppState) -> (StatusCode, Option<String>, Json) {
    let request = Request::builder()
        .uri("/readyz")
        .body(Body::empty())
        .expect("build request");
    let response = router(state.clone())
        .oneshot(request)
        .await
        .expect("request");
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

    let (status, _, body) = probe(&state).await;
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

    let (status, content_type, body) = probe(&state).await;
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

    let (status, content_type, body) = probe(&state).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["code"], "unavailable");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains(&(opened + 1).to_string()) && detail.contains(&opened.to_string()),
        "the problem names both versions: {detail}"
    );
}
