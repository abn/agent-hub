//! HTTP session brain entry route: read one KV value or FS file.

use agent_hub::http::router;
use agent_hub::store::sessions;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::http::{json_body, problem_body};
use common::state::TestState;

async fn state() -> TestState {
    common::state::open("http-brain").await
}

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    common::http::request(method, uri, auth, None)
}

#[tokio::test]
async fn kv_write_through_store_read_over_route_content_equal() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain.put("/kv/branch", b"main").await.expect("put key");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Fkv%2Fbranch",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    assert_eq!(body["path"], "/kv/branch");
    assert_eq!(body["type"], "key");
    assert_eq!(body["size_bytes"], 4);
    assert_eq!(body["content"], "main");
    assert_eq!(body["written_at"], Value::Null);
}

#[tokio::test]
async fn fs_file_write_through_store_read_over_route_content_equal() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain
        .put("/fs/notes/todo.txt", b"finish the tests")
        .await
        .expect("put file");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Ffs%2Fnotes%2Ftodo.txt",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    assert_eq!(body["path"], "/fs/notes/todo.txt");
    assert_eq!(body["type"], "file");
    assert_eq!(body["size_bytes"], 16);
    assert_eq!(body["content"], "finish the tests");
    assert_eq!(body["written_at"], Value::Null);
}

#[tokio::test]
async fn absent_path_answers_404_naming_path() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain.put("/kv/seed", b"value").await.expect("seed key");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Fkv%2Fabsent_key",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("/kv/absent_key"),
        "detail must name path: {}",
        problem["detail"]
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Ffs%2Fabsent_file.txt",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("/fs/absent_file.txt"),
        "detail must name path: {}",
        problem["detail"]
    );
}

#[tokio::test]
async fn directory_path_answers_409() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain
        .put("/fs/notes/todo.txt", b"finish the tests")
        .await
        .expect("put file");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain/entry?path=%2Fkv", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "conflict");
    assert!(
        problem["detail"].as_str().unwrap().contains("directory"),
        "detail must say directory: {}",
        problem["detail"]
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain/entry?path=%2Ffs", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "conflict");
    assert!(
        problem["detail"].as_str().unwrap().contains("directory"),
        "detail must say directory: {}",
        problem["detail"]
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Ffs%2Fnotes",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "conflict");
    assert!(
        problem["detail"].as_str().unwrap().contains("directory"),
        "detail must say directory: {}",
        problem["detail"]
    );
}

#[tokio::test]
async fn invalid_utf8_bytes_answer_422() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let brain = state
        .brain
        .open(&session.project_id, &session.id)
        .await
        .expect("open brain");
    brain
        .put("/kv/binary", &[0xff, 0xfe, 0xfd])
        .await
        .expect("put non-utf8 key");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Fkv%2Fbinary",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let problem = problem_body(response).await;
    assert_eq!(problem["status"], 422);

    brain
        .put("/fs/binary.bin", &[0xff, 0xfe, 0xfd])
        .await
        .expect("put non-utf8 file");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Ffs%2Fbinary.bin",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let problem = problem_body(response).await;
    assert_eq!(problem["status"], 422);
}

#[tokio::test]
async fn session_with_no_brain_answers_404_and_leaves_no_brain_file() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let path = state
        .brain
        .brain_path("proj", &session.id)
        .expect("brain path");
    assert!(!path.exists(), "brain file should not exist yet");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/sessions/{}/brain/entry?path=%2Fkv%2Fnote",
                session.id
            ),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        !path.exists(),
        "reading a session with no brain must not create a brain file"
    );

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
    assert!(
        problem["detail"].as_str().unwrap().contains("/kv/note"),
        "detail must name path: {}",
        problem["detail"]
    );
}

#[tokio::test]
async fn missing_path_query_parameter_is_a_problem_naming_path() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/sessions/{}/brain/entry", session.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
    assert!(
        problem["detail"].as_str().unwrap().contains("path"),
        "detail must name path: {}",
        problem["detail"]
    );
}
