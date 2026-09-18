//! Storage route: what the volume holds, what a prune would reclaim, and the
//! node the human is looking at.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::{prune, sessions};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-storage-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: Some("node-under-test".to_string()),
    })
    .await
    .expect("open state")
}

async fn usage(state: &AppState) -> Value {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/storage")
                .method("GET")
                .header(header::AUTHORIZATION, "Bearer token")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

/// Give a session a brain file with something in it, and report its bytes.
async fn write_brain(state: &AppState, session: &sessions::Session) -> i64 {
    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain.put("/kv/note", &vec![b'x'; 4096]).await.expect("put");
    let bytes = agent_hub::brain::file_bytes(&state.data_dir.join(&session.brain_path));
    assert!(bytes > 0, "the brain file is on disk");
    bytes
}

#[tokio::test]
async fn the_volume_numbers_are_measured_and_consistent() {
    let state = state().await;
    let body = usage(&state).await;

    let capacity = body["capacity_bytes"].as_i64().expect("a capacity");
    let free = body["free_bytes"].as_i64().expect("free space");
    assert!(capacity > 0, "the volume has a size");
    assert!(free >= 0 && free <= capacity, "{free} of {capacity}");
    assert_eq!(
        body["data_path"].as_str().expect("a data path"),
        state.data_dir.display().to_string()
    );
    assert_eq!(body["node"]["host"], "node-under-test");
    assert_eq!(body["node"]["mode"], "local");
}

#[tokio::test]
async fn a_data_directory_that_cannot_be_measured_is_reported_as_unknown() {
    let state = state().await;
    // The store is already open, so the rows still answer; only the volume
    // behind the path is gone.
    std::fs::remove_dir_all(&state.data_dir).expect("remove the data directory");

    let body = usage(&state).await;
    assert!(
        body["capacity_bytes"].is_null() && body["free_bytes"].is_null(),
        "an unmeasurable volume is unknown, never zero: {body}"
    );
    assert!(body["projects"].is_array(), "the rest is still served");
}

#[tokio::test]
async fn ending_a_session_moves_its_bytes_into_what_a_prune_would_reclaim() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let brain_bytes = write_brain(&state, &session).await;

    let before = usage(&state).await;
    assert_eq!(before["prunable"]["sessions"], 0);
    assert_eq!(before["prunable"]["bytes"], 0);
    assert_eq!(before["by_kind"]["sessions"], brain_bytes);
    assert_eq!(before["total_bytes"], brain_bytes);
    assert!(
        before["by_kind"]["events"].as_i64().expect("events") > 0,
        "the hub store is on the volume too"
    );
    assert_eq!(
        before["used_bytes"].as_i64().expect("used"),
        before["total_bytes"].as_i64().expect("total")
            + before["by_kind"]["events"].as_i64().expect("events")
    );

    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    state.notify();

    let after = usage(&state).await;
    assert_eq!(after["prunable"]["sessions"], 1);
    assert_eq!(
        after["prunable"]["bytes"], brain_bytes,
        "the hint names the bytes that session would give back"
    );
    assert_eq!(after["projects"][0]["project_id"], "proj");
    assert_eq!(after["projects"][0]["prunable_sessions"], 1);
    assert_eq!(after["projects"][0]["prunable_bytes"], brain_bytes);
}

#[tokio::test]
async fn a_pruned_session_leaves_the_report_at_once() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    write_brain(&state, &session).await;
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    state.notify();
    assert_eq!(usage(&state).await["prunable"]["sessions"], 1);

    prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");
    state.notify();

    let after = usage(&state).await;
    assert_eq!(
        after["prunable"]["sessions"], 0,
        "a session already pruned is not offered again"
    );
    assert_eq!(after["total_bytes"], 0);
}

#[tokio::test]
async fn a_write_invalidates_the_memo_rather_than_waiting_it_out() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    assert_eq!(usage(&state).await["total_bytes"], 0);

    let brain_bytes = write_brain(&state, &session).await;
    state.notify();

    assert_eq!(
        usage(&state).await["total_bytes"],
        brain_bytes,
        "the number a surface reads after a write is the new one"
    );
}

#[tokio::test]
async fn usage_without_a_token_is_refused() {
    let state = state().await;
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/storage")
                .method("GET")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
