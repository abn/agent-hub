//! HTTP idea capture: auth, validation, and inbox surfacing.

use agent_hub::http::router;
use axum::http::StatusCode;
use tower::ServiceExt;

mod common;

use common::http::{json_body, request};
use common::state::TestState;

async fn state() -> TestState {
    let state = common::state::open("http-ideas").await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Default Project").await;
    state
}

#[tokio::test]
async fn create_requires_a_token() {
    let state = state().await;
    let app = router(state.clone());
    let req = request(
        "POST",
        "/api/v1/projects/proj/ideas",
        None,
        Some(serde_json::json!({"summary":"x"})),
    );
    let response = app.oneshot(req).await.expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_unknown_project_is_not_found() {
    let state = state().await;
    let app = router(state.clone());
    let req = request(
        "POST",
        "/api/v1/projects/nope/ideas",
        Some("Bearer token"),
        Some(serde_json::json!({"summary":"x"})),
    );
    let response = app.oneshot(req).await.expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_returns_an_id_and_surfaces_in_inbox() {
    let state = state().await;
    let app = router(state.clone());
    let req = request(
        "POST",
        "/api/v1/projects/proj/ideas",
        Some("Bearer token"),
        Some(serde_json::json!({"summary":"Ship the thing","payload":{"note":"soon"}})),
    );
    let response = app.oneshot(req).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["id"].is_string());

    let app = router(state.clone());
    let response = app
        .oneshot(request("GET", "/api/v1/inbox", Some("Bearer token"), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let items = body["items"].as_array().cloned().unwrap_or_default();
    assert!(items.iter().any(|i| i["kind"] == "idea"));
}
