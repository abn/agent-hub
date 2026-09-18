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

async fn call(state: &AppState, method: &str, uri: &str) -> (StatusCode, Value) {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .method(method)
                .header(header::AUTHORIZATION, "Bearer token")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// An ended session in a project, with a brain file behind it.
async fn ended_session(state: &AppState, project: &str, name: &str) -> sessions::Session {
    let session = sessions::start(&state.db, project, name, "agent-one")
        .await
        .expect("start");
    write_brain(state, &session).await;
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    session
}

#[tokio::test]
async fn pruning_a_project_takes_its_ended_sessions_and_leaves_the_rest() {
    let state = state().await;
    for project in ["proj", "other"] {
        agent_hub::store::projects::create(&state.db, project, "Project")
            .await
            .expect("create project");
    }
    let first = ended_session(&state, "proj", "nightly").await;
    let second = ended_session(&state, "proj", "backfill").await;
    let running = sessions::start(&state.db, "proj", "live", "agent-two")
        .await
        .expect("start");
    let elsewhere = ended_session(&state, "other", "nightly").await;
    state.notify();

    let (status, body) = call(&state, "DELETE", "/api/v1/storage/projects/proj/sessions").await;
    assert_eq!(status, StatusCode::OK);
    let mut pruned: Vec<&str> = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|entry| entry["session_id"].as_str().expect("id"))
        .collect();
    pruned.sort_unstable();
    let mut expected = vec![first.id.as_str(), second.id.as_str()];
    expected.sort_unstable();
    assert_eq!(pruned, expected, "only this project's ended sessions");
    assert!(
        body["undo_expires_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty()),
        "the batch says how long it can be undone for"
    );
    for entry in body["sessions"].as_array().expect("sessions") {
        assert_eq!(entry["undo_token"], entry["session_id"]);
    }

    for session in [&first, &second] {
        assert!(
            sessions::get(&state.db, &session.id)
                .await
                .expect("get")
                .expect("row")
                .deleted_at
                .is_some()
        );
    }
    assert!(
        sessions::get(&state.db, &running.id)
            .await
            .expect("get")
            .expect("row")
            .deleted_at
            .is_none(),
        "an active session is never pruned"
    );
    assert!(
        sessions::get(&state.db, &elsewhere.id)
            .await
            .expect("get")
            .expect("row")
            .deleted_at
            .is_none(),
        "another project's ended session is not this project's to prune"
    );
}

#[tokio::test]
async fn undoing_a_batch_restores_every_session_in_it() {
    let state = state().await;
    let first = ended_session(&state, "proj", "nightly").await;
    let second = ended_session(&state, "proj", "backfill").await;

    let (_, body) = call(&state, "DELETE", "/api/v1/storage/sessions").await;
    let tokens: Vec<String> = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|entry| entry["undo_token"].as_str().expect("token").to_string())
        .collect();
    assert_eq!(tokens.len(), 2);
    assert!(
        sessions::list(&state.db, "proj")
            .await
            .expect("list")
            .is_empty()
    );

    for token in &tokens {
        let (status, _) = call(&state, "POST", &format!("/api/v1/prune/undo/{token}")).await;
        assert_eq!(status, StatusCode::OK);
    }

    let restored = sessions::list(&state.db, "proj").await.expect("list");
    assert_eq!(restored.len(), 2, "undo puts the whole batch back");
    for session in [&first, &second] {
        assert!(restored.iter().any(|row| row.id == session.id));
    }
}

#[tokio::test]
async fn the_sweep_commits_every_session_a_batch_pruned() {
    let state = state().await;
    let first = ended_session(&state, "proj", "nightly").await;
    let second = ended_session(&state, "proj", "backfill").await;
    call(&state, "DELETE", "/api/v1/storage/sessions").await;

    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE deleted_at IS NOT NULL",
        vec![turso::Value::Text(
            old.format(&time::format_description::well_known::Rfc3339)
                .expect("format"),
        )],
    )
    .await
    .expect("age");

    let committed = prune::sweep(&state.db, &state.data_dir)
        .await
        .expect("sweep");
    assert_eq!(committed, 2);
    for session in [&first, &second] {
        assert!(
            sessions::get(&state.db, &session.id)
                .await
                .expect("get")
                .is_none()
        );
        assert!(
            !state.data_dir.join(&session.brain_path).exists(),
            "the brain file goes with the row"
        );
    }
}

#[tokio::test]
async fn pruning_a_project_with_nothing_ended_removes_nothing() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("create project");
    sessions::start(&state.db, "proj", "live", "agent-one")
        .await
        .expect("start");

    let (status, body) = call(&state, "DELETE", "/api/v1/storage/projects/proj/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["sessions"].as_array().expect("sessions").len(),
        0,
        "nothing to reclaim is not an error"
    );
    assert_eq!(
        sessions::list(&state.db, "proj").await.expect("list").len(),
        1
    );
}

#[tokio::test]
async fn pruning_a_project_that_does_not_exist_says_so() {
    let state = state().await;
    // A typo must read as "no such project", the way the other project routes
    // read it, not as "nothing to reclaim".
    let (status, _) = call(
        &state,
        "DELETE",
        "/api/v1/storage/projects/missing/sessions",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_batch_prune_without_a_token_is_refused() {
    let state = state().await;
    for uri in [
        "/api/v1/storage/sessions",
        "/api/v1/storage/projects/proj/sessions",
    ] {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .method("DELETE")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn home_carries_the_counts_its_summary_line_and_cards_show() {
    let state = state().await;
    let ended = ended_session(&state, "proj", "nightly").await;
    let brain_bytes = agent_hub::brain::file_bytes(&state.data_dir.join(&ended.brain_path));
    // One agent with two live sessions, and a second agent with one.
    for name in ["live", "second"] {
        let session = sessions::start(&state.db, "proj", name, "agent-two")
            .await
            .expect("start");
        state
            .activity
            .touch(&state.db, &session.id)
            .await
            .expect("touch");
    }
    let other = sessions::start(&state.db, "proj", "live", "agent-three")
        .await
        .expect("start");
    state
        .activity
        .touch(&state.db, &other.id)
        .await
        .expect("touch");
    state.notify();

    let (status, body) = call(&state, "GET", "/api/v1/home").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["agents_active"], 2,
        "an agent with two sessions counts once, an ended one not at all: {body}"
    );
    assert_eq!(body["prunable"]["sessions"], 1);
    assert_eq!(body["prunable"]["bytes"], brain_bytes);
    assert!(body["storage"]["used_bytes"].as_i64().expect("used") > 0);
    assert!(
        body["storage"]["capacity_bytes"]
            .as_i64()
            .expect("capacity")
            >= body["storage"]["free_bytes"].as_i64().expect("free")
    );
    assert_eq!(
        body["last_event_at"], body["recent"][0]["created_at"],
        "the empty state's last event time is the newest event's own"
    );
    assert!(body["unread"].is_number() && body["waiting"].is_number());
}

#[tokio::test]
async fn an_agent_that_has_gone_quiet_is_not_counted_as_active() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET last_activity = '2026-09-16T00:00:00Z' WHERE id = ?1",
        vec![turso::Value::Text(session.id.clone())],
    )
    .await
    .expect("age the activity");

    let (_, body) = call(&state, "GET", "/api/v1/home").await;
    assert_eq!(
        body["agents_active"], 0,
        "a session last touched long ago makes nobody active"
    );
}
