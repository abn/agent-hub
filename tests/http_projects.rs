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

/// One project as the listing carries it.
async fn listed(state: &AppState, id: &str) -> Value {
    let body = json_body(call(state, "GET", "/api/v1/projects", None).await).await;
    body["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project| project["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} is not in the listing"))
}

#[tokio::test]
async fn a_project_is_renamed_and_keeps_its_id() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    signal(&state, "homelab", "a needle in the feed").await;

    let response = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "display_name": "Home lab" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["id"], "homelab");
    assert_eq!(body["display_name"], "Home lab");

    let project = json_body(call(&state, "GET", "/api/v1/projects/homelab", None).await).await;
    assert_eq!(project["display_name"], "Home lab");
    assert_eq!(listed(&state, "homelab").await["display_name"], "Home lab");

    // The id is what every other surface is keyed by, so the feed and the
    // corpus still answer under it after the rename.
    let feed = json_body(call(&state, "GET", "/api/v1/projects/homelab/feed", None).await).await;
    assert_eq!(feed["events"].as_array().expect("events").len(), 1);
    let found = json_body(call(&state, "GET", "/api/v1/search?q=needle", None).await).await;
    assert_eq!(found["count"], 1);
}

#[tokio::test]
async fn the_artifact_password_policy_is_set_and_read_back() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    assert_eq!(
        listed(&state, "homelab").await["artifact_password_policy"],
        "optional",
        "a new project keeps today's behaviour"
    );

    let response = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "artifact_password_policy": "required" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["artifact_password_policy"],
        "required"
    );

    let project = json_body(call(&state, "GET", "/api/v1/projects/homelab", None).await).await;
    assert_eq!(project["artifact_password_policy"], "required");
    assert_eq!(
        listed(&state, "homelab").await["artifact_password_policy"],
        "required"
    );

    let refused = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "artifact_password_policy": "maybe" })),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(refused).await["code"], "invalid_argument");
    assert_eq!(
        listed(&state, "homelab").await["artifact_password_policy"],
        "required",
        "a refused change writes nothing"
    );
}

#[tokio::test]
async fn a_project_id_is_read_only_after_creation() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");

    let refused = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "id": "workshop", "display_name": "Home lab" })),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(refused).await["code"], "invalid_argument");

    assert_eq!(listed(&state, "homelab").await["display_name"], "Homelab");
    let elsewhere = call(&state, "GET", "/api/v1/projects/workshop", None).await;
    assert_eq!(elsewhere.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_patch_that_names_nothing_changes_nothing() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");

    // The create route ignores a field it does not know, so this one does too.
    let response = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "retention": "forever" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["display_name"], "Homelab");
    assert_eq!(body["artifact_password_policy"], "optional");

    let empty = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(json_body(empty).await["display_name"], "Homelab");

    let blank = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "display_name": "  " })),
    )
    .await;
    assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(blank).await["code"], "invalid_argument");
}

#[tokio::test]
async fn an_agents_personal_space_is_settable_though_it_cannot_be_deleted() {
    let state = state().await;
    let agent = agent_hub::store::identity::create_agent(
        &state.db,
        "laptop",
        "Laptop",
        agent_hub::principal::Trust::Trusted,
    )
    .await
    .expect("create agent");
    let space = agent.personal_project_id;

    let response = call(
        &state,
        "PATCH",
        &format!("/api/v1/projects/{space}"),
        Some(serde_json::json!({
            "display_name": "Laptop scratch",
            "artifact_password_policy": "required"
        })),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a personal space is a project the human can still set up"
    );
    let body = json_body(response).await;
    assert_eq!(body["display_name"], "Laptop scratch");
    assert_eq!(body["artifact_password_policy"], "required");

    let undeletable = call(&state, "DELETE", &format!("/api/v1/projects/{space}"), None).await;
    assert_eq!(
        undeletable.status(),
        StatusCode::CONFLICT,
        "deleting it is still the agent's business, not a setting"
    );
}

#[tokio::test]
async fn patching_an_unknown_project_is_not_found_and_needs_a_token() {
    let state = state().await;
    let missing = call(
        &state,
        "PATCH",
        "/api/v1/projects/no-such-project",
        Some(serde_json::json!({ "display_name": "Nowhere" })),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(missing).await["code"], "not_found");

    let denied = router(state)
        .oneshot(request(
            "PATCH",
            "/api/v1/projects/homelab",
            None,
            Some(serde_json::json!({ "display_name": "Nowhere" })),
        ))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_patch_that_writes_nothing_does_not_wake_the_clients() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");

    let quiet = state.generation();
    let response = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.generation(),
        quiet,
        "a patch that changes nothing is not news"
    );

    let response = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "display_name": "Home lab" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(state.generation(), quiet, "a real change is");
}
