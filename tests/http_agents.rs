//! The agents control surface: admin-gated lifecycle over REST.

use agent_hub::http::router;
use agent_hub::store::projects;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;

use common::http::{json_body, json_request};
use common::state::TestState;

async fn state() -> TestState {
    common::state::open("agents").await
}

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    common::http::request(method, uri, auth, None)
}

#[tokio::test]
async fn the_agents_surface_rejects_a_missing_token() {
    let state = state().await;
    let app = router(state.clone());
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
    let state = state().await;
    let app = router(state.clone());
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
    let app = router(state.clone());
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
            r#"{"project_id":"proj"}"#,
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
    assert!(
        grants["grants"][0].get("access").is_none(),
        "a grant is access, not a level"
    );

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
async fn the_agents_surface_maps_store_errors() {
    let state = state().await;
    projects::create(&state.db, "proj", "Project")
        .await
        .expect("project");
    let app = router(state.clone());
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
            "POST",
            "/api/v1/agents/worker/grants",
            auth,
            r#"{"project_id":"ghost"}"#,
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

#[tokio::test]
async fn create_agent_rejects_trust_field_and_patch_route_is_removed() {
    let state = state().await;
    let app = router(state.clone());
    let auth = Some("Bearer token");

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/agents",
            auth,
            r#"{"id":"worker","display_name":"Worker","trust":"trusted"}"#,
        ))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem = common::http::problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
    let detail = problem["detail"].as_str().expect("detail string");
    assert!(
        detail.contains("trust"),
        "problem details must name the unknown trust field, got: {detail}"
    );

    let response = app
        .oneshot(json_request(
            "PATCH",
            "/api/v1/agents/worker",
            auth,
            r#"{"trust":"untrusted"}"#,
        ))
        .await
        .expect("patch");
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "PATCH /api/v1/agents/{{id}} route must be removed"
    );
}
