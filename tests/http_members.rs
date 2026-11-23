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
    common::state::open("members").await
}

fn request(method: &str, uri: &str, auth: Option<&str>) -> Request<Body> {
    common::http::request(method, uri, auth, None)
}

#[tokio::test]
async fn the_agents_surface_rejects_a_missing_token() {
    let state = state().await;
    let app = router(state.clone());
    for (method, uri) in [
        ("GET", "/api/v1/members"),
        ("POST", "/api/v1/members/one/token"),
        ("DELETE", "/api/v1/members/one/token"),
        ("GET", "/api/v1/members/one/grants"),
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
            "/api/v1/members",
            Some("Bearer not-the-admin"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let problem = common::http::problem_body(response).await;
    assert_eq!(problem["details"]["reason"], "token_unknown", "{problem}");
}

#[tokio::test]
async fn a_missing_token_names_its_reason() {
    let state = state().await;
    let response = router(state.clone())
        .oneshot(request("GET", "/api/v1/members", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let problem = common::http::problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["details"]["reason"], "token_missing", "{problem}");
}

#[tokio::test]
async fn the_old_agent_routes_are_gone() {
    let state = state().await;
    let app = router(state.clone());
    for (method, uri) in [
        ("GET", "/api/v1/agents"),
        ("POST", "/api/v1/agents"),
        ("POST", "/api/v1/agents/worker/token"),
        ("GET", "/api/v1/agents/worker/grants"),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, uri, Some("Bearer token")))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn a_new_member_id_is_plain_lowercase_and_not_reserved() {
    let state = state().await;
    let app = router(state.clone());
    for id in ["Worker", "has space", "human", "local", "loopback", "-lead"] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/members",
                Some("Bearer token"),
                &format!(r#"{{"id":"{id}","display_name":"X"}}"#),
            ))
            .await
            .expect("create");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{id}");
    }
}

#[tokio::test]
async fn reissuing_and_revoking_tell_the_streams_the_old_token_held_to_close() {
    let state = state().await;
    let app = router(state.clone());
    let auth = Some("Bearer token");
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/members",
            auth,
            r#"{"id":"worker","display_name":"Worker"}"#,
        ))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::CREATED);
    let issue = || request("POST", "/api/v1/members/worker/token", auth);
    let first = json_body(app.clone().oneshot(issue()).await.expect("issue")).await["token"]
        .as_str()
        .expect("token")
        .to_string();

    let mut revoked = state.revocations.subscribe();
    let second = json_body(app.clone().oneshot(issue()).await.expect("reissue")).await["token"]
        .as_str()
        .expect("token")
        .to_string();
    assert_eq!(
        revoked
            .recv()
            .await
            .expect("the replaced token is announced"),
        agent_hub::store::identity::hash_token(&first)
    );

    let response = app
        .clone()
        .oneshot(request("DELETE", "/api/v1/members/worker/token", auth))
        .await
        .expect("revoke");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        revoked
            .recv()
            .await
            .expect("the revoked token is announced"),
        agent_hub::store::identity::hash_token(&second)
    );
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
            "/api/v1/members",
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
        .oneshot(request("GET", "/api/v1/members", auth))
        .await
        .expect("list");
    assert_eq!(response.status(), StatusCode::OK);
    let listed = json_body(response).await;
    assert_eq!(listed["members"].as_array().expect("members").len(), 1);

    let response = app
        .clone()
        .oneshot(request("POST", "/api/v1/members/worker/token", auth))
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
        .oneshot(request("GET", "/api/v1/members", Some(&agent_bearer)))
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
            "/api/v1/members/worker/grants",
            auth,
            r#"{"project_id":"proj"}"#,
        ))
        .await
        .expect("grant");
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/members/worker/grants", auth))
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
        .oneshot(request(
            "DELETE",
            "/api/v1/members/worker/grants/proj",
            auth,
        ))
        .await
        .expect("ungrant");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(request("DELETE", "/api/v1/members/worker/token", auth))
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
        .oneshot(json_request("POST", "/api/v1/members", auth, create))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/members", auth, create))
        .await
        .expect("duplicate");
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/members", auth, "{not json"))
        .await
        .expect("malformed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/members",
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
            "/api/v1/members/worker/grants",
            auth,
            r#"{"project_id":"ghost"}"#,
        ))
        .await
        .expect("grant to a missing project");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(request("GET", "/api/v1/members/ghost/grants", auth))
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
            "/api/v1/members",
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
            "/api/v1/members/worker",
            auth,
            r#"{"trust":"untrusted"}"#,
        ))
        .await
        .expect("patch");
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "PATCH /api/v1/members/{{id}} route must be removed"
    );
}
