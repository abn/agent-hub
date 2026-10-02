//! Comment threads on project knowledge base pages, driven through the router.

use agent_hub::http::router;
use agent_hub::store::projects;
use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

mod common;

use common::http::{json_body, problem_body, request};
use common::state::TestState;

const ADMIN: &str = "Bearer token";

async fn state() -> TestState {
    let state = common::state::open("kb-comments").await;
    projects::create(&state.db, "proj", "Project")
        .await
        .expect("create project");
    state
}

async fn post_comment(state: &TestState, path: &str, body: &str) -> serde_json::Value {
    let app = router((**state).clone());
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/projects/proj/kb/comments",
            Some(ADMIN),
            Some(json!({ "path": path, "body": body })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await
}

#[tokio::test]
async fn add_and_list_a_comment_on_a_page() {
    let state = state().await;
    let posted = post_comment(&state, "notes/todo.md", "Looks good").await;
    let comment = &posted["comment"];
    let comment_id = comment["id"].as_str().expect("comment id").to_string();
    assert_eq!(comment["body"], "Looks good");
    assert_eq!(comment["author"], "human");
    assert_eq!(comment["done"], false);
    assert_eq!(comment["path"], "/fs/notes/todo.md");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/kb/comments?path=/fs/notes/todo.md",
            Some(ADMIN),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let listed = json_body(response).await;
    let comments = listed["comments"].as_array().expect("array");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["id"], comment_id);
    assert_eq!(comments[0]["body"], "Looks good");
}

#[tokio::test]
async fn a_page_with_no_comments_lists_empty() {
    let state = state().await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/kb/comments?path=/fs/notes/nothing.md",
            Some(ADMIN),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        json_body(response).await["comments"]
            .as_array()
            .expect("array")
            .is_empty()
    );
}

#[tokio::test]
async fn mark_a_comment_done() {
    let state = state().await;
    let posted = post_comment(&state, "notes/todo.md", "Resolve me").await;
    let comment_id = posted["comment"]["id"].as_str().expect("comment id");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/projects/proj/kb/comments/{comment_id}/done"),
            Some(ADMIN),
            Some(json!({ "done": true })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["comment"]["done"], true);
}

#[tokio::test]
async fn delete_a_comment() {
    let state = state().await;
    let posted = post_comment(&state, "notes/todo.md", "Remove me").await;
    let comment_id = posted["comment"]["id"].as_str().expect("comment id");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/projects/proj/kb/comments/{comment_id}"),
            Some(ADMIN),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/kb/comments?path=/fs/notes/todo.md",
            Some(ADMIN),
            None,
        ))
        .await
        .expect("request");
    assert!(
        json_body(response).await["comments"]
            .as_array()
            .expect("array")
            .is_empty(),
        "a deleted comment is gone"
    );
}

#[tokio::test]
async fn a_comment_stays_on_its_own_page() {
    let state = state().await;
    post_comment(&state, "notes/one.md", "On the first").await;
    post_comment(&state, "notes/two.md", "On the second").await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/kb/comments?path=/fs/notes/one.md",
            Some(ADMIN),
            None,
        ))
        .await
        .expect("request");
    let listed = json_body(response).await;
    let comments = listed["comments"].as_array().expect("array");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["body"], "On the first");
}

#[tokio::test]
async fn an_unknown_project_is_404() {
    let state = state().await;

    for (method, uri, body) in [
        (
            "GET",
            "/api/v1/projects/missing/kb/comments?path=/fs/notes/todo.md".to_string(),
            None,
        ),
        (
            "POST",
            "/api/v1/projects/missing/kb/comments".to_string(),
            Some(json!({ "path": "notes/todo.md", "body": "Hi" })),
        ),
        (
            "POST",
            "/api/v1/projects/missing/kb/comments/missing/done".to_string(),
            Some(json!({ "done": true })),
        ),
        (
            "DELETE",
            "/api/v1/projects/missing/kb/comments/missing".to_string(),
            None,
        ),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(method, &uri, Some(ADMIN), body))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
        assert_eq!(problem_body(response).await["code"], "not_found");
    }
}

#[tokio::test]
async fn comment_routes_require_a_token() {
    let state = state().await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/kb/comments?path=/fs/notes/todo.md",
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(problem_body(response).await["code"], "unauthenticated");
}
