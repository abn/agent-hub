//! Adversarial tests for session ownership atomicity, ended-session immutability,
//! guarded end transitions, and active lease validation.

use std::path::PathBuf;

use agent_hub::store::{identity, projects};
use serde_json::{Value, json};

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{mcp_post, rest};

const ADMIN_TOKEN: &str = "ownership-admin-token";
const PROJECT: &str = "proj";

#[derive(Clone)]
struct AgentClient {
    port: u16,
    token: String,
    session: String,
}

impl AgentClient {
    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = self.raw(name, arguments);
        let result = response
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} returned no structured content: {response}"));
        assert!(
            !response
                .pointer("/result/isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "{name} failed: {response}"
        );
        result
    }

    fn refusal(&self, name: &str, arguments: Value) -> (String, String) {
        let response = self.raw(name, arguments);
        let error = response
            .get("error")
            .cloned()
            .unwrap_or_else(|| panic!("{name} was expected to fail: {response}"));
        let code = error
            .pointer("/data/error/code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (code, message)
    }

    fn raw(&self, name: &str, arguments: Value) -> Value {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string();
        mcp_post(self.port, &body, Some(&self.token), Some(&self.session)).message()
    }
}

fn connect(port: u16, token: &str) -> AgentClient {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "ownership-test", "version": "0.0.0"},
        },
    })
    .to_string();
    let response = mcp_post(port, &body, Some(token), None);
    let session = response.header("mcp-session-id").expect("session id");
    mcp_post(
        port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        Some(token),
        Some(&session),
    );
    AgentClient {
        port,
        token: token.to_string(),
        session,
    }
}

struct TestFleet {
    _child: HubProcess,
    _dir: TempDir,
    port: u16,
    #[allow(dead_code)]
    data_dir: PathBuf,
    agents: Vec<AgentClient>,
}

impl TestFleet {
    async fn new(tag: &str, agents: &[&str]) -> Self {
        let dir = TempDir::new(tag);
        let db = common::store::open(&dir).await;
        projects::create(&db, PROJECT, "Ownership Project")
            .await
            .expect("create project");
        let mut tokens = Vec::new();
        for agent in agents {
            identity::create_agent(&db, agent, agent)
                .await
                .expect("create agent");
            tokens.push(
                identity::issue_token(&db, agent)
                    .await
                    .expect("issue token")
                    .token,
            );
        }
        drop(db);

        let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
        let port = child.port();
        let agent_clients = tokens.iter().map(|tok| connect(port, tok)).collect();
        let data_dir = dir.to_path_buf();
        Self {
            _child: child,
            _dir: dir,
            port,
            data_dir,
            agents: agent_clients,
        }
    }

    fn agent(&self, idx: usize) -> &AgentClient {
        &self.agents[idx]
    }
}

fn admin(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let response = rest(port, method, path, Some(ADMIN_TOKEN), body);
    (response.status, response.body().to_string())
}

#[tokio::test]
async fn ended_session_refuses_brain_writes() {
    let fleet = TestFleet::new("ended-refusal", &["agent-one"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    // Human ends the session over the admin API while agent is still connected.
    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/end"),
        Some(r#"{"handoff":"work done"}"#),
    );
    assert_eq!(status, 200, "end session via REST: {body}");

    // The agent now attempts to write to the ended session's brain.
    let (code, msg) = fleet.agent(0).refusal(
        "brain_put",
        json!({"store": "session", "path": "/kv/note", "content": "late write"}),
    );

    assert_eq!(
        code, "conflict",
        "brain_put on ended session must fail with conflict, got code={code} msg={msg}"
    );
    assert!(
        msg.contains("ended") || msg.contains("no longer available"),
        "error message should indicate ended session, got: {msg}"
    );
}

/// A write that names a session reaches only the agent that owns it.
///
/// Naming a session is how a caller with no active session of its own writes,
/// so the one-writer rule cannot rest on the active session alone: the second
/// agent names the first agent's session and is refused, and nothing it sent is
/// stored. The same call against its own session lands, which is what makes the
/// refusal about ownership and not about naming.
#[tokio::test]
async fn another_agents_session_refuses_a_named_write() {
    let fleet = TestFleet::new("named-write-owner", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    fleet.agent(0).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/plan", "content": "the plan"}),
    );

    // The second agent may read the session: project read is the read rule.
    let read = fleet.agent(1).call(
        "brain_get",
        json!({"store": "session", "path": "/kv/plan", "session": {"session_id": session_id}}),
    );
    assert_eq!(read["content"], "the plan", "the sibling reads it: {read}");

    for (tool, arguments) in [
        (
            "brain_put",
            json!({"store": "session", "path": "/kv/plan", "content": "not yours",
                   "session": {"session_id": session_id}}),
        ),
        (
            "brain_put",
            json!({"store": "session", "path": "/kv/plan", "content": "not yours",
                   "session": {"agent": "agent-one", "name": "worker", "project_id": PROJECT}}),
        ),
        (
            "brain_delete",
            json!({"store": "session", "path": "/kv/plan",
                   "session": {"session_id": session_id}}),
        ),
    ] {
        let (code, msg) = fleet.agent(1).refusal(tool, arguments);
        assert_eq!(
            code, "forbidden",
            "{tool} into another agent's session must fail with forbidden, got code={code} msg={msg}"
        );
        assert!(
            msg.contains("owner=agent-one"),
            "the refusal names the owner, got: {msg}"
        );
    }

    // Nothing landed: the owner's entry is as it was, and the sibling's own
    // session, which it names to prove naming works for it, holds its own write.
    let unchanged = fleet
        .agent(0)
        .call("brain_get", json!({"store": "session", "path": "/kv/plan"}));
    assert_eq!(
        unchanged["content"], "the plan",
        "the refused writes changed nothing: {unchanged}"
    );

    let own = fleet.agent(1).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker-two"}),
    );
    let own_id = own["session_id"].as_str().expect("session id").to_string();
    let wrote = fleet.agent(1).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/plan", "content": "mine",
               "session": {"session_id": own_id}}),
    );
    assert_eq!(wrote["ok"], true, "its own session writes: {wrote}");
}

