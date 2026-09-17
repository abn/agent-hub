//! The agents control surface: admin-gated lifecycle over REST.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::projects;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state_with(trust_default: TrustDefault) -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-agents-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        admin_token: Some("token".to_string()),
        trust_default,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
    })
    .await
    .expect("open state")
}

async fn state() -> AppState {
    state_with(TrustDefault::Trusted).await
}

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method(method);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder.body(Body::empty()).expect("build request")
}

fn json_request(method: &str, uri: &str, auth: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("build request")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn the_agents_surface_rejects_a_missing_token() {
    let app = router(state().await);
    for (method, uri) in [
        ("GET", "/api/v1/agents"),
        ("POST", "/api/v1/agents/one/token"),
        ("DELETE", "/api/v1/agents/one/token"),
        ("GET", "/api/v1/agents/one/grants"),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, uri, None))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must require a token"
        );
    }
}

#[tokio::test]
async fn the_agents_surface_rejects_a_foreign_token() {
    let app = router(state().await);
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/agents",
            Some("Bearer not-the-admin"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_agents_surface_manages_agents_tokens_and_grants() {
    let state = state().await;
    projects::create(&state.db, "proj", "Project")
        .await
        .expect("project");
    let app = router(state);
    let auth = Some("Bearer token");

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/agents",
            auth,
            r#"{"id":"worker","display_name":"Worker"}"#,
        ))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::CREATED);
    let agent = json_body(response).await;
    assert_eq!(agent["id"], "worker");
    assert!(
        agent["personal_project_id"]
            .as_str()
            .expect("personal space")
            .starts_with("space-")
    );

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/agents", auth))
        .await
        .expect("list");
    assert_eq!(response.status(), StatusCode::OK);
    let listed = json_body(response).await;
    assert_eq!(listed["agents"].as_array().expect("agents").len(), 1);

    let response = app
        .clone()
        .oneshot(json_request(
            "PATCH",
            "/api/v1/agents/worker",
            auth,
            r#"{"trust":"untrusted"}"#,
        ))
        .await
        .expect("patch");
    assert_eq!(response.status(), StatusCode::OK);
    let updated = json_body(response).await;
    assert_eq!(updated["trust"], "untrusted");

    let response = app
        .clone()
        .oneshot(request("POST", "/api/v1/agents/worker/token", auth))
        .await
        .expect("issue token");
    assert_eq!(response.status(), StatusCode::CREATED);
    let issued = json_body(response).await;
    let token = issued["token"].as_str().expect("token").to_string();
    assert_eq!(token.len(), 64);
    assert!(
        issued.get("token_hash").is_none(),
        "the hash is internal, not part of the response"
    );

    let agent_bearer = format!("Bearer {token}");
    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/agents", Some(&agent_bearer)))
        .await
        .expect("agent token on REST");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an agent token is rejected on the control surface"
    );

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/agents/worker/grants",
            auth,
            r#"{"project_id":"proj","access":"read"}"#,
        ))
        .await
        .expect("grant");
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/agents/worker/grants", auth))
        .await
        .expect("grants");
    assert_eq!(response.status(), StatusCode::OK);
    let grants = json_body(response).await;
    assert_eq!(grants["grants"].as_array().expect("grants").len(), 1);

    let response = app
        .clone()
        .oneshot(request("DELETE", "/api/v1/agents/worker/grants/proj", auth))
        .await
        .expect("ungrant");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(request("DELETE", "/api/v1/agents/worker/token", auth))
        .await
        .expect("revoke");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_new_agent_follows_the_strict_default() {
    let app = router(state_with(TrustDefault::Untrusted).await);
    let response = app
        .oneshot(json_request(
            "POST",
            "/api/v1/agents",
            Some("Bearer token"),
            r#"{"id":"worker","display_name":"Worker"}"#,
        ))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::CREATED);
    let agent = json_body(response).await;
    assert_eq!(agent["trust"], "untrusted");
}

#[tokio::test]
async fn the_agents_surface_maps_store_errors() {
    let state = state().await;
    projects::create(&state.db, "proj", "Project")
        .await
        .expect("project");
    let app = router(state);
    let auth = Some("Bearer token");
    let create = r#"{"id":"worker","display_name":"Worker"}"#;

    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/agents", auth, create))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/agents", auth, create))
        .await
        .expect("duplicate");
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/agents", auth, "{not json"))
        .await
        .expect("malformed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/agents",
            auth,
            r#"{"id":"other","display_name":"Other","trust":"owner"}"#,
        ))
        .await
        .expect("unknown trust");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            "PATCH",
            "/api/v1/agents/ghost",
            auth,
            r#"{"trust":"trusted"}"#,
        ))
        .await
        .expect("patch unknown");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/agents/worker/grants",
            auth,
            r#"{"project_id":"ghost","access":"read"}"#,
        ))
        .await
        .expect("grant to a missing project");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(request("GET", "/api/v1/agents/ghost/grants", auth))
        .await
        .expect("grants for an unknown agent");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
