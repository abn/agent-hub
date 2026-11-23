//! The enrol-only MCP surface: a caller with no credential, or one waiting for
//! approval, sees `enrol` and `whoami` and the bootstrap resource, and the
//! hub answers every other call with the reason it is refused.

use agent_hub::app::AppState;
use agent_hub::mcp::http_router;
use agent_hub::store::{identity, projects};
use axum::Router;
use axum::body::Body;
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
    next: u64,
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
            next: 0,
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

    /// Send one request and return the JSON-RPC message that answers it.
    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = self
            .app
            .clone()
            .oneshot(
                self.builder("POST")
                    .body(Body::from(body.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("request");
        let status = response.status();
        let body = text(response).await;
        let parsed = body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
            .find(|message| message["id"] == id)
            .or_else(|| serde_json::from_str::<Value>(&body).ok());
        parsed.unwrap_or_else(|| panic!("no answer to {method} ({status}): {body}"))
    }

    /// Call a tool and return the whole JSON-RPC message.
    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }

    /// Call a tool that must succeed and return its structured result.
    async fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let message = self.tool(name, arguments).await;
        message
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} did not succeed: {message}"))
    }
}

/// The `details.reason` of a refused call, from either error shape.
fn reason(message: &Value) -> String {
    message
        .pointer("/error/data/error/details/reason")
        .or_else(|| message.pointer("/result/structuredContent/error/details/reason"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no reason in {message}"))
        .to_string()
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

/// A networked hub: its bind address is not loopback, so a caller with no
/// token is anonymous rather than the open loopback member.
async fn networked(tag: &str, enrol_enabled: bool) -> (common::state::TestState, Router) {
    let state = common::state::open_with(tag, |config| {
        config.bind = "192.0.2.10:47318".parse().expect("socket address");
        config.enrol_enabled = enrol_enabled;
    })
    .await;
    projects::create(&state.db, "homelab", "Homelab")
        .await
        .expect("create project");
    let app = http_router((*state).clone());
    (state, app)
}

async fn tool_names(mcp: &mut Mcp) -> Vec<String> {
    let listed = mcp.rpc("tools/list", json!({})).await;
    let mut names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn an_anonymous_caller_sees_the_enrol_surface_only() {
    let (_state, app) = networked("enrol-surface-anonymous", true).await;
    let mut mcp = Mcp::connect(&app, None, None, "claude-code").await;
    assert_eq!(tool_names(&mut mcp).await, vec!["enrol", "whoami"]);

    let refused = mcp
        .tool("feed_read", json!({"project_id": "homelab"}))
        .await;
    assert_eq!(reason(&refused), "token_missing");

    let who = mcp.ok("whoami", json!({})).await;
    assert_eq!(who["state"], "anonymous");
    assert_eq!(who["enrol"], true);

    let resources = mcp.rpc("resources/list", json!({})).await;
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect();
    assert_eq!(uris, vec!["agenthub://skill"]);
}

#[tokio::test]
async fn enrolling_returns_a_pending_token_that_sees_the_same_surface() {
    let (state, app) = networked("enrol-surface-enrol", true).await;
    let mut mcp = Mcp::connect(&app, None, None, "claude-code").await;
    let enrolled = mcp
        .ok(
            "enrol",
            json!({"member": "laptop", "display_name": "Laptop", "reason": "my laptop", "host": "laptop"}),
        )
        .await;
    assert_eq!(enrolled["status"], "pending");
    assert_eq!(enrolled["member"], "laptop");
    let token = enrolled["token"].as_str().expect("token").to_string();
    assert!(
        enrolled["status_url"]
            .as_str()
            .is_some_and(|url| url.ends_with("/api/v1/enrol/status"))
    );

    let mut pending = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    assert_eq!(tool_names(&mut pending).await, vec!["enrol", "whoami"]);
    let who = pending.ok("whoami", json!({})).await;
    assert_eq!(who["state"], "pending");
    assert_eq!(who["member"], "laptop");
    let refused = pending
        .tool("feed_read", json!({"project_id": "homelab"}))
        .await;
    assert_eq!(reason(&refused), "token_pending");

    identity::approve_member(&state.db, "laptop", None, None, None)
        .await
        .expect("approve");
    let mut approved = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let names = tool_names(&mut approved).await;
    assert!(names.contains(&"feed_read".to_string()), "{names:?}");
    assert!(
        !names.contains(&"enrol".to_string()),
        "a member does not enrol"
    );
    assert_eq!(approved.ok("whoami", json!({})).await["state"], "active");
}

#[tokio::test]
async fn enrolling_is_refused_when_the_hub_has_it_off() {
    let (_state, app) = networked("enrol-surface-off", false).await;
    let mut mcp = Mcp::connect(&app, None, None, "claude-code").await;
    let refused = mcp
        .tool("enrol", json!({"member": "laptop", "reason": "my laptop"}))
        .await;
    assert_eq!(reason(&refused), "enrol_disabled");
}

#[tokio::test]
async fn enrolling_checks_the_reason_like_the_rest_route() {
    let (_state, app) = networked("enrol-surface-reason", true).await;
    let mut mcp = Mcp::connect(&app, None, None, "claude-code").await;
    for bad in ["", "two\nlines", &"x".repeat(201)] {
        let refused = mcp
            .tool("enrol", json!({"member": "laptop", "reason": bad}))
            .await;
        assert_eq!(
            refused
                .pointer("/error/data/error/code")
                .and_then(Value::as_str),
            Some("invalid_argument"),
            "{bad:?}: {refused}"
        );
    }
}

#[tokio::test]
async fn a_revoked_or_unknown_token_is_refused_with_its_reason() {
    let (state, app) = networked("enrol-surface-refused", true).await;
    identity::create_member(&state.db, "laptop", "Laptop")
        .await
        .expect("create member");
    let (issued, _) = identity::issue_member_token(&state.db, "laptop")
        .await
        .expect("issue");
    let hashes = identity::revoke_member_token(&state.db, "laptop")
        .await
        .expect("revoke");
    assert!(!hashes.is_empty());
    for (token, expected) in [
        (issued.token.as_str(), "token_revoked"),
        ("not-a-token", "token_unknown"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::HOST, "127.0.0.1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from("{}"))
                    .expect("build request"),
            )
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body: Value = serde_json::from_str(&text(response).await).expect("json");
        assert_eq!(body["error"]["details"]["reason"], expected);
    }
}

#[tokio::test]
async fn whoami_has_one_shape_per_state() {
    let (state, app) = networked("enrol-surface-whoami", true).await;
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let who = mcp.ok("whoami", json!({})).await;
    for key in [
        "state",
        "hub_url",
        "member",
        "agent",
        "kind",
        "session",
        "default_project",
        "suggested_session",
        "personal_project",
        "admin",
        "skill_url",
        "actor",
    ] {
        assert!(who.get(key).is_some(), "whoami carries {key}: {who}");
    }
    assert_eq!(who["state"], "active");
    assert!(who["session"].is_null());
    assert_eq!(who["default_project"], who["personal_project"]);
    assert!(
        who["suggested_session"]
            .as_str()
            .is_some_and(|name| name.starts_with("claude-code-"))
    );

    let started = mcp
        .ok(
            "session_start",
            json!({"project_id": "homelab", "session_name": "feat-x"}),
        )
        .await;
    let inside = mcp
        .ok(
            "whoami",
            json!({"session": {"session_id": started["session_id"]}}),
        )
        .await;
    assert_eq!(inside["session"]["name"], "feat-x");
    assert_eq!(inside["default_project"], "homelab");
}

#[tokio::test]
async fn a_pending_token_naming_a_session_joins_and_creates_nothing() {
    let (state, app) = networked("enrol-surface-pending-session", true).await;
    let token = member(&state, "desktop").await;
    let mut active = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let started = active
        .ok(
            "session_start",
            json!({"project_id": "homelab", "session_name": "feat-x"}),
        )
        .await;
    let session_id = started["session_id"].as_str().expect("id").to_string();

    let mut anonymous = Mcp::connect(&app, None, None, "claude-code").await;
    let enrolled = anonymous
        .ok(
            "enrol",
            json!({"member": "laptop", "display_name": "Laptop", "reason": "my laptop"}),
        )
        .await;
    let pending_token = enrolled["token"].as_str().expect("token").to_string();
    let mut pending = Mcp::connect(&app, Some(&pending_token), None, "claude-code").await;

    // A plugin passes `session` on every call, whoami included: naming a live
    // session, or one that does not exist yet, makes the pending caller no
    // participant and creates no session.
    for session in [
        json!({"session_id": session_id}),
        json!({"project_id": "homelab", "name": "feat-x"}),
        json!({"project_id": "homelab", "name": "brand-new"}),
    ] {
        let who = pending.ok("whoami", json!({"session": session})).await;
        assert_eq!(who["state"], "pending", "{who}");
    }
    let participants = agent_hub::store::sessions::participants(&state.db, &session_id)
        .await
        .expect("participants");
    assert!(
        participants.iter().all(|entry| entry.member != "laptop"),
        "the pending member joined nothing: {participants:?}"
    );
    assert!(
        agent_hub::store::sessions::lookup_name(&state.db, "homelab", "brand-new")
            .await
            .expect("lookup")
            .is_none(),
        "no session was created for a pending caller"
    );

    // A write naming a new session is refused for the credential.
    let refused = pending
        .tool(
            "signal_append",
            json!({"project_id": "homelab", "kind": "signal", "summary": "hi",
                   "session": {"project_id": "homelab", "name": "brand-new"}}),
        )
        .await;
    assert_eq!(reason(&refused), "token_pending");
}
