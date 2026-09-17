//! The PWA shell and the projects and storage routes it depends on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::artifacts::{self, NewArtifact};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

static NEXT: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-web-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("addr"),
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
    })
    .await
    .expect("open state")
}

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    request("GET", uri, auth, None)
}

fn request(method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
}

async fn text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn json(response: axum::response::Response) -> Value {
    serde_json::from_str(&text(response).await).expect("json")
}

#[tokio::test]
async fn serves_the_pwa_shell() {
    let app = router(state().await);
    let response = app.oneshot(get("/", None)).await.expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("Agent Hub"), "the shell renders");
    assert!(body.contains("/app.js"), "the shell loads the app");
}

#[tokio::test]
async fn serves_the_tokens() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/tokens.css", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("--accent"));
}

#[tokio::test]
async fn projects_create_list_and_storage() {
    let state = state().await;

    let app = router(state.clone());
    let created = app
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            Some("Bearer token"),
            Some(serde_json::json!({"id": "homelab", "display_name": "Homelab"})),
        ))
        .await
        .expect("request");
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(json(created).await["id"], "homelab");

    let app = router(state.clone());
    let listed = app
        .oneshot(get("/api/v1/projects", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(json(listed).await["projects"][0]["id"], "homelab");

    let app = router(state.clone());
    let usage = app
        .oneshot(get("/api/v1/storage", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(usage.status(), StatusCode::OK);
    assert!(json(usage).await["total_bytes"].is_number());

    let app = router(state);
    let denied = app
        .oneshot(get("/api/v1/projects", None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn serves_every_shell_asset_with_a_policy() {
    let state = state().await;
    for (path, needle) in [
        ("/app.js", "crypto.mjs"),
        ("/app.css", "var(--"),
        ("/crypto.mjs", "export"),
        ("/manifest.webmanifest", "Agent Hub"),
        ("/sw.js", "caches"),
        ("/icon.svg", "<svg"),
    ] {
        let app = router(state.clone());
        let response = app.oneshot(get(path, None)).await.expect("request");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(text(response).await.contains(needle), "{path} content");
    }

    let app = router(state);
    let response = app.oneshot(get("/", None)).await.expect("request");
    assert!(
        response.headers().contains_key("content-security-policy"),
        "the shell carries a content security policy"
    );
}

#[tokio::test]
async fn serves_artifact_content_for_the_viewer() {
    let state = state().await;
    let published = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Note",
            kind: "markdown",
            content: b"hello",
            envelope: None,
        },
    )
    .await
    .expect("publish");

    let app = router(state.clone());
    let denied = app
        .oneshot(get(&format!("/api/v1/artifacts/{}", published.id), None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let app = router(state.clone());
    let public = app
        .oneshot(get(
            &format!("/api/v1/artifacts/{}", published.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(public.status(), StatusCode::OK);
    let body = json(public).await;
    assert_eq!(body["title"], "Note");
    assert_eq!(body["protected"], false);
    assert_eq!(body["content"], "hello");
    assert!(body["envelope"].is_null());

    let protected = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "human",
            project_id: "proj",
            title: "Secret",
            kind: "markdown",
            content: b"Y2lwaGVy",
            envelope: Some(serde_json::json!({"alg": "AES-256-GCM"})),
        },
    )
    .await
    .expect("publish protected");

    let app = router(state);
    let response = app
        .oneshot(get(
            &format!("/api/v1/artifacts/{}", protected.id),
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let body = json(response).await;
    assert_eq!(body["protected"], true);
    assert_eq!(body["content"], "Y2lwaGVy");
    assert_eq!(body["envelope"]["alg"], "AES-256-GCM");
}
