//! Hardening: the artifact cap lives in the blob layer, each surface reads
//! bodies up to its own limit, and a pruned brain file is removed through the
//! wrapper so it cannot race a write.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::blob;
use agent_hub::brain::BrainStore;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::error::ErrorCode;
use agent_hub::http::router;
use agent_hub::limits::{AGENT_BODY_BYTES_MAX, ARTIFACT_BYTES_MAX, REQUEST_BODY_BYTES_MAX};
use agent_hub::mcp::http_router;
use agent_hub::store::projects;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

const PROTOCOL_VERSION: &str = "2025-06-18";

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn state(tag: &str) -> AppState {
    AppState::open(Config {
        data_dir: temp_dir(tag),
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

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::AUTHORIZATION, "Bearer token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

async fn detail(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let value: Value = serde_json::from_slice(&bytes).expect("problem json");
    value["detail"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn the_rest_surface_reads_bodies_up_to_the_hub_limit() {
    let state = state("hardening-body-limit").await;

    // Over the framework default and under the hub's own limit: the handler
    // must see the body and judge it on its content.
    let under = json!({"id": "pad", "display_name": "x".repeat(3 * 1024 * 1024)});
    let response = router(state.clone())
        .oneshot(post("/api/v1/projects", under))
        .await
        .expect("request");
    let under = detail(response).await;
    assert!(
        under.contains("display name is too long"),
        "the body reached the handler: {under}"
    );

    // Over the hub limit: refused for its size, before the handler.
    let over = json!({"id": "pad", "display_name": "x".repeat(REQUEST_BODY_BYTES_MAX)});
    let response = router(state)
        .oneshot(post("/api/v1/projects", over))
        .await
        .expect("request");
    let over = detail(response).await;
    assert!(
        over.contains("length limit"),
        "the body was refused for its size: {over}"
    );
}

fn mcp_post(session: Option<&str>, body: String) -> Request<Body> {
    let mut builder = Request::builder()
        .uri("/mcp")
        .method("POST")
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, "Bearer token")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body)).expect("build request")
}

/// Initialize a streamable HTTP session and return its id.
async fn mcp_session(app: &axum::Router) -> String {
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "hardening", "version": "0.0.0"},
        },
    });
    let response = app
        .clone()
        .oneshot(mcp_post(None, initialize.to_string()))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK, "the session opens");
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session id")
        .to_string();

    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    app.clone()
        .oneshot(mcp_post(Some(&session), initialized.to_string()))
        .await
        .expect("request");

    session
}

async fn text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn the_agent_transport_carries_an_artifact_over_the_rest_limit() {
    let state = state("hardening-agent-body").await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    let app = http_router(state);
    let session = mcp_session(&app).await;

    // An artifact larger than a REST body is the point of the agent limit: the
    // content travels as a tool argument, so the transport must not refuse it.
    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "artifact_publish",
            "arguments": {
                "project_id": "homelab",
                "title": "Big page",
                "kind": "html",
                "content": "x".repeat(REQUEST_BODY_BYTES_MAX + 1024 * 1024),
            },
        },
    });
    let response = app
        .clone()
        .oneshot(mcp_post(Some(&session), call.to_string()))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK, "the publish is accepted");
    let body = text(response).await;
    assert!(
        body.contains("artifact_id"),
        "the artifact is stored: {body}"
    );
}

#[tokio::test]
async fn the_agent_transport_refuses_a_body_over_its_limit() {
    let state = state("hardening-agent-body-over").await;
    let app = http_router(state);

    // Under the limit the body is read and judged as a message; over it the
    // transport refuses on size alone.
    let under = app
        .clone()
        .oneshot(mcp_post(None, "x".repeat(AGENT_BODY_BYTES_MAX)))
        .await
        .expect("request");
    assert_ne!(under.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let over = app
        .oneshot(mcp_post(None, "x".repeat(AGENT_BODY_BYTES_MAX + 1)))
        .await
        .expect("request");
    assert_eq!(over.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// The served router is the merge of the two, so the two limits have to
/// survive being merged: a body the REST surface refuses on size must still
/// reach the agent transport.
#[tokio::test]
async fn the_merged_router_keeps_both_body_limits() {
    let state = state("hardening-merged-limits").await;
    let app = router(state.clone()).merge(http_router(state));

    // Over the REST limit, well under the agent one.
    let padding = "x".repeat(REQUEST_BODY_BYTES_MAX + 1024);
    let rest = app
        .clone()
        .oneshot(post(
            "/api/v1/projects",
            json!({"id": "pad", "display_name": padding}),
        ))
        .await
        .expect("request");
    assert_eq!(rest.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        rest.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
        "the REST refusal is problem details"
    );

    let agent = app.oneshot(mcp_post(None, padding)).await.expect("request");
    assert_ne!(
        agent.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "the same size is not too large for the agent transport"
    );
}

#[test]
fn the_blob_layer_enforces_the_artifact_cap() {
    let dir = temp_dir("hardening-blob");

    let over = vec![0u8; ARTIFACT_BYTES_MAX + 1];
    let err = blob::write(&dir, "proj", "art", 1, "html", &over).expect_err("over the cap");
    assert_eq!(err.code(), ErrorCode::PayloadTooLarge);

    blob::write(&dir, "proj", "art", 1, "html", b"a small blob").expect("under the cap");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn removing_a_brain_file_goes_through_the_store() {
    let dir = temp_dir("hardening-brain");
    let store = BrainStore::new(dir.join("sessions"));

    let brain = store.open("proj", "s1").await.expect("open");
    brain.put("/kv/note", b"state").await.expect("put");
    let path = store.brain_path("proj", "s1").expect("path");
    assert!(path.exists(), "the brain file was written");

    assert!(
        store.remove("proj", "s1").await.expect("remove"),
        "a file existed and was removed"
    );
    assert!(!path.exists(), "the brain file is gone");
    assert!(
        !store.remove("proj", "s1").await.expect("remove again"),
        "a second removal reports nothing was there"
    );

    let reopened = store.open("proj", "s1").await.expect("reopen");
    assert!(
        reopened.get("/kv/note").await.expect("get").is_none(),
        "a removed brain starts empty"
    );

    drop(reopened);
    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}
