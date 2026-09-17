//! HTTP artifact and storage routes: listing, rendering, prune, and undo.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::error::Error;
use agent_hub::http::problem::Problem;
use agent_hub::http::router;
use agent_hub::store::artifacts::{self, NewArtifact, UpdateOptions};
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
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
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

fn csp(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
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
            description: "",
            favicon: "",
            label: None,
        },
        None,
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
            description: "",
            favicon: "",
            label: None,
        },
        None,
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
async fn rendering_a_public_artifact_serves_its_html_sandboxed() {
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
    assert!(
        csp(&response).contains("sandbox"),
        "the artifact page is sandboxed"
    );

    let body = text_body(response).await;
    assert!(body.contains("public-artifact-body"));
    assert!(
        body.contains("sandbox srcdoc"),
        "agent-authored HTML renders in a sandboxed frame"
    );
}

#[tokio::test]
async fn rendering_a_markdown_artifact_renders_and_escapes_it() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Notes",
            kind: "markdown",
            content: b"# Runbook\n\nSteps to deploy safely.\n\n<script>alert(1)</script>",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
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
    assert!(
        csp(&response).contains("sandbox"),
        "the artifact page is sandboxed"
    );

    let body = text_body(response).await;
    assert!(
        body.contains("<h1>Runbook</h1>"),
        "the heading is rendered as a heading element"
    );
    assert!(
        !body.contains("# Runbook"),
        "the literal heading marker is not shown"
    );
    assert!(body.contains("<p>Steps to deploy safely.</p>"));
    assert!(
        !body.contains("<script>alert(1)</script>"),
        "raw HTML in the markdown must be escaped, not interpreted"
    );
    assert!(
        body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "the escaped script source is shown as text"
    );
    assert_eq!(
        body.matches("<h1>").count(),
        1,
        "the artifact title is not repeated above its own heading"
    );
}

#[tokio::test]
async fn rendering_a_markdown_artifact_without_a_heading_shows_the_title() {
    let state = state().await;
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Bare note",
            kind: "markdown",
            content: b"Just a paragraph.",
            envelope: None,
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish markdown")
    .id;

    let app = router(state);
    let response = app
        .oneshot(request("GET", &format!("/artifacts/{id}"), None, None))
        .await
        .expect("request");
    let body = text_body(response).await;
    assert!(
        body.contains("<h1>Bare note</h1>"),
        "a markdown artifact with no heading still shows its title"
    );
    assert!(body.contains("<p>Just a paragraph.</p>"));
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
    sessions::end(&state.db, &session.id, "human")
        .await
        .expect("end");
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

async fn publish_versioned(state: &AppState) -> String {
    let id = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Report",
            kind: "html",
            content: b"<p>v1</p>",
            envelope: None,
            description: "A report",
            favicon: "star",
            label: Some("v1"),
        },
        None,
    )
    .await
    .expect("publish v1")
    .id;
    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &id,
        b"<p>v2</p>",
        None,
        UpdateOptions {
            base_version: None,
            force: false,
            label: Some("v2"),
        },
        None,
    )
    .await
    .expect("publish v2");
    id
}

#[tokio::test]
async fn content_serves_a_version_and_defaults_to_latest() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}?version=1"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"], "<p>v1</p>");
    assert_eq!(body["version"], 1);
    assert_eq!(body["description"], "A report");
    assert_eq!(body["favicon"], "star");
    assert_eq!(body["label"], "v1");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"], "<p>v2</p>");
    assert_eq!(body["version"], 2);
    assert_eq!(body["label"], "v2");
}

#[tokio::test]
async fn raw_serves_a_version() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw?version=1"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        content_type(&response).as_deref(),
        Some("text/plain; charset=utf-8"),
    );
    assert_eq!(text_body(response).await, "<p>v1</p>");
}

#[tokio::test]
async fn content_rejects_unknown_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}?version=0"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}?version=99"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

#[tokio::test]
async fn versions_lists_history_oldest_first() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/versions"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let versions = body["versions"].as_array().expect("versions array");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["version"], 1);
    assert_eq!(versions[1]["version"], 2);
    assert_eq!(versions[0]["label"], "v1");
    assert_eq!(versions[1]["label"], "v2");
    assert!(versions[0]["created_at"].as_str().is_some());
}

