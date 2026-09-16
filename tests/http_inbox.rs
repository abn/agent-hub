//! HTTP home, inbox, and answer routes: counts, listing, auth, and problems.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::questions::{self, NewQuestion};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-inbox-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
    })
    .await
    .expect("open state")
}

fn request(method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .expect("build request"),
        None => builder.body(Body::empty()).expect("build request"),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

async fn problem_body(response: axum::response::Response) -> Value {
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
    );
    json_body(response).await
}

async fn seed_question(state: &AppState, subject: &str) -> String {
    questions::post(
        &state.db,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject,
            body: None,
            context: None,
            to: None,
            idempotency_key: None,
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
        },
    )
    .await
    .expect("append finished")
}

#[tokio::test]
async fn home_returns_the_counts_and_recent_events() {
    let state = state().await;
    seed_question(&state, "Deploy tonight?").await;
    seed_finished(&state, "nightly report done").await;

    let app = router(state);
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

    let app = router(state);
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

    let app = router(state);
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

    let app = router(state);
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

    let app = router(state);
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
    let app = router(state().await);
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
async fn missing_token_is_a_problem() {
    let app = router(state().await);
    let response = app
        .oneshot(request("GET", "/api/v1/home", None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
}
