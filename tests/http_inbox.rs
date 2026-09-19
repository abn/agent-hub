//! HTTP home, inbox, and answer routes: counts, listing, auth, and problems.

use std::time::Duration;

use tokio_stream::StreamExt;

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::questions::{self, NewQuestion};
use axum::http::{StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

use common::http::{json_body, problem_body, request};
use common::state::TestState;

async fn state() -> TestState {
    common::state::open("http-inbox").await
}

async fn seed_question(state: &AppState, subject: &str) -> String {
    questions::post(
        &state.db,
        &agent_hub::limits::InboxCaps::disabled(),
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject,
            body: None,
            context: None,
            to: None,
            idempotency_key: None,
            session_id: None,
        },
    )
    .await
    .expect("post question")
}

async fn seed_finished(state: &AppState, summary: &str) -> String {
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "finished".to_string(),
            summary: summary.to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append finished")
}

async fn seed_approval(state: &AppState, summary: &str) -> String {
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "approval".to_string(),
            summary: summary.to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append approval")
}

#[tokio::test]
async fn home_returns_the_counts_and_recent_events() {
    let state = state().await;
    seed_question(&state, "Deploy tonight?").await;
    seed_finished(&state, "nightly report done").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/home", Some("Bearer token"), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    assert_eq!(body["unread"], 1);
    assert_eq!(body["waiting"], 1);
    assert_eq!(body["recent"].as_array().expect("recent array").len(), 2);
}

#[tokio::test]
async fn inbox_lists_the_items() {
    let state = state().await;
    seed_question(&state, "Ship it?").await;
    seed_finished(&state, "nightly report done").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/inbox", Some("Bearer token"), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["items"].as_array().expect("items array").len(), 2);
}

