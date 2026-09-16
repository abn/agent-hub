//! HTTP artifact and storage routes: listing, rendering, prune, and undo.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::sessions;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

/// The ciphertext published for the protected render test.
const CIPHERTEXT: &str = "cipher-mark-7f3a9c";

/// A marker that never reaches the server for a protected artifact.
const PLAINTEXT: &str = "plaintext-should-never-appear";

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-artifacts-{}-{nanos}-{unique}",
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

async fn text_body(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8(bytes.to_vec()).expect("body is UTF-8")
}

fn content_type(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
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

async fn publish_public(state: &AppState, project_id: &str, title: &str, content: &[u8]) -> String {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id,
            title,
            kind: "html",
            content,
            envelope: None,
        },
    )
    .await
    .expect("publish public artifact")
    .id
}

async fn publish_protected(
    state: &AppState,
    project_id: &str,
    title: &str,
    content: &[u8],
) -> String {
    artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id,
            title,
            kind: "html",
            content,
            envelope: Some(json!({
                "alg": "AES-256-GCM",
                "kdf": "PBKDF2-HMAC-SHA256",
                "iterations": 600000,
                "salt": "c2FsdA",
                "iv": "aXY",
            })),
        },
    )
    .await
    .expect("publish protected artifact")
    .id
}

async fn list_sessions(state: &AppState) -> Vec<Value> {
    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/sessions?project=proj",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["sessions"]
        .as_array()
        .expect("sessions array")
        .clone()
}

#[tokio::test]
async fn listing_artifacts_requires_a_token() {
    let app = router(state().await);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/artifacts",
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
}

#[tokio::test]
async fn listing_artifacts_returns_the_project_artifacts() {
    let state = state().await;
    let first = publish_public(&state, "proj", "Report", b"<p>one</p>").await;
    publish_public(&state, "other", "Elsewhere", b"<p>two</p>").await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/projects/proj/artifacts",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    let listed = body["artifacts"].as_array().expect("artifacts array");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], first);
    assert_eq!(listed[0]["title"], "Report");
    assert_eq!(listed[0]["protected"], false);
}

#[tokio::test]
async fn rendering_a_public_artifact_serves_its_html() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>public-artifact-body</p>").await;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/html; charset=utf-8")
    );

    let body = text_body(response).await;
    assert!(body.contains("public-artifact-body"));
}

#[tokio::test]
async fn rendering_a_markdown_artifact_wraps_and_escapes_it() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Notes",
            kind: "markdown",
            content: b"# Heading\n<script>alert(1)</script>",
            envelope: None,
        },
    )
    .await
    .expect("publish markdown")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/html; charset=utf-8")
    );

    let body = text_body(response).await;
    assert!(body.contains("<pre>"));
    assert!(body.contains("# Heading"));
    assert!(body.contains("&lt;script&gt;"));
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "agent markdown must be escaped, not interpreted"
    );
}

#[tokio::test]
async fn rendering_a_protected_artifact_serves_the_unlock_shell() {
    let state = state().await;
    let id = publish_protected(&state, "proj", "Sealed report", CIPHERTEXT.as_bytes()).await;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/html; charset=utf-8")
    );

    let body = text_body(response).await;
    assert!(body.contains("This artifact is encrypted"));
    assert!(body.contains("Decryption happens in your browser"));
    assert!(body.contains("AES-256-GCM"));
    assert!(
        body.contains(CIPHERTEXT),
        "the shell must carry the ciphertext"
    );
    assert!(
        !body.contains(PLAINTEXT),
        "the shell must not carry any plaintext"
    );
}

#[tokio::test]
async fn pruning_a_session_returns_a_token_and_undo_restores_it() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    assert_eq!(list_sessions(&state).await.len(), 1);

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/storage/sessions/{}", session.id),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let body = json_body(response).await;
    let token = body["undo_token"].as_str().expect("undo token").to_string();
    assert_eq!(token, session.id);
    assert!(
        body["undo_expires_at"].as_str().is_some(),
        "an expiry must be returned"
    );

    assert!(
        list_sessions(&state).await.is_empty(),
        "a pruned session is hidden from the listing"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/prune/undo/{token}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["ok"], true);

    let restored = list_sessions(&state).await;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0]["id"], session.id);
}

#[tokio::test]
async fn pruning_requires_a_token() {
    let state = state().await;
    let session = sessions::start(&state.db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let app = router(state);
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/storage/sessions/{}", session.id),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
}
