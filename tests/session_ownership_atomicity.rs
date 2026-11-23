//! Adversarial tests for shared-session writes: ended-session immutability,
//! the base a second participant writes against, and ending by any participant.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_hub::store::{identity, projects};
use serde_json::{Value, json};

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{mcp_post, rest};

const ADMIN_TOKEN: &str = "ownership-admin-token";
const PROJECT: &str = "proj";

/// A member's connection. It follows the session of its last successful
/// `session_start`, adding it to a call that names none.
#[derive(Clone)]
struct AgentClient {
    port: u16,
    token: String,
    session: String,
    following: Arc<Mutex<Option<Value>>>,
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

    /// Call a tool that sends no session, whatever the client follows.
    fn call_sessionless(&self, name: &str, arguments: Value) -> Value {
        let response = self.raw_exact(name, arguments);
        response
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} returned no structured content: {response}"))
    }

    /// The error code and `details` of a refusal.
    fn refusal_details(&self, name: &str, arguments: Value) -> (String, Value) {
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
        let details = error
            .pointer("/data/error/details")
            .cloned()
            .unwrap_or(Value::Null);
        (code, details)
    }

    fn raw(&self, name: &str, arguments: Value) -> Value {
        let mut arguments = arguments;
        let following = self.following.lock().expect("following").clone();
        if let (Some(following), Some(object)) = (following, arguments.as_object_mut()) {
            object.entry("session").or_insert(following);
        }
        let message = self.raw_exact(name, arguments.clone());
        match name {
            "session_start" => {
                if let Some(id) = message
                    .pointer("/result/structuredContent/session_id")
                    .and_then(Value::as_str)
                {
                    *self.following.lock().expect("following") = Some(json!({"session_id": id}));
                }
            }
            "session_end" if message.get("error").is_none() => {
                let ended = arguments
                    .pointer("/session/session_id")
                    .or_else(|| arguments.get("session_id"));
                let mut following = self.following.lock().expect("following");
                if ended == following.as_ref().and_then(|f| f.get("session_id")) {
                    *following = None;
                }
            }
            _ => {}
        }
        message
    }

    fn raw_exact(&self, name: &str, arguments: Value) -> Value {
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
            "clientInfo": {"name": "atomicity-test", "version": "0.0.0"},
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
        following: Arc::new(Mutex::new(None)),
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

    // Human ends the session over the admin API while the agent still names it.
    let (status, body) = admin(
        fleet.port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/end"),
        Some(r#"{"handoff":"work done"}"#),
    );
    assert_eq!(status, 200, "end session via REST: {body}");

    let (code, details) = fleet.agent(0).refusal_details(
        "brain_put",
        json!({"store": "session", "path": "/kv/note", "content": "late write"}),
    );

    assert_eq!(
        code, "conflict",
        "brain_put on an ended session must fail with conflict, got details={details}"
    );
    assert_eq!(details["reason"], "session_ended", "{details}");
}

/// A second participant writes the shared session against the version it read.
///
/// Naming a session joins it, so a second member's write lands in the one
/// brain. What guards the first member's value is the base: a key another
/// participant wrote last answers `stale_base` with the current version, by
/// either address and for a delete as for a put, and nothing it sent is stored.
/// With the version it read, the same write lands.
#[tokio::test]
async fn a_second_participants_write_needs_the_version_it_read() {
    let fleet = TestFleet::new("named-write-base", &["agent-one", "agent-two"]).await;

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

    let read = fleet.agent(1).call(
        "brain_get",
        json!({"store": "session", "path": "/kv/plan", "session": {"session_id": session_id}}),
    );
    assert_eq!(read["content"], "the plan", "the sibling reads it: {read}");

    for (tool, arguments) in [
        (
            "brain_put",
            json!({"store": "session", "path": "/kv/plan", "content": "not yet",
                   "session": {"session_id": session_id}}),
        ),
        (
            "brain_put",
            json!({"store": "session", "path": "/kv/plan", "content": "not yet",
                   "session": {"name": "worker", "project_id": PROJECT}}),
        ),
        (
            "brain_delete",
            json!({"store": "session", "path": "/kv/plan",
                   "session": {"session_id": session_id}}),
        ),
    ] {
        let (code, details) = fleet.agent(1).refusal_details(tool, arguments);
        assert_eq!(code, "conflict", "{tool} without a base: {details}");
        assert_eq!(details["reason"], "stale_base", "{tool}: {details}");
        assert_eq!(details["version"], read["version"], "{tool}: {details}");
    }

    let unchanged = fleet
        .agent(0)
        .call("brain_get", json!({"store": "session", "path": "/kv/plan"}));
    assert_eq!(
        unchanged["content"], "the plan",
        "the refused writes changed nothing: {unchanged}"
    );

    let wrote = fleet.agent(1).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/plan", "content": "agreed",
               "if_version": read["version"], "session": {"session_id": session_id}}),
    );
    assert_eq!(
        wrote["ok"], true,
        "a write on the version it read lands: {wrote}"
    );
    assert_eq!(wrote["session"], session_id.as_str());
    let changed = fleet
        .agent(0)
        .call("brain_get", json!({"store": "session", "path": "/kv/plan"}));
    assert_eq!(
        changed["content"], "agreed",
        "the first member sees it: {changed}"
    );

    // A key that does not exist yet needs no base from anyone.
    let fresh = fleet.agent(1).call(
        "brain_put",
        json!({"store": "session", "path": "/kv/other", "content": "new",
               "session": {"session_id": session_id}}),
    );
    assert_eq!(fresh["ok"], true, "a new key lands: {fresh}");
}