#[tokio::test]
async fn inbox_filters_by_status_and_rejects_an_unknown_one() {
    let state = state().await;
    let question_id = seed_question(&state, "Ship it?").await;
    seed_finished(&state, "nightly report done").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/inbox?status=action",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["event_id"], question_id);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/inbox?status=bogus",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn answering_a_question_returns_an_event_and_resolves_the_item() {
    let state = state().await;
    let question_id = seed_question(&state, "Ship it?").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/questions/{question_id}/answer"),
            Some("Bearer token"),
            Some(json!({ "body": "Yes, ship it" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let event_id = body["event_id"].as_str().expect("event id");
    assert!(!event_id.is_empty());
    assert_ne!(event_id, question_id);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/inbox?status=resolved",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    let body = json_body(response).await;
    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["event_id"], question_id);

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/home", Some("Bearer token"), None))
        .await
        .expect("request");
    let body = json_body(response).await;
    assert_eq!(body["waiting"], 0);
}

#[tokio::test]
async fn answering_a_non_question_is_a_problem() {
    let state = state().await;
    let finished_id = seed_finished(&state, "not a question").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/questions/{finished_id}/answer"),
            Some("Bearer token"),
            Some(json!({ "body": "nope" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn answering_an_unknown_id_is_not_found() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/questions/unknown-question/answer",
            Some("Bearer token"),
            Some(json!({ "body": "hello" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn deciding_an_approval_returns_an_event_and_resolves_the_item() {
    let state = state().await;
    let approval_id = seed_approval(&state, "Deploy 0.4.2").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/approvals/{approval_id}/decision"),
            Some("Bearer token"),
            Some(json!({ "decision": "approve", "note": "ship it" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let event_id = body["event_id"].as_str().expect("event id");
    assert!(!event_id.is_empty());
    assert_ne!(event_id, approval_id);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/inbox?status=resolved",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    let body = json_body(response).await;
    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["event_id"], approval_id);

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/home", Some("Bearer token"), None))
        .await
        .expect("request");
    let body = json_body(response).await;
    assert_eq!(body["waiting"], 0);
}

#[tokio::test]
async fn a_repeated_decision_is_a_conflict() {
    let state = state().await;
    let approval_id = seed_approval(&state, "Deploy 0.4.2").await;

    let decide = |id: String| {
        let app = router(state.clone());
        async move {
            app.oneshot(request(
                "POST",
                &format!("/api/v1/approvals/{id}/decision"),
                Some("Bearer token"),
                Some(json!({ "decision": "approve" })),
            ))
            .await
            .expect("request")
        }
    };

    let first = decide(approval_id.clone()).await;
    assert_eq!(first.status(), StatusCode::OK);

    let second = decide(approval_id).await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let problem = problem_body(second).await;
    assert_eq!(problem["code"], "conflict");
}

#[tokio::test]
async fn a_decision_replays_on_its_idempotency_key() {
    let state = state().await;
    let approval_id = seed_approval(&state, "Deploy 0.4.2").await;

    let decide = |body: Value| {
        let app = router(state.clone());
        let id = approval_id.clone();
        async move {
            app.oneshot(request(
                "POST",
                &format!("/api/v1/approvals/{id}/decision"),
                Some("Bearer token"),
                Some(body),
            ))
            .await
            .expect("request")
        }
    };

    let first = decide(json!({ "decision": "approve", "idempotency_key": "key-1" })).await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_id = json_body(first).await["event_id"]
        .as_str()
        .expect("event id")
        .to_string();

    let replay = decide(json!({ "decision": "approve", "idempotency_key": "key-1" })).await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_id = json_body(replay).await["event_id"]
        .as_str()
        .expect("event id")
        .to_string();
    assert_eq!(first_id, replay_id, "a replay returns the original answer");
}

#[tokio::test]
async fn deciding_a_non_approval_is_a_problem() {
    let state = state().await;
    let finished_id = seed_finished(&state, "not an approval").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/approvals/{finished_id}/decision"),
            Some("Bearer token"),
            Some(json!({ "decision": "approve" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn deciding_an_unknown_id_is_not_found() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/approvals/unknown-approval/decision",
            Some("Bearer token"),
            Some(json!({ "decision": "approve" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn a_malformed_decision_is_a_problem() {
    let state = state().await;
    let approval_id = seed_approval(&state, "Deploy 0.4.2").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/approvals/{approval_id}/decision"),
            Some("Bearer token"),
            Some(json!({ "decision": "maybe" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn deciding_without_a_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/approvals/any-approval/decision",
            None,
            Some(json!({ "decision": "approve" })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
}

#[tokio::test]
async fn the_stream_requires_a_token_and_pushes_a_tick() {
    let state = state().await;
    let question_id = seed_question(&state, "Ship it?").await;

    let app = router(state.clone());
    let denied = app
        .oneshot(request("GET", "/api/v1/stream", None, None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/stream", Some("Bearer token"), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
    );

    // A write on the REST surface reaches the subscriber as a tick.
    let mut body = response.into_body().into_data_stream();
    let app = router(state.clone());
    let answered = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/questions/{question_id}/answer"),
            Some("Bearer token"),
            Some(json!({ "body": "yes" })),
        ))
        .await
        .expect("request");
    assert_eq!(answered.status(), StatusCode::OK);

    let frame = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .expect("a frame arrives")
        .expect("a chunk")
        .expect("read ok");
    let text = String::from_utf8_lossy(&frame);
    assert!(
        text.contains("event: tick"),
        "the write ticks the stream: {text}"
    );
}

#[tokio::test]
async fn missing_token_is_a_problem() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/home", None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
}

#[tokio::test]
async fn the_inbox_listing_honours_the_limit() {
    let state = state().await;
    for index in 0..3 {
        events::append(
            &state.db,
            "agent-one",
            None,
            NewEvent {
                project_id: "proj".to_string(),
                kind: "finished".to_string(),
                summary: format!("done {index}"),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append finished work");
    }

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/inbox?status=unread&limit=2",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(
        body["items"].as_array().expect("items").len(),
        2,
        "the limit caps the page"
    );
}

async fn seed_finished_in(state: &AppState, project_id: &str, summary: &str) -> String {
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: project_id.to_string(),
            kind: "finished".to_string(),
            summary: summary.to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append finished")
}

/// The counts Home reports and the sizes of the two listings that feed them.
async fn counted(state: &AppState) -> (i64, i64, usize, usize) {
    let app = router(state.clone());
    let home = json_body(
        app.oneshot(request("GET", "/api/v1/home", Some("Bearer token"), None))
            .await
            .expect("request"),
    )
    .await;

    let app = router(state.clone());
    let unread = json_body(
        app.oneshot(request(
            "GET",
            "/api/v1/inbox?unread_only=true",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request"),
    )
    .await;

    let app = router(state.clone());
    let waiting = json_body(
        app.oneshot(request(
            "GET",
            "/api/v1/inbox?status=action",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request"),
    )
    .await;

    (
        home["unread"].as_i64().expect("unread count"),
        home["waiting"].as_i64().expect("waiting count"),
        unread["items"].as_array().expect("items").len(),
        waiting["items"].as_array().expect("items").len(),
    )
}

async fn post(state: &AppState, path: &str, body: Option<Value>) -> axum::response::Response {
    router(state.clone())
        .oneshot(request("POST", path, Some("Bearer token"), body))
        .await
        .expect("request")
}

#[tokio::test]
async fn an_item_is_read_then_unread_and_the_counts_follow() {
    let state = state().await;
    let event_id = seed_finished(&state, "nightly report done").await;
    seed_question(&state, "Ship it?").await;
    assert_eq!(counted(&state).await, (1, 1, 1, 1));

    let response = post(&state, &format!("/api/v1/inbox/{event_id}/read"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["status"], "read");
    assert_eq!(body["changed"], true);
    assert_eq!(
        counted(&state).await,
        (0, 1, 0, 1),
        "the unread count, the unread listing and the waiting queue agree"
    );

    let replay = post(&state, &format!("/api/v1/inbox/{event_id}/read"), None).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["changed"], false);
    assert_eq!(counted(&state).await, (0, 1, 0, 1));

    let response = post(&state, &format!("/api/v1/inbox/{event_id}/unread"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["status"], "unread");
    assert_eq!(body["changed"], true);
    assert_eq!(counted(&state).await, (1, 1, 1, 1));
}

#[tokio::test]
async fn marking_a_waiting_item_read_leaves_it_waiting() {
    let state = state().await;
    let approval_id = seed_approval(&state, "Deploy 0.4.2").await;

    let response = post(&state, &format!("/api/v1/inbox/{approval_id}/read"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(
        body["status"], "action",
        "what waits on the human carries no read state"
    );
    assert_eq!(body["changed"], false);

    let (unread, waiting, unread_rows, waiting_rows) = counted(&state).await;
    assert_eq!((unread, waiting), (0, 1));
    assert_eq!(unread_rows, 0);
    assert_eq!(waiting_rows, 1, "the approval still waits on the human");
}

#[tokio::test]
async fn marking_everything_read_is_scoped_by_project() {
    let state = state().await;
    seed_finished(&state, "here").await;
    seed_finished_in(&state, "other", "there").await;
    seed_question(&state, "Ship it?").await;

    let response = post(
        &state,
        "/api/v1/inbox/read-all",
        Some(json!({ "project_id": "proj" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["marked"], 1);

    let (unread, waiting, unread_rows, waiting_rows) = counted(&state).await;
    assert_eq!(unread, 1, "the other project keeps its unread item");
    assert_eq!(unread_rows, 1);
    assert_eq!((waiting, waiting_rows), (1, 1));

    let response = post(&state, "/api/v1/inbox/read-all", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["marked"], 1);
    assert_eq!(counted(&state).await, (0, 1, 0, 1));
}

#[tokio::test]
async fn reading_an_unknown_item_is_not_found() {
    let state = state().await;
    let response = post(&state, "/api/v1/inbox/no-such-event/read", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(response).await["code"], "not_found");
}

#[tokio::test]
async fn the_read_routes_need_a_token() {
    let state = state().await;
    let event_id = seed_finished(&state, "nightly report done").await;

    for path in [
        format!("/api/v1/inbox/{event_id}/read"),
        format!("/api/v1/inbox/{event_id}/unread"),
        "/api/v1/inbox/read-all".to_string(),
    ] {
        let response = router(state.clone())
            .oneshot(request("POST", &path, None, None))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{path} is admin only"
        );
    }
}

#[tokio::test]
async fn unread_only_cannot_contradict_the_status_filter() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/inbox?unread_only=true&status=action",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(response).await["code"], "invalid_argument");
}
