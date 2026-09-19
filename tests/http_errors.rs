//! Every REST failure is problem details, whichever part of the request the
//! framework refused: the body, its type, its size, a query, a path, or the
//! method.

use agent_hub::http::problem::ProblemPath;
use agent_hub::http::router;
use agent_hub::limits::REQUEST_BODY_BYTES_MAX;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

use common::state::TestState;

const PROBLEM_JSON: &str = "application/problem+json";

async fn state() -> TestState {
    common::state::open("http-errors").await
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
    let state = state().await;
    let response = router(state.clone())
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
    let state = state().await;
    let response = router(state.clone())
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
    let state = state().await;
    let response = router(state.clone())
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
    let state = state().await;
    let response = router(state.clone())
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
    let state = state().await;
    let response = router(state.clone())
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

/// A handler that reads more path parameters than its route declares is a
/// server fault, and the framework says so with a 500. No production route
/// does that, so the test declares the broken pair itself.
#[tokio::test]
async fn a_path_the_route_cannot_satisfy_stays_a_server_error() {
    async fn mismatched(ProblemPath((one, two)): ProblemPath<(String, String)>) -> String {
        format!("{one}{two}")
    }

    let response = Router::new()
        .route("/mismatched/{id}", get(mismatched))
        .oneshot(request("GET", "/mismatched/only-one", None, Body::empty()))
        .await
        .expect("request");

    let (status, problem) = problem(response).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(problem["code"], "internal");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        !detail.contains("path arguments"),
        "the framework's own wording stays off the wire: {detail}"
    );
}

#[tokio::test]
async fn a_method_mismatch_is_a_problem_that_names_the_methods() {
    let state = state().await;
    let response = router(state.clone())
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