/// A session that has ended is written through neither address.
///
/// Naming a session is the second way in, so the ended-session rule cannot hold
/// on the followed session alone: a participant naming its ended session by id
/// or by name is refused the same way.
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
               "session": {"name": "worker", "project_id": PROJECT}}),
    ] {
        let (code, details) = fleet.agent(0).refusal_details("brain_put", arguments);
        assert_eq!(
            code, "conflict",
            "a named write on an ended session must fail with conflict, got {details}"
        );
        assert_eq!(details["reason"], "session_ended", "{details}");
    }
}

#[tokio::test]
async fn an_artifact_published_outside_a_session_is_not_stamped_with_it() {
    let fleet = TestFleet::new("lineage-outside", &["agent-one"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    // Published while following the session, the artifact records it.
    let inside = fleet.agent(0).call(
        "artifact_publish",
        json!({
            "project_id": PROJECT,
            "path": "inside.md",
            "title": "Inside Title",
            "kind": "markdown",
            "content": "inside the session",
            "mime_type": "text/markdown"
        }),
    );
    assert_eq!(inside["session"], session_id.as_str(), "{inside}");

    // The same member publishes once more with no session on the call.
    let outside = fleet.agent(0).call_sessionless(
        "artifact_publish",
        json!({
            "project_id": PROJECT,
            "path": "outside.md",
            "title": "Outside Title",
            "kind": "markdown",
            "content": "outside any session",
            "mime_type": "text/markdown"
        }),
    );
    assert!(
        outside["session"].is_null(),
        "a call that names no session records none: {outside}"
    );

    let feed = fleet.agent(0).call(
        "feed_read",
        json!({"project_id": PROJECT, "session_id": session_id}),
    );
    let summaries: Vec<&str> = feed["events"]
        .as_array()
        .expect("session events array")
        .iter()
        .filter_map(|event| event["summary"].as_str())
        .collect();
    assert!(
        summaries
            .iter()
            .any(|summary| summary.contains("Inside Title")),
        "the session feed holds the artifact published in it: {summaries:?}"
    );
    assert!(
        !summaries
            .iter()
            .any(|summary| summary.contains("Outside Title")),
        "the artifact published outside it is not in the session feed: {summaries:?}"
    );
}

#[tokio::test]
async fn a_second_participant_may_end_the_session_the_first_started() {
    let fleet = TestFleet::new("end-any", &["agent-one", "agent-two"]).await;

    let started = fleet.agent(0).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );
    let session_id = started["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    fleet.agent(1).call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "worker"}),
    );

    let ended = fleet.agent(1).call(
        "session_end",
        json!({"session_id": session_id, "handoff": "second participant ends it"}),
    );
    assert_eq!(ended["ok"], true, "{ended}");
    assert!(
        ended["ended"]
            .as_array()
            .expect("ended ids")
            .iter()
            .any(|id| id == session_id.as_str()),
        "the result lists the session: {ended}"
    );

    let (code, details) = fleet.agent(0).refusal_details(
        "brain_put",
        json!({"store": "session", "path": "/kv/late", "content": "x"}),
    );
    assert_eq!(code, "conflict", "the first participant is told it ended");
    assert_eq!(details["reason"], "session_ended", "{details}");
}
