//! Every REST failure is problem details, whichever part of the request the
//! framework refused: the body, its type, its size, a query, a path, or the
//! method.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::limits::REQUEST_BODY_BYTES_MAX;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

static NEXT: AtomicU64 = AtomicU64::new(0);

const PROBLEM_JSON: &str = "application/problem+json";

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-errors-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
    })
    .await
    .expect("open state")
}

fn request(method: &str, uri: &str, content_type: Option<&str>, body: Body) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, "Bearer token");
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(body).expect("build request")
}

async fn problem(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some(PROBLEM_JSON),
        "the failure is problem details"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let value: Value = serde_json::from_slice(&bytes).expect("problem json");
    (status, value)
}

#[tokio::test]
async fn a_body_over_the_limit_is_payload_too_large() {
    let body = json!({"id": "pad", "display_name": "x".repeat(REQUEST_BODY_BYTES_MAX)});
    let response = router(state().await)
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            Some("application/json"),
            Body::from(body.to_string()),
        ))
        .await
        .expect("request");

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(problem["code"], "payload_too_large");
}

#[tokio::test]
async fn a_body_without_the_json_type_is_unsupported_media_type() {
    let body = json!({"id": "homelab", "display_name": "Homelab"});
    let response = router(state().await)
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            None,
            Body::from(body.to_string()),
        ))
        .await
        .expect("request");

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn a_malformed_body_stays_an_invalid_argument() {
    let response = router(state().await)
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            Some("application/json"),
            Body::from("{not json"),
        ))
        .await
        .expect("request");

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn an_unparsable_query_is_a_problem() {
    let response = router(state().await)
        .oneshot(request(
            "GET",
            "/api/v1/artifacts/unknown/raw?version=abc",
            None,
            Body::empty(),
        ))
        .await
        .expect("request");

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_argument");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("version"),
        "the problem names the parameter: {problem}"
    );
}

#[tokio::test]
async fn an_unparsable_path_is_a_problem() {
    let response = router(state().await)
        .oneshot(request(
            "GET",
            "/api/v1/artifacts/%FF/versions",
            None,
            Body::empty(),
        ))
        .await
        .expect("request");

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn a_method_mismatch_is_a_problem_that_names_the_methods() {
    let response = router(state().await)
        .oneshot(request("POST", "/api/v1/home", None, Body::empty()))
        .await
        .expect("request");

    let allow = response
        .headers()
        .get(header::ALLOW)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        allow.contains("GET"),
        "the allowed methods survive: {allow}"
    );

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(problem["code"], "invalid_argument");
}
