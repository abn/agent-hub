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
    let app = router((*state).clone());
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
    let app = router((*state).clone());
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
    let app = router((*state).clone());
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

    let app = router((*state).clone());
    let response = app
        .oneshot(request("GET", "/api/v1/inbox", Some("Bearer token"), None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let items = body["items"].as_array().cloned().unwrap_or_default();
    assert!(items.iter().any(|i| i["kind"] == "idea"));
}

async fn captured(state: &TestState, summary: &str) -> String {
    let app = router((*state).clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/projects/proj/ideas",
            Some("Bearer token"),
            Some(serde_json::json!({"summary": summary})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["id"]
        .as_str()
        .expect("idea id")
        .to_string()
}

async fn inbox(state: &TestState, status: &str) -> Vec<serde_json::Value> {
    let app = router((*state).clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/inbox?status={status}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn resolve_closes_an_idea() {
    let state = state().await;
    let idea = captured(&state, "Ship the thing").await;

    let app = router((*state).clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/ideas/{idea}/resolve"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let resolved = inbox(&state, "resolved").await;
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["event_id"], idea);
}

#[tokio::test]
async fn clear_removes_an_idea_from_the_queue() {
    let state = state().await;
    let idea = captured(&state, "Park this thought").await;

    let app = router((*state).clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/ideas/{idea}/clear"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let cleared = inbox(&state, "cleared").await;
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0]["event_id"], idea);
    assert!(inbox(&state, "action").await.is_empty());
}

#[tokio::test]
async fn reply_logs_an_addendum_on_the_idea_thread() {
    let state = state().await;
    let idea = captured(&state, "An idea").await;

    let app = router((*state).clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/ideas/{idea}/reply"),
            Some("Bearer token"),
            Some(serde_json::json!({"summary": "More context"})),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let reply = json_body(response).await["id"]
        .as_str()
        .expect("reply id")
        .to_string();

    let app = router((*state).clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/feed",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    let events = json_body(response).await["events"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let addendum = events
        .iter()
        .find(|event| event["id"] == reply)
        .expect("reply on the feed");
    assert_eq!(addendum["kind"], "idea");
    assert_eq!(addendum["thread_id"], idea);
    assert_eq!(addendum["needs_action"], false);
    assert!(
        inbox(&state, "action")
            .await
            .iter()
            .all(|item| item["event_id"] != reply)
    );
}