#[tokio::test]
async fn raw_serves_text_for_a_public_artifact() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>raw-body</p>").await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = content_type(&response).unwrap_or_default();
    assert!(
        content_type.starts_with("text/plain"),
        "public raw is text, got {content_type}"
    );
    assert_eq!(
        response
            .headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("nosniff"),
    );
    assert!(csp(&response).is_empty(), "raw carries no policy");
    let body = text_body(response).await;
    assert_eq!(body, "<p>raw-body</p>");
}

#[tokio::test]
async fn raw_serves_envelope_and_ciphertext_for_a_protected_artifact() {
    let state = state().await;
    let id = publish_protected(&state, "proj", "Sealed report", CIPHERTEXT.as_bytes()).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response).as_deref(), Some("application/json"),);
    let body = json_body(response).await;
    assert_eq!(body["envelope"]["alg"], "AES-256-GCM");
    assert_eq!(body["ciphertext"], "Y2lwaGVyLW1hcmstN2YzYTlj");
    assert!(
        !body.to_string().contains(PLAINTEXT),
        "raw never carries plaintext"
    );
}

#[tokio::test]
async fn raw_rejects_bad_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw?version=0"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(response).await["code"], "invalid_argument");

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "GET",
            &format!("/api/v1/artifacts/{id}/raw?version=99"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(response).await["code"], "not_found");

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/artifacts/missing/raw",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_removes_the_artifact_and_its_history() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state.clone());
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/artifacts/{id}"),
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["ok"], true);

    for uri in [
        format!("/api/v1/artifacts/{id}"),
        format!("/api/v1/artifacts/{id}/versions"),
        format!("/api/v1/artifacts/{id}/raw"),
        format!("/artifacts/{id}"),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request("GET", &uri, Some("Bearer token"), None))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {uri}");
    }

    let app = router(state);
    let response = app
        .oneshot(request(
            "DELETE",
            "/api/v1/artifacts/missing",
            Some("Bearer token"),
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(response).await["code"], "not_found");
}

#[tokio::test]
async fn new_admin_routes_require_a_token() {
    let state = state().await;
    let id = publish_public(&state, "proj", "Report", b"<p>one</p>").await;

    for (method, uri) in [
        ("GET", format!("/api/v1/artifacts/{id}")),
        ("GET", format!("/api/v1/artifacts/{id}/versions")),
        ("GET", format!("/api/v1/artifacts/{id}/raw")),
        ("DELETE", format!("/api/v1/artifacts/{id}")),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(method, &uri, None, None))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
        let problem = problem_body(response).await;
        assert_eq!(problem["code"], "unauthenticated");
    }
}

#[tokio::test]
async fn render_serves_a_version() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    let app = router(state);
    let response = app
        .oneshot(request(
            "GET",
            &format!("/artifacts/{id}?version=1"),
            None,
            None,
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        csp(&response).contains("sandbox"),
        "a versioned page is still sandboxed"
    );
    let body = text_body(response).await;
    assert!(body.contains("&lt;p&gt;v1&lt;/p&gt;"));
    assert!(!body.contains("v2"));
}

#[tokio::test]
async fn render_rejects_bad_versions() {
    let state = state().await;
    let id = publish_versioned(&state).await;

    for (version, status) in [
        ("0", StatusCode::BAD_REQUEST),
        ("99", StatusCode::NOT_FOUND),
        ("abc", StatusCode::BAD_REQUEST),
    ] {
        let app = router(state.clone());
        let response = app
            .oneshot(request(
                "GET",
                &format!("/artifacts/{id}?version={version}"),
                None,
                None,
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), status, "version {version}");
    }
}

#[test]
fn stale_base_conflict_maps_to_409() {
    let problem = Problem::from_error(&Error::Conflict(
        "artifact abc is at version 2, not base version 1".to_string(),
    ));
    assert_eq!(problem.status, 409);
    assert_eq!(problem.code, "conflict");
}
