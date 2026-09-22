//! HTTP session routes: listing, ending, auth, and problem responses.

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::sessions;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::http::{json_body, problem_body};
use common::state::TestState;

async fn state() -> TestState {
    common::state::open("http-sessions").await
}

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    common::http::request(method, uri, auth, None)
}

#[tokio::test]
async fn list_without_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/sessions?project=proj", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
}

#[tokio::test]
async fn list_requires_a_project() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/sessions", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn list_returns_the_project_sessions() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    let listed = body["sessions"].as_array().expect("sessions array");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], session.id);
    assert_eq!(listed[0]["status"], "active");
}

#[tokio::test]
async fn end_marks_the_session_ended() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/sessions/{}/end", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    assert_eq!(body["ok"], true);

    let ended = sessions::get(&state.db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(ended.status, "ended");
}

#[tokio::test]
async fn end_unknown_session_is_not_found() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/sessions/unknown-session/end",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn end_without_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/sessions/unknown-session/end",
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
}

#[tokio::test]
async fn brain_lists_entries() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let brain = state
        .brain
        .open("proj", &session.id)
        .await
        .expect("open brain");
    brain.put("/kv/note", b"value").await.expect("put key");
    brain
        .put("/fs/notes/todo.txt", b"value")
        .await
        .expect("put file");

    let app = router(state.clone());
    let keys = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain?path=%2Fkv", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(keys.status(), StatusCode::OK);
    let body = json_body(keys).await;
    assert_eq!(
        body["entries"],
        serde_json::json!([{"path": "/kv/note", "type": "key", "size_bytes": 5}]),
        "a key carries the bytes a read would hand back"
    );
    assert_eq!(body["path"], "/kv");
    assert_eq!(body["truncated"], false);

    let app = router(state.clone());
    let files = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain?path=%2Ffs", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(files.status(), StatusCode::OK);
    let body = json_body(files).await;
    assert_eq!(
        body["entries"],
        serde_json::json!([{"path": "/fs/notes", "type": "dir", "size_bytes": 5}]),
        "a directory is a directory, with the bytes it holds directly"
    );

    // The tree opens one level at a time, so a directory lists its own
    // children rather than the whole brain.
    let app = router(state.clone());
    let level = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain?path=%2Ffs%2Fnotes", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let body = json_body(level).await;
    assert_eq!(
        body["entries"],
        serde_json::json!([{"path": "/fs/notes/todo.txt", "type": "file", "size_bytes": 5}])
    );
    assert_eq!(body["path"], "/fs/notes");
}

#[tokio::test]
async fn a_brain_listing_reports_a_level_it_could_not_carry_whole() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "wide", "agent-one")
        .await
        .expect("start");
    let brain = state
        .brain
        .open("proj", &session.id)
        .await
        .expect("open brain");
    for index in 0..=agent_hub::limits::BRAIN_LIST_ENTRIES_MAX {
        brain
            .put(&format!("/kv/key-{index:04}"), b"v")
            .await
            .expect("put key");
    }

    let app = router(state.clone());
    let body = json_body(
        app.oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain?path=%2Fkv", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request"),
    )
    .await;
    assert_eq!(
        body["entries"].as_array().expect("entries").len(),
        agent_hub::limits::BRAIN_LIST_ENTRIES_MAX
    );
    assert_eq!(
        body["truncated"], true,
        "a level with more says so rather than looking complete"
    );
}

#[tokio::test]
async fn brain_read_does_not_create_a_file() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "fresh", "agent-one")
        .await
        .expect("start");
    let path = state.brain.brain_path("proj", &session.id).expect("path");
    assert!(!path.exists(), "no brain file before the first write");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain?path=%2Fkv", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["entries"], serde_json::json!([]));
    assert!(!path.exists(), "a brain read does not create the file");
}

#[tokio::test]
async fn brain_hides_a_pruned_session() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    agent_hub::store::prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain?path=%2Fkv", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a pruned session is not readable"
    );
}

#[tokio::test]
async fn brain_unknown_session_is_not_found() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions/unknown-session/brain?path=%2Fkv",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn brain_without_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions/unknown-session/brain?path=%2Fkv",
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
}

#[tokio::test]
async fn a_listing_carries_the_owner_and_where_the_work_came_from() {
    let state = state().await;
    let source = sessions::start(&state.db, "proj", "migration", "agent-one")
        .await
        .expect("start");
    sessions::end(&state.db, &source.id, "agent-one", Some("half applied"))
        .await
        .expect("end");
    sessions::start_from(&state.db, "proj", "pickup", "agent-two", &source.id)
        .await
        .expect("adopt");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let entry = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["id"] == source.id.as_str())
        .expect("the adopted session is listed")
        .clone();

    assert_eq!(entry["agent"], "agent-two", "the owner is the current one");
    assert_eq!(entry["handoff"], "half applied");
    assert_eq!(entry["adopted_from"], source.id.as_str());
    // The screen renders "picked up from agent-one" without a second lookup.
    assert_eq!(entry["lineage"]["kind"], "adopted");
    assert_eq!(entry["lineage"]["session_id"], source.id.as_str());
    assert_eq!(entry["lineage"]["agent"], "agent-two");
    assert_eq!(entry["lineage"]["pruned"], false);
}

