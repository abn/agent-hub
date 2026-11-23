//! Participants: who a call is, which session it names, and what sharing a
//! session means. Each test drives an in-process hub over its `/mcp` route
//! with member tokens, so the member comes from the token and the agent from
//! the handshake or the header.

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

const PROJECT: &str = "homelab";

async fn hub(tag: &str) -> (common::state::TestState, Router) {
    let state = common::state::open(tag).await;
    projects::create(&state.db, PROJECT, "Homelab")
        .await
        .expect("create project");
    let app = http_router((*state).clone());
    (state, app)
}

fn named(name: &str) -> Value {
    json!({"project_id": PROJECT, "name": name})
}

#[tokio::test]
async fn the_agent_is_the_header_else_the_handshake_else_the_cli() {
    let (state, app) = hub("participants-agent").await;
    let token = member(&state, "laptop").await;

    let mut by_handshake = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let who = by_handshake.ok("whoami", json!({})).await;
    assert_eq!(who["member"], "laptop");
    assert_eq!(who["agent"], "claude-code");
    assert_eq!(who["kind"], "participant");

    let mut by_header = Mcp::connect(&app, Some(&token), Some("opencode"), "claude-code").await;
    assert_eq!(by_header.ok("whoami", json!({})).await["agent"], "opencode");

    let mut unnamed = Mcp::connect(&app, Some(&token), None, "").await;
    assert_eq!(unnamed.ok("whoami", json!({})).await["agent"], "cli");

    let mut unknown = Mcp::connect(&app, Some(&token), None, "agy").await;
    assert_eq!(
        unknown.ok("whoami", json!({})).await["agent"],
        "antigravity"
    );
}

#[tokio::test]
async fn a_session_argument_names_one_session_or_is_refused() {
    let (state, app) = hub("participants-shapes").await;
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let started = mcp
        .ok(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x"}),
        )
        .await;
    let id = started["session_id"].as_str().expect("id").to_string();

    for empty in [json!({}), json!({"project_id": PROJECT})] {
        let refused = mcp
            .tool("brain_list", json!({"store": "session", "session": empty}))
            .await;
        assert_eq!(reason(&refused), "session_invalid");
    }
    let mismatch = mcp
        .tool(
            "brain_list",
            json!({"store": "session", "session": {"session_id": id, "name": "other"}}),
        )
        .await;
    assert_eq!(reason(&mismatch), "session_mismatch");
    let unknown = mcp
        .tool(
            "brain_list",
            json!({"store": "session", "session": {"session_id": "no-such-session"}}),
        )
        .await;
    assert_eq!(reason(&unknown), "session_unknown");
    let without = mcp
        .tool(
            "brain_put",
            json!({"store": "session", "path": "/kv/a", "content": "x"}),
        )
        .await;
    assert_eq!(reason(&without), "session_required");

    for form in [json!({"session_id": id}), named("feat-x")] {
        let listed = mcp
            .ok("brain_list", json!({"store": "session", "session": form}))
            .await;
        assert_eq!(listed["store"], "session");
    }
}

#[tokio::test]
async fn a_write_creates_the_session_it_names_and_a_second_member_joins_it() {
    let (state, app) = hub("participants-join").await;
    let first = member(&state, "laptop").await;
    let second = member(&state, "nas").await;
    let mut a = Mcp::connect(&app, Some(&first), None, "claude-code").await;
    let mut b = Mcp::connect(&app, Some(&second), None, "opencode").await;

    let wrote = a
        .ok(
            "signal_append",
            json!({"project_id": PROJECT, "kind": "signal", "summary": "first", "session": named("feat-x")}),
        )
        .await;
    assert!(
        wrote["session"].is_string(),
        "the write recorded its session: {wrote}"
    );

    let joined = b
        .ok(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x"}),
        )
        .await;
    assert_eq!(
        joined["session_id"], wrote["session"],
        "one name, one session"
    );
    assert_eq!(joined["resumed"], true);

    let listed = a.ok("session_list", json!({"project_id": PROJECT})).await;
    let sessions = listed["sessions"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 1);
    let members: Vec<&str> = sessions[0]["participants"]
        .as_array()
        .expect("participants")
        .iter()
        .filter_map(|p| p["member"].as_str())
        .collect();
    assert!(
        members.contains(&"laptop") && members.contains(&"nas"),
        "{members:?}"
    );
}

#[tokio::test]
async fn a_write_in_another_project_records_no_session() {
    let (state, app) = hub("participants-other-project").await;
    projects::create(&state.db, "elsewhere", "Elsewhere")
        .await
        .expect("create project");
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    mcp.ok(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "feat-x"}),
    )
    .await;
    let wrote = mcp
        .ok(
            "signal_append",
            json!({"project_id": "elsewhere", "kind": "signal", "summary": "x", "session": named("feat-x")}),
        )
        .await;
    assert!(wrote["session"].is_null());
    assert_eq!(wrote["session_note"], "other_project");
}

