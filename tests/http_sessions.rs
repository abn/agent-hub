//! HTTP session routes: listing, ending, auth, and problem responses.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::sessions;
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
        "agent-hub-http-sessions-{}-{nanos}-{unique}",
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

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder.body(Body::empty()).expect("build request")
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

#[tokio::test]
async fn list_without_token_is_a_problem() {
    let app = router(state().await);
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
    let app = router(state().await);
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

    let app = router(state);
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
    let app = router(state().await);
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
    let app = router(state().await);
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
    assert_eq!(body["entries"], serde_json::json!(["/kv/note"]));

    let app = router(state);
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
    assert_eq!(body["entries"], serde_json::json!(["/fs/notes"]));
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
    sessions::end(&state.db, &session.id, "agent-one")
        .await
        .expect("end");
    agent_hub::store::prune::prune_session(&state.db, &session.id)
        .await
        .expect("prune");

    let app = router(state);
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
    let app = router(state().await);
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
    let app = router(state().await);
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
