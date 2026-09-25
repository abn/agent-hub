//! Session lineage tests: artifacts recording caller session, feed and artifact
//! session filtering, and MCP parity.

use agent_hub::http::router;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{artifacts, prune, sessions};
use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

mod common;

use common::http::{get, json_body};
use common::state::TestState;
use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

async fn state() -> TestState {
    let state = common::state::open("session-lineage").await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Project").await;
    state
}

#[tokio::test]
async fn feed_filters_by_session_and_combines_with_kind() {
    let state = state().await;
    let s1 = sessions::start(&state.db, "proj", "sess-1", "agent-one")
        .await
        .expect("start s1");
    let s2 = sessions::start(&state.db, "proj", "sess-2", "agent-two")
        .await
        .expect("start s2");

    // Event in session 1 (signal)
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "s1 signal".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: Some(s1.id.clone()),
        },
    )
    .await
    .expect("append");

    // Event in session 1 (finished)
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "finished".to_string(),
            summary: "s1 finished".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: Some(s1.id.clone()),
        },
    )
    .await
    .expect("append");

    // Event in session 2 (signal)
    events::append(
        &state.db,
        "agent-two",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "s2 signal".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: Some(s2.id.clone()),
        },
    )
    .await
    .expect("append");

    let app = router(state.clone());
    let res = app
        .oneshot(get(
            &format!("/api/v1/feed?session={}", s1.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    let events = body["events"].as_array().expect("events array");
    assert_eq!(
        events.len(),
        3,
        "only session 1 events are returned (start + 2 appended)"
    );

    // Combining with kind filter
    let app = router(state.clone());
    let res = app
        .oneshot(get(
            &format!("/api/v1/feed?session={}&kinds=signal", s1.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1, "session 1 signal returned");
    assert_eq!(events[0]["summary"], "s1 signal");
}

#[tokio::test]
async fn feed_and_artifacts_return_empty_for_unknown_or_pruned_session() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "to-prune", "agent-one")
        .await
        .expect("start");
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "pruned signal".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: Some(session.id.clone()),
        },
    )
    .await
    .expect("append");

    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");

    // Unknown session feed
    let app = router(state.clone());
    let res = app
        .oneshot(get(
            "/api/v1/feed?session=nonexistent",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["events"].as_array().expect("events").len(), 0);

    // Pruned session feed
    let app = router(state.clone());
    let res = app
        .oneshot(get(
            &format!("/api/v1/feed?session={}", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["events"].as_array().expect("events").len(), 0);

    // Unknown session artifacts
    let app = router(state.clone());
    let res = app
        .oneshot(get(
            "/api/v1/artifacts?session=nonexistent",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["artifacts"].as_array().expect("artifacts").len(), 0);

    // Pruned session artifacts
    let app = router(state.clone());
    let res = app
        .oneshot(get(
            &format!("/api/v1/artifacts?session={}", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["artifacts"].as_array().expect("artifacts").len(), 0);
}

#[test]
fn mcp_artifact_session_lineage_and_parities() {
    let data_dir = TempDir::new("mcp-lineage");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = McpServer::mcp(data_dir.path(), &[("HUB_AGENT_ID", "agent-one")]);
    server.initialize();

    // Publish outside session
    let outside = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Outside",
            "kind": "markdown",
            "content": "# Outside",
        }),
    );
    let outside_id = structured(&outside)["artifact_id"]
        .as_str()
        .expect("outside id")
        .to_string();

    // Start session
    let started = server.call_tool(
        "session_start",
        json!({
            "project_id": "proj",
            "session_name": "worker",
        }),
    );
    let session_id = structured(&started)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    // Publish inside session
    let inside = server.call_tool(
        "artifact_publish",
        json!({
            "project_id": "proj",
            "title": "Inside",
            "kind": "markdown",
            "content": "# Inside",
            // Attempt to forge another session id: must be ignored/refused
            "session_id": "forged-id-999",
        }),
    );
    let inside_id = structured(&inside)["artifact_id"]
        .as_str()
        .expect("inside id")
        .to_string();

    // List artifacts for session via MCP
    let listed = server.call_tool(
        "artifact_list",
        json!({
            "project_id": "proj",
            "session": session_id,
        }),
    );
    let artifacts = structured(&listed)["artifacts"]
        .as_array()
        .expect("artifacts");
    assert_eq!(artifacts.len(), 1, "only the session's artifact is listed");
    assert_eq!(artifacts[0]["id"], inside_id);
    assert_eq!(
        artifacts[0]["session_id"], session_id,
        "caller session is recorded, not forged id"
    );
    assert!(
        !artifacts.iter().any(|a| a["id"] == outside_id),
        "outside artifact is not listed for session"
    );

    // List artifacts for unknown session returns empty list
    let empty_listed = server.call_tool(
        "artifact_list",
        json!({
            "project_id": "proj",
            "session": "nonexistent",
        }),
    );
    assert_eq!(
        structured(&empty_listed)["artifacts"]
            .as_array()
            .expect("artifacts")
            .len(),
        0
    );

    // Feed read for session via MCP
    let feed = server.call_tool(
        "feed_read",
        json!({
            "project_id": "proj",
            "session": session_id,
        }),
    );
    let events = structured(&feed)["events"].as_array().expect("events");
    assert!(
        events.iter().any(|e| e["summary"] == "published Inside"),
        "feed shows artifact event for session"
    );
    assert!(
        !events.iter().any(|e| e["summary"] == "published Outside"),
        "outside event not in session feed"
    );
}

#[tokio::test]
async fn pruning_preserves_artifacts_and_search_index() {
    let dir = TempDir::new("prune-artifacts");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    let session = sessions::start(&db, "proj", "sess-art", "agent-one")
        .await
        .expect("start");

    let art = artifacts::publish(
        &db,
        &dir,
        artifacts::NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Durable Report",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"# Important conclusions",
            envelope: None,
            session_id: Some(&session.id),
        },
        None,
    )
    .await
    .expect("publish");

    // Prune session
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end");
    prune::prune_session(&db, &session.id).await.expect("prune");

    // Age tombstone past undo window
    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
        vec![
            turso::Value::Text(
                old.format(&time::format_description::well_known::Rfc3339)
                    .expect("format"),
            ),
            turso::Value::Text(session.id.clone()),
        ],
    )
    .await
    .expect("age");

    let swept = prune::sweep(&db, &dir).await.expect("sweep");
    assert_eq!(swept, 1);

    // Artifact row must still exist
    let meta = artifacts::metadata(&db, &art.id).await.expect("metadata");
    assert_eq!(meta.id, art.id);
    assert_eq!(meta.session_id.as_deref(), Some(session.id.as_str()));

    // Search doc for artifact must still exist
    let mut rows = conn
        .query(
            "SELECT doc_id FROM search_docs WHERE doc_id = ?1",
            vec![turso::Value::Text(format!("artifact:{}", art.id))],
        )
        .await
        .expect("query search_docs");
    assert!(
        rows.next().await.expect("row").is_some(),
        "artifact search doc was not pruned"
    );
}
