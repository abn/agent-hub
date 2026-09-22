//! HTTP project routes: what the listing carries and what a project holds.

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::projects;
use axum::body::to_bytes;
use axum::http::StatusCode;
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::http::{json_body, problem_body, request};
use common::state::TestState;

async fn state() -> TestState {
    common::state::open("http-projects").await
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    call_with_auth(state, method, uri, Some("Bearer token"), body).await
}

async fn call_with_auth(
    state: &AppState,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    router(state.clone())
        .oneshot(request(method, uri, auth, body))
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
    let response = router(state.clone())
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
async fn artifact_password_policy_is_removed_from_projects() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    let initial = listed(&state, "homelab").await;
    assert!(
        initial.get("artifact_password_policy").is_none(),
        "listed project no longer carries artifact_password_policy"
    );

    let response = call(
        &state,
        "PATCH",
        "/api/v1/projects/homelab",
        Some(serde_json::json!({ "artifact_password_policy": "required" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(
        body.get("artifact_password_policy").is_none(),
        "patched project does not carry artifact_password_policy"
    );

    let project = json_body(call(&state, "GET", "/api/v1/projects/homelab", None).await).await;
    assert!(
        project.get("artifact_password_policy").is_none(),
        "fetched project does not carry artifact_password_policy"
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
    let agent = agent_hub::store::identity::create_agent(&state.db, "laptop", "Laptop")
        .await
        .expect("create agent");
    let space = agent.personal_project_id;

    let response = call(
        &state,
        "PATCH",
        &format!("/api/v1/projects/{space}"),
        Some(serde_json::json!({
            "display_name": "Laptop scratch"
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

    let denied = router(state.clone())
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

#[tokio::test]
async fn project_stats_report_threads_agents_written_and_disk_bytes() {
    let state = state().await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");

    let q1 = events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "homelab".to_string(),
            kind: "question".to_string(),
            summary: "event one".to_string(),
            payload: None,
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");

    events::append(
        &state.db,
        "agent-two",
        None,
        NewEvent {
            project_id: "homelab".to_string(),
            kind: "question".to_string(),
            summary: "event two".to_string(),
            payload: None,
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");

    events::append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "homelab".to_string(),
            kind: "answer".to_string(),
            summary: "event three".to_string(),
            payload: None,
            needs_action: false,
            thread_id: Some(q1),
            session_id: None,
        },
    )
    .await
    .expect("append");

    let response = call(&state, "GET", "/api/v1/projects/homelab/stats", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["threads"], 2);
    assert_eq!(body["agents_written"], 2);
    assert!(body["disk_bytes"].is_number());
}

#[tokio::test]
async fn confidential_projects_are_absent_without_grant_and_byte_for_byte_identical_to_404() {
    let state = state().await;
    let agent = agent_hub::store::identity::create_agent(&state.db, "agent-one", "Agent One")
        .await
        .expect("create agent");
    let token = agent_hub::store::identity::issue_token(&state.db, &agent.id)
        .await
        .expect("issue token")
        .token;
    let agent_auth = format!("Bearer {token}");

    // Create an open project
    let res = call(
        &state,
        "POST",
        "/api/v1/projects",
        Some(serde_json::json!({
            "id": "open-work",
            "display_name": "Open Work"
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let open_proj = json_body(res).await;
    assert_eq!(open_proj["confidential"], false);

    // Create a confidential project
    let res = call(
        &state,
        "POST",
        "/api/v1/projects",
        Some(serde_json::json!({
            "id": "secret-work",
            "display_name": "Secret Work",
            "confidential": true
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let secret_proj = json_body(res).await;
    assert_eq!(secret_proj["confidential"], true);

    // Admin lists projects: sees open-work and secret-work
    let res = call(&state, "GET", "/api/v1/projects", None).await;
    assert_eq!(res.status(), StatusCode::OK);
    let admin_list = json_body(res).await;
    let admin_ids: Vec<&str> = admin_list["projects"]
        .as_array()
        .expect("array")
        .iter()
        .map(|p| p["id"].as_str().expect("id"))
        .collect();
    assert!(admin_ids.contains(&"open-work"));
    assert!(admin_ids.contains(&"secret-work"));

    // Agent lists projects: sees open-work and its own space, but NOT secret-work
    let res = call_with_auth(&state, "GET", "/api/v1/projects", Some(&agent_auth), None).await;
    assert_eq!(res.status(), StatusCode::OK);
    let agent_list = json_body(res).await;
    let agent_ids: Vec<&str> = agent_list["projects"]
        .as_array()
        .expect("array")
        .iter()
        .map(|p| p["id"].as_str().expect("id"))
        .collect();
    assert!(agent_ids.contains(&"open-work"));
    assert!(agent_ids.contains(&agent.personal_project_id.as_str()));
    assert!(
        !agent_ids.contains(&"secret-work"),
        "confidential project must be absent from agent listing"
    );

    // Direct read of nonexistent project:
    let res_ghost = call_with_auth(
        &state,
        "GET",
        "/api/v1/projects/nonexistent",
        Some(&agent_auth),
        None,
    )
    .await;
    assert_eq!(res_ghost.status(), StatusCode::NOT_FOUND);
    let ghost_headers = res_ghost.headers().clone();
    let ghost_bytes = to_bytes(res_ghost.into_body(), usize::MAX)
        .await
        .expect("ghost bytes");

    // Direct read of confidential project by agent without grant:
    // It must return what a nonexistent project returns with identical 404 problem details
    let res_secret = call_with_auth(
        &state,
        "GET",
        "/api/v1/projects/secret-work",
        Some(&agent_auth),
        None,
    )
    .await;
    assert_eq!(res_secret.status(), StatusCode::NOT_FOUND);
    let secret_headers = res_secret.headers().clone();
    let secret_bytes = to_bytes(res_secret.into_body(), usize::MAX)
        .await
        .expect("secret bytes");

    assert_eq!(
        secret_headers.get("content-type"),
        ghost_headers.get("content-type")
    );
    let ghost_json: serde_json::Value = serde_json::from_slice(&ghost_bytes).expect("ghost json");
    let secret_json: serde_json::Value =
        serde_json::from_slice(&secret_bytes).expect("secret json");
    assert_eq!(ghost_json["code"], "not_found");
    assert_eq!(secret_json["code"], "not_found");
    assert_eq!(ghost_json["detail"], "project nonexistent not found");
    assert_eq!(secret_json["detail"], "project secret-work not found");

    // To verify byte-for-byte exactness against the same URI when nonexistent:
    // Query a nonexistent project with the exact same id "secret-work" on another clean state
    let clean_state = common::state::open("http-projects-clean").await;
    let clean_agent =
        agent_hub::store::identity::create_agent(&clean_state.db, "agent-one", "Agent One")
            .await
            .expect("clean agent");
    let clean_token = agent_hub::store::identity::issue_token(&clean_state.db, &clean_agent.id)
        .await
        .expect("clean token")
        .token;
    let clean_auth = format!("Bearer {clean_token}");
    let res_clean_ghost = call_with_auth(
        &clean_state,
        "GET",
        "/api/v1/projects/secret-work",
        Some(&clean_auth),
        None,
    )
    .await;
    let clean_ghost_bytes = to_bytes(res_clean_ghost.into_body(), usize::MAX)
        .await
        .expect("clean ghost bytes");
    assert_eq!(
        secret_bytes, clean_ghost_bytes,
        "direct read of a confidential project without grant must be byte-for-byte identical to a nonexistent project"
    );

    // Explicit grant gives agent access:
    agent_hub::store::identity::add_grant(&state.db, "agent-one", "secret-work", "read")
        .await
        .expect("grant");

    // Now agent listing contains secret-work
    let res = call_with_auth(&state, "GET", "/api/v1/projects", Some(&agent_auth), None).await;
    assert_eq!(res.status(), StatusCode::OK);
    let agent_list = json_body(res).await;
    let agent_ids: Vec<&str> = agent_list["projects"]
        .as_array()
        .expect("array")
        .iter()
        .map(|p| p["id"].as_str().expect("id"))
        .collect();
    assert!(agent_ids.contains(&"secret-work"));

    // Direct read succeeds with 200 OK
    let res = call_with_auth(
        &state,
        "GET",
        "/api/v1/projects/secret-work",
        Some(&agent_auth),
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let proj = json_body(res).await;
    assert_eq!(proj["id"], "secret-work");
    assert_eq!(proj["confidential"], true);

    // Revoking grant makes it absent again
    agent_hub::store::identity::remove_grant(&state.db, "agent-one", "secret-work")
        .await
        .expect("revoke");

    let res = call_with_auth(&state, "GET", "/api/v1/projects", Some(&agent_auth), None).await;
    let agent_list = json_body(res).await;
    let agent_ids: Vec<&str> = agent_list["projects"]
        .as_array()
        .expect("array")
        .iter()
        .map(|p| p["id"].as_str().expect("id"))
        .collect();
    assert!(!agent_ids.contains(&"secret-work"));

    let res = call_with_auth(
        &state,
        "GET",
        "/api/v1/projects/secret-work",
        Some(&agent_auth),
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let revoked_bytes = to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("revoked bytes");
    assert_eq!(revoked_bytes, clean_ghost_bytes);
}
