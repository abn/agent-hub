//! Delivery per participant: each (member, agent, session) has its own
//! cursor, the advance is a compare-and-set, and an item asked in a session
//! reaches every participant of it once.

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

async fn hub(tag: &str) -> (common::state::TestState, Router, Mcp) {
    let state = common::state::open(tag).await;
    projects::create(&state.db, PROJECT, "Homelab")
        .await
        .expect("create project");
    let app = http_router((*state).clone());
    let human = Mcp::connect(&app, Some(common::state::ADMIN_TOKEN), None, "human").await;
    (state, app, human)
}

fn session() -> Value {
    json!({"project_id": PROJECT, "name": "feat-x"})
}

/// The ids of the items a result delivered, from its `notifications` member.
fn delivered(result: &Value) -> Vec<String> {
    result["notifications"]["pending"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn wait_once(mcp: &mut Mcp, session: Value) -> Vec<String> {
    delivered(
        &mcp.ok(
            "notify_wait",
            json!({"wait_seconds": 0, "session": session}),
        )
        .await,
    )
}

#[tokio::test]
async fn an_answer_reaches_a_participant_that_joined_before_its_first_drain() {
    let (state, app, mut human) = hub("delivery-before-first-drain").await;
    let asker = member(&state, "laptop").await;
    let other = member(&state, "nas").await;
    let mut a = Mcp::connect(&app, Some(&asker), None, "claude-code").await;
    let mut b = Mcp::connect(&app, Some(&other), None, "opencode").await;

    let asked = a
        .ok(
            "question_post",
            json!({"project_id": PROJECT, "subject": "Roll the logs?", "session": session()}),
        )
        .await;
    let question = asked["question_id"].as_str().expect("question").to_string();
    // B joins the session now, before the answer exists and before it has
    // drained anything.
    b.ok(
        "feed_read",
        json!({"project_id": PROJECT, "session": session()}),
    )
    .await;
    human
        .ok(
            "answer_post",
            json!({"question_id": question, "body": "yes"}),
        )
        .await;

    let for_b = wait_once(&mut b, session()).await;
    assert_eq!(
        for_b.len(),
        1,
        "the newcomer is told of the answer: {for_b:?}"
    );
    assert_eq!(
        wait_once(&mut b, session()).await,
        Vec::<String>::new(),
        "once"
    );
    assert_eq!(
        wait_once(&mut a, session()).await.len(),
        1,
        "the asker is told too"
    );
    assert!(wait_once(&mut a, session()).await.is_empty());
}

#[tokio::test]
async fn a_member_that_switches_harness_is_told_once_under_each() {
    let (state, app, mut human) = hub("delivery-harness-switch").await;
    let token = member(&state, "laptop").await;
    let mut claude = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let mut opencode = Mcp::connect(&app, Some(&token), None, "opencode").await;

    let asked = claude
        .ok(
            "question_post",
            json!({"project_id": PROJECT, "subject": "Which branch?", "session": session()}),
        )
        .await;
    opencode
        .ok(
            "feed_read",
            json!({"project_id": PROJECT, "session": session()}),
        )
        .await;
    human
        .ok(
            "answer_post",
            json!({"question_id": asked["question_id"], "body": "main"}),
        )
        .await;

    assert_eq!(wait_once(&mut claude, session()).await.len(), 1);
    assert_eq!(wait_once(&mut opencode, session()).await.len(), 1);
    assert!(wait_once(&mut claude, session()).await.is_empty());
    assert!(wait_once(&mut opencode, session()).await.is_empty());
}

#[tokio::test]
async fn concurrent_drains_of_one_participant_deliver_an_item_once() {
    let (state, app, mut human) = hub("delivery-concurrent").await;
    let token = member(&state, "laptop").await;
    let mut asker = Mcp::connect(&app, Some(&token), None, "claude-code").await;
    let asked = asker
        .ok(
            "question_post",
            json!({"project_id": PROJECT, "subject": "Go?", "session": session()}),
        )
        .await;
    human
        .ok(
            "answer_post",
            json!({"question_id": asked["question_id"], "body": "go"}),
        )
        .await;

    let mut drains = Vec::new();
    for _ in 0..8 {
        let mut mcp = Mcp::connect(&app, Some(&token), None, "claude-code").await;
        drains.push(tokio::spawn(
            async move { wait_once(&mut mcp, session()).await },
        ));
    }
    let mut total = 0;
    for drain in drains {
        total += drain.await.expect("drain").len();
    }
    assert_eq!(total, 1, "eight drains of one cursor deliver the item once");
}

#[tokio::test]
async fn a_comment_on_a_session_artifact_arrives_without_a_subscription() {
    let (state, app, _human) = hub("delivery-comment").await;
    let author = member(&state, "laptop").await;
    let reader = member(&state, "nas").await;
    let mut a = Mcp::connect(&app, Some(&author), None, "claude-code").await;
    let mut b = Mcp::connect(&app, Some(&reader), None, "opencode").await;

    let published = a
        .ok(
            "artifact_publish",
            json!({"project_id": PROJECT, "title": "Plan", "kind": "markdown", "content": "# Plan", "session": session()}),
        )
        .await;
    b.ok(
        "comment_post",
        json!({
            "artifact_id": published["artifact_id"],
            "body": "Looks right",
            "session": {"project_id": PROJECT, "name": "review"},
        }),
    )
    .await;

    let told = a
        .ok(
            "notify_wait",
            json!({"wait_seconds": 0, "session": session()}),
        )
        .await;
    let items = told["notifications"]["pending"]
        .as_array()
        .expect("pending");
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "comment" && item["direct"] == true),
        "the comment names the author's session: {told}"
    );
    assert!(
        wait_once(&mut a, session()).await.is_empty(),
        "and only once"
    );
}