#[tokio::test]
async fn a_child_needs_a_live_parent_and_a_person_name() {
    let (state, app) = hub("participants-names").await;
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;

    let orphan = mcp
        .tool(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x/review"}),
        )
        .await;
    assert_eq!(reason(&orphan), "session_parent_missing");

    mcp.ok(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "feat-x"}),
    )
    .await;
    let child = mcp
        .ok(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x/review"}),
        )
        .await;
    assert_eq!(child["session_name"], "feat-x/review");

    let opaque = mcp
        .tool(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "0f8e5c2a1b3d4e5f8a9b0c1d2e3f4a5b"}),
        )
        .await;
    assert_eq!(reason(&opaque), "session_name_opaque");
}

#[tokio::test]
async fn an_exclusive_start_refuses_a_name_the_project_holds() {
    let (state, app) = hub("participants-exclusive").await;
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let first = mcp
        .ok(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x", "exclusive": true}),
        )
        .await;
    let again = mcp
        .tool(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x", "exclusive": true}),
        )
        .await;
    assert_eq!(reason(&again), "session_name_taken");
    assert_eq!(
        again.pointer("/error/data/error/details/session_id"),
        Some(&first["session_id"])
    );
    let plain = mcp
        .ok(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x"}),
        )
        .await;
    assert_eq!(plain["session_id"], first["session_id"]);
}

#[tokio::test]
async fn two_connections_write_two_sessions() {
    let (state, app) = hub("participants-two-sessions").await;
    let first = member(&state, "laptop").await;
    let second = member(&state, "nas").await;
    let mut a = Mcp::connect(&app, Some(&first), None, "claude-code").await;
    let mut b = Mcp::connect(&app, Some(&second), None, "claude-code").await;
    for (client, name) in [(&mut a, "feat-a"), (&mut b, "feat-b")] {
        client
            .ok(
                "brain_put",
                json!({"store": "session", "path": "/kv/who", "content": name, "session": named(name)}),
            )
            .await;
    }
    for (client, name, other) in [(&mut a, "feat-a", "feat-b"), (&mut b, "feat-b", "feat-a")] {
        let own = client
            .ok(
                "brain_get",
                json!({"store": "session", "path": "/kv/who", "session": named(name)}),
            )
            .await;
        assert_eq!(own["content"], name);
        let read = client
            .ok(
                "brain_get",
                json!({"store": "session", "path": "/kv/who", "session": named(other)}),
            )
            .await;
        assert_eq!(
            read["content"], other,
            "a session is readable by name, and is its own"
        );
    }
}

#[tokio::test]
async fn a_participant_overwriting_anothers_value_needs_the_base() {
    let (state, app) = hub("participants-stale").await;
    let first = member(&state, "laptop").await;
    let second = member(&state, "nas").await;
    let mut a = Mcp::connect(&app, Some(&first), None, "claude-code").await;
    let mut b = Mcp::connect(&app, Some(&second), None, "opencode").await;
    let session = named("feat-x");

    let put = a
        .ok(
            "brain_put",
            json!({"store": "session", "path": "/kv/note", "content": "one", "session": session}),
        )
        .await;
    let base = put["version"].as_str().expect("version").to_string();
    a.ok(
        "brain_put",
        json!({"store": "session", "path": "/kv/note", "content": "two", "session": session}),
    )
    .await;

    let stale = b
        .tool(
            "brain_put",
            json!({"store": "session", "path": "/kv/note", "content": "three", "session": session}),
        )
        .await;
    assert_eq!(reason(&stale), "stale_base");
    assert!(
        stale
            .pointer("/error/data/error/details/version")
            .is_some_and(Value::is_string),
        "the refusal names the current version: {stale}"
    );
    let wrong_base = b
        .tool(
            "brain_put",
            json!({"store": "session", "path": "/kv/note", "content": "three", "if_version": base, "session": session}),
        )
        .await;
    assert_eq!(reason(&wrong_base), "stale_base");

    let current = b
        .ok(
            "brain_get",
            json!({"store": "session", "path": "/kv/note", "session": session}),
        )
        .await;
    b.ok(
        "brain_put",
        json!({"store": "session", "path": "/kv/note", "content": "three", "if_version": current["version"], "session": session}),
    )
    .await;

    let delete = a
        .tool(
            "brain_delete",
            json!({"store": "session", "path": "/kv/note", "session": session}),
        )
        .await;
    assert_eq!(reason(&delete), "stale_base");
}

