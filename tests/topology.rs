//! One process serves the REST API, the PWA, MCP, and the prune sweeper on one
//! listener.

use std::time::{Duration, Instant};

use agent_hub::app::AppState;
use agent_hub::brain::BrainStore;
use agent_hub::http::router;
use agent_hub::store::{identity, migrate, open_engine, prune, schema, sessions};
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
    let agent = identity::create_agent(&db, "probe", "Probe")
        .await
        .expect("create agent");
    let agent_token = identity::issue_token(&db, &agent.id)
        .await
        .expect("issue token")
        .token;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start session");
    let store = BrainStore::new(dir.join("sessions"));
    let brain = store.open("proj", &session.id).await.expect("open brain");
    brain.put("/kv/note", b"to be pruned").await.expect("put");
    let brain_file = dir.join(&session.brain_path);
    assert!(brain_file.exists());
    sessions::end(&db, &session.id, "agent-one", None, None)
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
    let (status, _) = request(port, "GET", "/api/v1/agents", Some(&agent_token), "");
    assert_eq!(
        status, 401,
        "an agent token is rejected on REST admin control surface"
    );

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
    assert_eq!(
        body["supported_max"],
        schema::SUPPORTED_MAX,
        "the probe says how far this binary can go"
    );
    assert!(
        body["free_bytes"].as_i64().is_some_and(|free| free > 0),
        "the probe reports the room on the volume: {body}"
    );
}

#[tokio::test]
async fn readyz_reports_unavailable_when_the_store_path_is_gone() {
    let state = probe_state("topology-removed-store").await;

    // The process keeps the file descriptor, so the engine still answers the
    // version query. The path is what a restart, a backup or an operator
    // walks, and it is gone: readiness must say so. (The inode stays alive
    // while the descriptor is held, so an inode-only check would not.)
    std::fs::remove_file(state.dir().join("hub.db")).expect("remove the store path");

    let (status, content_type, body) = probe(&state).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["code"], "unavailable");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("hub store") && detail.contains("gone"),
        "the problem names the missing path: {detail}"
    );
}

#[tokio::test]
async fn readyz_reports_unavailable_when_the_store_file_is_replaced() {
    let state = probe_state("topology-replaced-store").await;

    // A different file is renamed over the path. The process keeps its
    // descriptor on the original, so the version query still answers; the
    // captured device and inode are what tell the two apart.
    let hub = state.dir().join("hub.db");
    let other = state.dir().join("replacement.db");
    std::fs::write(&other, b"not this hub").expect("write a replacement");
    std::fs::rename(&other, &hub).expect("replace the store path");

    let (status, content_type, body) = probe(&state).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["code"], "unavailable");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("hub store") && detail.contains("replaced"),
        "the problem says the store was replaced: {detail}"
    );
}

#[tokio::test]
async fn migrate_refuses_a_store_newer_than_the_binary() {
    let dir = TempDir::new("topology-newer-store");
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db)
        .await
        .expect("migrate to the supported version");

    // A later binary wrote a schema this one has never seen.
    let conn = db.connect().expect("connect");
    conn.execute(
        "INSERT INTO schema_version(version) VALUES (?1)",
        (schema::SUPPORTED_MAX + 1,),
    )
    .await
    .expect("record a newer version");
    drop(conn);

    let refused = migrate(&db)
        .await
        .expect_err("the startup migration refuses a newer store");
    let message = refused.to_string();
    assert!(
        message.contains(&(schema::SUPPORTED_MAX + 1).to_string())
            && message.contains(&schema::SUPPORTED_MAX.to_string()),
        "the refusal names both versions: {message}"
    );
}

/// A store that has migrations pending is copied aside first; one that is
/// already current is not. The copy is per migration, not per start.
#[tokio::test]
async fn a_second_start_writes_no_pre_migration_backup() {
    let dir = TempDir::new("topology-backup-once");
    let config = common::state::config(&dir);

    let state = AppState::open(config.clone()).await.expect("first open");
    let first = backup_names(dir.path());
    assert_eq!(
        first.len(),
        1,
        "a fresh store is copied aside once before migrating: {first:?}"
    );
    drop(state);

    let state = AppState::open(config).await.expect("second open");
    assert_eq!(
        backup_names(dir.path()),
        first,
        "an up-to-date store writes no backup on start"
    );
    drop(state);
}

/// The pre-migration backup files present in `data_dir`.
fn backup_names(data_dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data_dir.join("backups"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry.file_name().to_str().is_some_and(|name| {
                        name.starts_with("pre-migration-") && name.ends_with(".db")
                    })
                })
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
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