#[tokio::test]
async fn a_lineage_whose_source_is_gone_reads_as_pruned() {
    let state = state().await;
    let source = sessions::start(&state.db, "proj", "live", "agent-one")
        .await
        .expect("start");
    let fork = sessions::insert_fork(&state.db, &source, "branch", "agent-two", "01FORKED")
        .await
        .expect("fork");
    sessions::end(&state.db, &source.id, "agent-one", None)
        .await
        .expect("end the source");
    agent_hub::store::prune::prune_session(&state.db, &source.id)
        .await
        .expect("prune the source");

    // Inside the undo window the source is still there, pruned.
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let pending = json_body(response).await;
    let entry = pending["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["id"] == fork.id.as_str())
        .expect("the fork is listed")
        .clone();
    assert_eq!(entry["lineage"]["pruned"], true);
    assert_eq!(entry["lineage"]["agent"], "agent-one");

    // Once the sweep has committed the prune the row is gone, and the lineage
    // id is dangling by design.
    state
        .db
        .connect()
        .expect("connect")
        .execute(
            "DELETE FROM sessions WHERE id = ?1",
            vec![turso::Value::Text(source.id.clone())],
        )
        .await
        .expect("commit the prune");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let body = json_body(response).await;
    let entry = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["id"] == fork.id.as_str())
        .expect("the fork is listed")
        .clone();
    assert_eq!(entry["lineage"]["kind"], "forked");
    assert_eq!(entry["lineage"]["pruned"], true);
    assert_eq!(entry["lineage"]["agent"], Value::Null);
}

#[tokio::test]
async fn the_human_reassigns_a_session_over_the_control_surface() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "stuck", "agent-one")
        .await
        .expect("start");
    agent_hub::store::identity::create_agent(&state.db, "agent-two", "Agent two")
        .await
        .expect("create the agent the session moves to");

    // An owner no token resolves to could never resume, end or write the
    // session again, so a name that is not an agent is refused.
    let refused = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/sessions/{}/reassign", session.id))
                .method("POST")
                .header(header::AUTHORIZATION, "Bearer token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"agent":"no-such-agent"}"#))
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    let unmoved = sessions::get(&state.db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        unmoved.agent, "agent-one",
        "a refused reassign moves nothing"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/sessions/{}/reassign", session.id))
                .method("POST")
                .header(header::AUTHORIZATION, "Bearer token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"agent":"agent-two"}"#))
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["agent"], "agent-two");

    let moved = sessions::get(&state.db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(moved.agent, "agent-two");
}

#[tokio::test]
async fn reassign_without_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/sessions/unknown/reassign")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"agent":"agent-two"}"#))
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(problem_body(response).await["code"], "unauthenticated");
}

/// Write to a session's brain through the wrapper and report the file's bytes.
async fn write_brain(state: &AppState, session: &sessions::Session) -> i64 {
    state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain")
        .put("/kv/note", &vec![b'x'; 2048])
        .await
        .expect("put");
    agent_hub::brain::file_bytes(&state.data_dir.join(&session.brain_path))
}

#[tokio::test]
async fn a_session_detail_carries_its_size_events_and_last_line() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let brain_bytes = write_brain(&state, &session).await;
    assert!(brain_bytes > 0);

    // Work the session did, plus a signal from elsewhere that is not its own.
    for summary in ["first report", "second report"] {
        agent_hub::store::events::append(
            &state.db,
            "agent-one",
            None,
            agent_hub::store::events::NewEvent {
                project_id: "proj".to_string(),
                kind: "signal".to_string(),
                summary: summary.to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: Some(session.id.clone()),
            },
        )
        .await
        .expect("append");
    }
    agent_hub::store::events::append(
        &state.db,
        "agent-two",
        None,
        agent_hub::store::events::NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "another agent's work".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    assert_eq!(body["id"], session.id.as_str());
    assert_eq!(body["agent"], "agent-one");
    assert_eq!(body["created_at"], session.created_at.as_str());
    assert_eq!(body["brain_bytes"], brain_bytes);
    assert_eq!(
        body["events"], 3,
        "the started event plus the two the session wrote: {body}"
    );
    assert_eq!(body["last_event"]["summary"], "second report");
    assert_eq!(body["last_event"]["actor"], "agent-one");
    assert!(
        body["last_event"]["at"]
            .as_str()
            .is_some_and(|at| !at.is_empty()),
        "the line says when"
    );
}

#[tokio::test]
async fn a_session_that_wrote_nothing_has_no_line_invented_for_it() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    // The lifecycle event the start wrote is the session's own, so the line is
    // real; a session whose events are gone has none.
    let conn = state.db.connect().expect("connect");
    conn.execute("DELETE FROM events", ()).await.expect("clear");

    let app = router(state.clone());
    let body = json_body(
        app.oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("response"),
    )
    .await;
    assert_eq!(body["events"], 0);
    assert!(
        body["last_event"].is_null(),
        "no writes means no line: {body}"
    );
    assert_eq!(
        body["brain_bytes"], 0,
        "a session that never wrote has no brain file"
    );
}

#[tokio::test]
async fn a_pruned_session_has_no_detail() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&state.db, &session.id, "agent-one", None)
        .await
        .expect("end");
    agent_hub::store::prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    problem_body(response).await;
}

#[tokio::test]
async fn a_listing_row_carries_the_brain_size_the_prune_button_shows() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let brain_bytes = write_brain(&state, &session).await;

    let app = router(state.clone());
    let body = json_body(
        app.oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
        ))
        .await
        .expect("response"),
    )
    .await;
    assert_eq!(body["sessions"][0]["brain_bytes"], brain_bytes);
}
