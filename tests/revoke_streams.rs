//! Revoking or replacing a token closes the `/mcp` sessions it opened within
//! two seconds, and leaves a replacement token's sessions open. This is the
//! MCP half; the app's event stream has its own test.

use std::time::Duration;

use agent_hub::app::AppState;
use agent_hub::mcp::http_router;
use agent_hub::store::{identity, projects};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

use common::http::text_body as text;
use common::stdio::PROTOCOL_VERSION;

/// One MCP connection to an in-process hub: a bearer, the agent it names in
/// the header (if any), and the handshake name it sends.
struct Mcp {
    app: Router,
    bearer: Option<String>,
    agent_header: Option<String>,
    session: Option<String>,
}

impl Mcp {
    fn builder(&self, method: &str) -> axum::http::request::Builder {
        let mut builder = Request::builder()
            .uri("/mcp")
            .method(method)
            .header(header::HOST, "127.0.0.1")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(bearer) = &self.bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        if let Some(agent) = &self.agent_header {
            builder = builder.header("x-agent-id", agent);
        }
        if let Some(session) = &self.session {
            builder = builder.header("mcp-session-id", session);
        }
        builder
    }

    /// Open a connection and complete the handshake as the named client.
    async fn connect(
        app: &Router,
        bearer: Option<&str>,
        agent_header: Option<&str>,
        client_name: &str,
    ) -> Self {
        let mut mcp = Self {
            app: app.clone(),
            bearer: bearer.map(str::to_string),
            agent_header: agent_header.map(str::to_string),
            session: None,
        };
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": client_name, "version": "0.0.0"},
            },
        });
        let response = mcp
            .app
            .clone()
            .oneshot(
                mcp.builder("POST")
                    .body(Body::from(initialize.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK, "the session opens");
        mcp.session = Some(
            response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .expect("session id")
                .to_string(),
        );
        let _ = text(response).await;
        let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        mcp.app
            .clone()
            .oneshot(
                mcp.builder("POST")
                    .body(Body::from(initialized.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("request");
        mcp
    }
}

/// A member with a token, ready to connect.
async fn member(state: &AppState, id: &str) -> String {
    identity::create_member(&state.db, id, id)
        .await
        .expect("create member");
    identity::issue_member_token(&state.db, id)
        .await
        .expect("issue a token")
        .0
        .token
}

/// Open the server-sent event stream a connection holds for server messages.
async fn stream_of(mcp: &Mcp) -> axum::body::Body {
    let response = mcp
        .app
        .clone()
        .oneshot(
            mcp.builder("GET")
                .header(header::ACCEPT, "text/event-stream")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK, "the stream opens");
    response.into_body()
}

fn send_revoked(state: &AppState, hashes: Vec<String>) {
    assert!(!hashes.is_empty(), "a revoke names the hashes it closed");
    for hash in hashes {
        state.revocations.send(hash).expect("a receiver listens");
    }
}

#[tokio::test]
async fn revoking_a_token_closes_its_streams_in_two_seconds() {
    let state = common::state::open("revoke-streams-close").await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    let app = http_router((*state).clone());
    let token = member(&state, "laptop").await;
    let mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let stream = stream_of(&mcp).await;
    let closed = tokio::spawn(async move { to_bytes(stream, usize::MAX).await.is_ok() });

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !closed.is_finished(),
        "the stream stays open while the token lives"
    );

    let hashes = identity::revoke_member_token(&state.db, "laptop")
        .await
        .expect("revoke");
    send_revoked(&state, hashes);
    tokio::time::timeout(Duration::from_secs(2), closed)
        .await
        .expect("the stream closed within two seconds")
        .expect("the task ended");

    let again = app
        .clone()
        .oneshot(
            mcp.builder("POST")
                .body(Body::from("{}"))
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);
    let body: Value = serde_json::from_str(&text(again).await).expect("json");
    assert_eq!(body["error"]["details"]["reason"], "token_revoked");
}

#[tokio::test]
async fn a_replacement_tokens_stream_survives_the_reissue() {
    let state = common::state::open("revoke-streams-replace").await;
    let app = http_router((*state).clone());
    let old = member(&state, "laptop").await;

    // The old token's session is open before the reissue, as a machine that
    // was working when its token was replaced.
    let opened = Mcp::connect(&app, Some(&old), None, "claude-code").await;
    let old_stream = stream_of(&opened).await;
    let old_closed = tokio::spawn(async move { to_bytes(old_stream, usize::MAX).await.is_ok() });

    let (replacement, hashes) = identity::issue_member_token(&state.db, "laptop")
        .await
        .expect("reissue");
    // The replacement opens its stream before the old token's hash is sent, as
    // a machine moved to a new token does.
    let kept = Mcp::connect(&app, Some(&replacement.token), None, "claude-code").await;
    let kept_stream = stream_of(&kept).await;
    let kept_open = tokio::spawn(async move { to_bytes(kept_stream, usize::MAX).await.is_ok() });

    send_revoked(&state, hashes);
    tokio::time::timeout(Duration::from_secs(2), old_closed)
        .await
        .expect("the replaced token's stream closed")
        .expect("the task ended");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !kept_open.is_finished(),
        "the replacement's stream is still open"
    );
    kept_open.abort();
}