/// A session that has ended is written through neither address.
///
/// Naming a session is the second way in, so the ended-session rule cannot hold
/// on the active path alone: an owner naming its own ended session is refused
/// the same way a connection still holding the lease is.
#[tokio::test]
async fn an_ended_session_refuses_a_named_write() {
    let fleet = TestFleet::new("named-write-ended", &["agent-one"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/end"),
        Some(r#"{"handoff":"work done"}"#),
    );
    assert_eq!(status, 200, "end session via REST: {body}");

    for arguments in [
        json!({"store": "session", "path": "/kv/note", "content": "late write",
               "session": {"session_id": session_id}}),
        json!({"store": "session", "path": "/kv/note", "content": "late write",
               "session": {"agent": "agent-one", "name": "worker", "project_id": PROJECT}}),
    ] {
        let (code, msg) = fleet.agent(0).refusal("brain_put", arguments);
        assert_eq!(
            code, "conflict",
            "a named write on an ended session must fail with conflict, got code={code} msg={msg}"
        );
        assert!(msg.contains("ended"), "the refusal says why: {msg}");
    }
}

#[tokio::test]
async fn reassigned_session_clears_stale_lineage_in_session_in() {
    let fleet = TestFleet::new("stale-lineage", &["agent-one", "agent-two"]).await;

    // Agent 0 starts a session.
    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    // Agent 0 publishes an artifact before reassignment; it should carry session_id.
    let before_reassign = fleet.agent(0).call(
        "artifact_publish",
        json!({
            "project_id": PROJECT,
            "path": "before.md",
            "title": "Before Reassign",
            "kind": "markdown",
            "content": "before reassign",
            "mime_type": "text/markdown"
        }),
    );
    let before_id = before_reassign["artifact_id"]
        .as_str()
        .expect("artifact id");
    let (_, before_body) = admin(
        fleet.port,
        "GET",
        &format!("/api/v1/artifacts/{before_id}"),
        None,
    );
    eprintln!("BEFORE REASSIGN ARTIFACT: {before_body}");

    // Human reassigns the session to Agent 1 over the admin API.
    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/reassign"),
        Some(r#"{"agent":"agent-two"}"#),
    );
    assert_eq!(status, 200, "reassign session via REST: {body}");

    // Agent 0 now publishes an artifact or writes a feed event.
    // Agent 0's active session lease must NOT be stamped onto the new work.
    let _published = fleet.agent(0).call(
        "artifact_publish",
        json!({
            "project_id": PROJECT,
            "path": "note.md",
            "title": "Note Title",
            "kind": "markdown",
            "content": "new note without session",
            "mime_type": "text/markdown"
        }),
    );

    // Check session-filtered feed for the session.
    // In unfixed code, session_in() returns the stale session_id, so the event published
    // after reassignment is wrongly stamped with session_id!
    let (_, session_feed_body) = admin(
        fleet.port,
        "GET",
        &format!("/api/v1/projects/{PROJECT}/feed?session={session_id}"),
        None,
    );
    eprintln!("SESSION FILTERED FEED: {session_feed_body}");
    let session_feed: Value = serde_json::from_str(&session_feed_body).expect("parse session feed");
    let session_events = session_feed["events"]
        .as_array()
        .expect("session events array");

    let has_after_event = session_events
        .iter()
        .any(|e| e["summary"].as_str().unwrap_or("").contains("Note Title"));
    assert!(
        !has_after_event,
        "event published by old owner after reassignment must NOT be stamped with the session, but was found in session feed: {session_feed_body}"
    );
}

#[tokio::test]
async fn reassigned_session_refuses_end_by_previous_owner() {
    let fleet = TestFleet::new("end-ownership", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    // Human reassigns the session to Agent 1.
    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/reassign"),
        Some(r#"{"agent":"agent-two"}"#),
    );
    assert_eq!(status, 200, "reassign session via REST: {body}");

    // Previous owner (Agent 0) attempts to end the reassigned session.
    let (code, msg) = fleet.agent(0).refusal(
        "session_end",
        json!({"session_id": session_id, "handoff": "attempted end"}),
    );

    assert_eq!(
        code, "forbidden",
        "ending a reassigned session by previous owner must fail with forbidden, got code={code} msg={msg}"
    );
    assert!(
        msg.contains("belongs to another agent"),
        "error message should indicate ownership mismatch, got: {msg}"
    );
}