#[tokio::test]
async fn ending_waits_for_a_write_in_flight_and_then_refuses_writes() {
    let (state, app) = hub("participants-gate").await;
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let started = mcp
        .ok(
            "session_start",
            json!({"project_id": PROJECT, "session_name": "feat-x"}),
        )
        .await;
    let id = started["session_id"].as_str().expect("id").to_string();

    let write = state.session_gates.shared(&id).await;
    let mut ender = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let ending = tokio::spawn({
        let id = id.clone();
        async move {
            ender
                .tool("session_end", json!({"session": {"session_id": id}}))
                .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !ending.is_finished(),
        "the end waits for the write that holds the gate"
    );
    drop(write);
    let ended = ending.await.expect("the end finishes");
    assert!(
        ended.pointer("/result/structuredContent/ok").is_some(),
        "{ended}"
    );

    let late = mcp
        .tool(
            "brain_put",
            json!({"store": "session", "path": "/kv/late", "content": "x", "session": {"session_id": id}}),
        )
        .await;
    assert_eq!(reason(&late), "session_ended");
    let publish = mcp
        .tool(
            "artifact_publish",
            json!({"project_id": PROJECT, "title": "t", "kind": "markdown", "content": "x", "session": {"session_id": id}}),
        )
        .await;
    assert_eq!(reason(&publish), "session_ended");
}

#[tokio::test]
async fn leaving_ends_nothing_and_ending_a_parent_ends_its_children() {
    let (state, app) = hub("participants-end").await;
    let token = member(&state, "laptop").await;
    let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    mcp.ok(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "feat-x"}),
    )
    .await;
    mcp.ok(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "feat-x/review"}),
    )
    .await;

    mcp.ok("session_leave", json!({"session": named("feat-x/review")}))
        .await;
    let listed = mcp.ok("session_list", json!({"project_id": PROJECT})).await;
    let status = |name: &str| {
        listed["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .find(|s| s["session_name"] == name)
            .map(|s| s["status"].clone())
    };
    assert_eq!(
        status("feat-x/review"),
        Some(json!("active")),
        "leaving ends nothing"
    );

    mcp.ok(
        "session_end",
        json!({"session": named("feat-x"), "handoff": "done"}),
    )
    .await;
    let listed = mcp.ok("session_list", json!({"project_id": PROJECT})).await;
    for row in listed["sessions"].as_array().expect("sessions") {
        assert_eq!(row["status"], "ended", "{row}");
    }
}

#[tokio::test]
async fn a_confidential_project_is_not_found_through_a_session() {
    let (state, app) = hub("participants-confidential").await;
    projects::create(&state.db, "vault", "Vault")
        .await
        .expect("create project");
    projects::set_confidential(&state.db, "vault", true)
        .await
        .expect("confidential");
    let insider = member(&state, "laptop").await;
    identity::add_grant(&state.db, "laptop", "vault")
        .await
        .expect("grant");
    let outsider = member(&state, "nas").await;
    let mut inside = Mcp::connect(&app, Some(&insider), None, "claude-code").await;
    let secret = inside
        .ok(
            "session_start",
            json!({"project_id": "vault", "session_name": "plans"}),
        )
        .await;
    let mut outside = Mcp::connect(&app, Some(&outsider), None, "claude-code").await;
    let seen = outside
        .tool(
            "brain_list",
            json!({"store": "session", "session": {"session_id": secret["session_id"]}}),
        )
        .await;
    assert_eq!(reason(&seen), "session_unknown");
    assert_eq!(
        seen.pointer("/error/data/error/code")
            .and_then(Value::as_str),
        Some("not_found")
    );
}

#[tokio::test]
async fn ending_a_session_or_its_child_needs_access_to_the_project() {
    let (state, app) = hub("participants-end-access").await;
    projects::create(&state.db, "vault", "Vault")
        .await
        .expect("create project");
    projects::set_confidential(&state.db, "vault", true)
        .await
        .expect("confidential");
    let insider = member(&state, "laptop").await;
    identity::add_grant(&state.db, "laptop", "vault")
        .await
        .expect("grant");
    let outsider = member(&state, "nas").await;
    let mut inside = Mcp::connect(&app, Some(&insider), None, "claude-code").await;
    let parent = inside
        .ok(
            "session_start",
            json!({"project_id": "vault", "session_name": "plans"}),
        )
        .await;
    let child = inside
        .ok(
            "session_start",
            json!({"project_id": "vault", "session_name": "plans/review"}),
        )
        .await;

    // Any participant may end a shared session, but only a caller that may
    // reach its project: one without the grant cannot end it or its child.
    let mut outside = Mcp::connect(&app, Some(&outsider), None, "claude-code").await;
    for target in [&parent, &child] {
        let refused = outside
            .tool(
                "session_end",
                json!({"session": {"session_id": target["session_id"]}}),
            )
            .await;
        assert_eq!(reason(&refused), "session_unknown", "{refused}");
    }
    for target in [&parent, &child] {
        let id = target["session_id"].as_str().expect("id");
        let row = agent_hub::store::sessions::get(&state.db, id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(row.status, "active", "{id} was not ended");
    }

    inside
        .ok(
            "session_end",
            json!({"session": {"session_id": parent["session_id"]}}),
        )
        .await;
    let child_id = child["session_id"].as_str().expect("id");
    let row = agent_hub::store::sessions::get(&state.db, child_id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.status, "ended", "ending the parent ends its child");
}
