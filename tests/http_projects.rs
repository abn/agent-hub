//! HTTP project routes: what the listing carries and what a project holds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::projects;
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
        "agent-hub-http-projects-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
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

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    router(state.clone())
        .oneshot(request(method, uri, Some("Bearer token"), body))
        .await
        .expect("request")
}

async fn signal(state: &AppState, project_id: &str, summary: &str) -> String {
    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: project_id.to_string(),
            kind: "signal".to_string(),
            summary: summary.to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append signal")
}

/// The unseen count each project carries in the listing.
async fn listed_unseen(state: &AppState) -> Vec<(String, i64)> {
    let body = json_body(call(state, "GET", "/api/v1/projects", None).await).await;
    body["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|project| {
            (
                project["id"].as_str().expect("id").to_string(),
                project["unseen_events"].as_i64().expect("unseen events"),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_listing_counts_what_the_human_has_not_seen() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    projects::create(&state.db, "workshop", "Workshop")
        .await
        .expect("create project");
    assert_eq!(
        listed_unseen(&state).await,
        vec![("homelab".to_string(), 0), ("workshop".to_string(), 0)],
        "a project with no events has nothing unseen"
    );

    signal(&state, "homelab", "first").await;
    let second = signal(&state, "homelab", "second").await;
    signal(&state, "workshop", "elsewhere").await;
    assert_eq!(
        listed_unseen(&state).await,
        vec![("homelab".to_string(), 2), ("workshop".to_string(), 1)]
    );

    let response = call(
        &state,
        "POST",
        "/api/v1/projects/homelab/feed/seen",
        Some(serde_json::json!({ "event_id": second })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        listed_unseen(&state).await,
        vec![("homelab".to_string(), 0), ("workshop".to_string(), 1)],
        "opening a feed clears its count and leaves the others alone"
    );

    signal(&state, "homelab", "third").await;
    assert_eq!(
        listed_unseen(&state).await,
        vec![("homelab".to_string(), 1), ("workshop".to_string(), 1)],
        "an event written after the cursor is unseen again"
    );

    // Home carries the same numbers, so its newest rows can draw the same dot.
    let home = json_body(call(&state, "GET", "/api/v1/home", None).await).await;
    let mut unseen: Vec<(String, i64)> = home["unseen"]
        .as_array()
        .expect("unseen")
        .iter()
        .map(|row| {
            (
                row["project_id"].as_str().expect("project").to_string(),
                row["events"].as_i64().expect("events"),
            )
        })
        .collect();
    unseen.sort();
    assert_eq!(
        unseen,
        vec![("homelab".to_string(), 1), ("workshop".to_string(), 1)]
    );
}

#[tokio::test]
async fn the_listing_needs_a_token() {
    let state = state().await;
    let response = router(state)
        .oneshot(request("GET", "/api/v1/projects", None, None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(problem_body(response).await["code"], "unauthenticated");
}
