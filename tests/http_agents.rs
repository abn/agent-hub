//! The agents control surface is admin-only, even while its handlers are stubs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
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
        trust_default: TrustDefault::Trusted,
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

#[tokio::test]
async fn the_agents_surface_rejects_a_missing_token() {
    let app = router(state().await);
    for uri in [
        "/api/v1/agents",
        "/api/v1/agents/one/tokens",
        "/api/v1/agents/one/grants",
    ] {
        let response = app
            .clone()
            .oneshot(request("GET", uri, None))
            .await
            .expect("request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} must require a token"
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
async fn the_agents_surface_admits_the_admin_token() {
    let app = router(state().await);
    let response = app
        .oneshot(request("GET", "/api/v1/agents", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the handlers are still stubs behind the gate"
    );
}
