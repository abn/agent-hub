//! HTTP feed route: auth, ordering, and problem responses.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::events::{NewEvent, append};
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
        "agent-hub-http-{}-{nanos}-{unique}",
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

fn event(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
    }
}

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method("GET");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder.body(Body::empty()).expect("build request")
}

async fn problem_body(response: axum::response::Response) -> Value {
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
    );
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("problem is JSON")
}

#[tokio::test]
async fn missing_token_is_a_problem() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
    assert!(problem["title"].is_string());
}

#[tokio::test]
async fn bearer_token_reads_events_newest_first() {
    let state = state().await;
    append(&state.db, "agent", None, event("first"))
        .await
        .expect("append first");
    append(&state.db, "agent", None, event("second"))
        .await
        .expect("append second");

    let app = router(state);
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    let events = page["events"].as_array().expect("events array");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["summary"], "second");
    assert_eq!(events[1]["summary"], "first");
    assert_eq!(page["next_since"], Value::Null);
}

#[tokio::test]
async fn invalid_limit_is_rejected() {
    let app = router(state().await);
    let response = app
        .oneshot(get(
            "/api/v1/projects/proj/feed?limit=soon",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn unknown_route_is_not_found() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/api/v1/no-such-route", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}
