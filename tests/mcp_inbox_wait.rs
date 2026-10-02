//! End-to-end tests for `inbox_wait` and the cursor-safe `inbox_read`.
//!
//! A stdio server answers one request at a time, so a wait that blocks cannot be
//! answered from the same process. The hub runs as its own process and is driven
//! over two HTTP MCP connections: one blocks in `inbox_wait`, the other posts the
//! answer. The wait must wake on the answer rather than sit out its deadline, and
//! the cursor it returns must not report the same answer twice.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::mcp_post;

const ADMIN_TOKEN: &str = "inbox-wait-admin";
const AGENT: &str = "waiter";
const OTHER_AGENT: &str = "bystander";
const PROJECT: &str = "proj";

/// One authenticated MCP connection.
#[derive(Clone)]
struct Connection {
    port: u16,
    token: String,
    session: String,
}

impl Connection {
    /// Call a tool over HTTP and return its structured result.
    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = mcp_post(
            self.port,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            })
            .to_string(),
            Some(&self.token),
            Some(&self.session),
        );
        let message = response.message();
        assert!(message.get("error").is_none(), "{name} failed: {message}");
        message
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} returned no structured content: {message}"))
    }
}

/// Complete the MCP handshake and return one connection.
fn connect(port: u16, token: &str) -> Connection {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp-inbox-wait", "version": "0.0.0"},
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
    Connection {
        port,
        token: token.to_string(),
        session,
    }
}

/// A hub holding one project and two connected sessions.
///
/// The sessions are separate on purpose: a streamable HTTP session answers one
/// request at a time, so a blocking `inbox_wait` on one session would queue the
/// answer behind it on the same session. Two sessions let the answer land while
/// the wait is still open.
struct Fleet {
    // Declared before the directory: the hub is reaped before its directory goes.
    _child: HubProcess,
    _dir: TempDir,
    agent: Connection,
    waiter: Connection,
    other: Connection,
}

impl Fleet {
    async fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let db = common::store::open(&dir).await;
        agent_hub::store::projects::create(&db, PROJECT, "Project")
            .await
            .expect("create project");
        let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
        let other_token = common::seed::agent_token(&db, OTHER_AGENT, "Bystander").await;
        drop(db);

        let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
        let port = child.port();
        let agent = connect(port, &token);
        let waiter = connect(port, &token);
        let other = connect(port, &other_token);
        Self {
            _child: child,
            _dir: dir,
            agent,
            waiter,
            other,
        }
    }

    /// Post a question and return its id.
    fn ask(&self, subject: &str) -> String {
        let asked = self.agent.call(
            "question_post",
            json!({"project_id": PROJECT, "subject": subject, "body": "ready?"}),
        );
        asked["question_id"]
            .as_str()
            .expect("question_post returns a question id")
            .to_string()
    }
}

#[tokio::test]
async fn a_wait_wakes_on_the_answer_instead_of_timing_out() {
    let fleet = Fleet::new("wait-answer").await;
    let question_id = fleet.ask("Deploy tonight?");

    // The wait blocks on its own session while the answer lands on this one.
    let waiter = fleet.waiter.clone();
    let waited_for = question_id.clone();
    let handle = std::thread::spawn(move || {
        let started = Instant::now();
        let result = waiter.call(
            "inbox_wait",
            json!({"project_id": PROJECT, "wait_seconds": 20}),
        );
        (started.elapsed(), result)
    });

    // Give the wait time to subscribe and make its first query, then answer.
    std::thread::sleep(Duration::from_millis(300));
    fleet.agent.call(
        "answer_post",
        json!({"question_id": question_id, "body": "Yes, ship it"}),
    );

    let (elapsed, result) = handle.join().expect("the waiter thread");
    let items = result["items"].as_array().expect("wait returns items");
    assert_eq!(
        items.len(),
        1,
        "the answered question is returned, so the wait woke rather than elapsing: {result}"
    );
    assert_eq!(items[0]["event_id"], waited_for.as_str());
    assert_eq!(items[0]["status"], "resolved");
    assert_eq!(items[0]["answer"]["body"], "Yes, ship it");
    assert!(
        result["next_since"].as_str().is_some(),
        "a cursor comes back for the next call: {result}"
    );
    // A timeout would have returned no items. The bound is loose because the
    // HTTP client here reads to the end of the connection: what it proves is
    // that the answer, not the 20 second deadline, ended the wait.
    assert!(
        elapsed < Duration::from_secs(20),
        "the wait did not run its full deadline: {elapsed:?}"
    );
}

#[tokio::test]
async fn a_wait_does_not_report_the_same_answer_twice() {
    let fleet = Fleet::new("wait-cursor").await;
    let question_id = fleet.ask("Deploy tonight?");
    fleet.agent.call(
        "answer_post",
        json!({"question_id": question_id, "body": "Ship it"}),
    );

    // The answer is already on record, so the first wait returns it at once.
    let first = fleet.agent.call(
        "inbox_wait",
        json!({"project_id": PROJECT, "wait_seconds": 5}),
    );
    assert_eq!(
        first["items"].as_array().expect("items").len(),
        1,
        "an answer that already landed is returned, not waited on: {first}"
    );
    let cursor = first["next_since"].as_str().expect("a cursor").to_string();

    let second = fleet.agent.call(
        "inbox_wait",
        json!({"project_id": PROJECT, "since": cursor, "wait_seconds": 0}),
    );
    assert_eq!(
        second["items"].as_array().expect("items").len(),
        0,
        "the cursor keeps the same answer from being reported twice: {second}"
    );
}

#[tokio::test]
async fn a_wait_that_hears_nothing_returns_at_the_deadline() {
    let fleet = Fleet::new("wait-timeout").await;

    let started = Instant::now();
    let result = fleet.agent.call(
        "inbox_wait",
        json!({"project_id": PROJECT, "wait_seconds": 1}),
    );
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(800),
        "the wait held for its deadline: {elapsed:?}"
    );
    assert_eq!(
        result["items"].as_array().expect("items").len(),
        0,
        "nothing landed: {result}"
    );
    assert!(
        result["next_since"].is_null(),
        "with nothing seen there is no cursor yet: {result}"
    );
}

#[tokio::test]
async fn a_wait_is_scoped_to_the_callers_own_items() {
    let fleet = Fleet::new("wait-scope").await;

    // Another agent posts and gets answered. It is in a project the caller may
    // read, but it is not the caller's own item and must not wake its wait.
    let asked = fleet.other.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Mine, not yours"}),
    );
    let other_question = asked["question_id"]
        .as_str()
        .expect("question id")
        .to_string();
    fleet.other.call(
        "answer_post",
        json!({"question_id": other_question, "body": "answered for the other agent"}),
    );

    let result = fleet.agent.call(
        "inbox_wait",
        json!({"project_id": PROJECT, "wait_seconds": 1}),
    );
    assert_eq!(
        result["items"].as_array().expect("items").len(),
        0,
        "another actor's resolved item is not the caller's answer: {result}"
    );
}

#[tokio::test]
async fn inbox_read_polls_from_its_cursor() {
    let fleet = Fleet::new("read-cursor").await;
    let question_id = fleet.ask("Deploy tonight?");

    let before = fleet
        .agent
        .call("inbox_read", json!({"project_id": PROJECT}));
    let items = before["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "the open question is listed: {before}");
    let cursor = before["next_since"].as_str().expect("a cursor").to_string();
    assert_eq!(
        cursor, question_id,
        "an open entry's cursor is its own event id: {before}"
    );

    fleet.agent.call(
        "answer_post",
        json!({"question_id": question_id, "body": "sure"}),
    );

    let after = fleet.agent.call(
        "inbox_read",
        json!({"project_id": PROJECT, "since": cursor}),
    );
    let items = after["items"].as_array().expect("items");
    assert_eq!(
        items.len(),
        1,
        "the answer sits above the cursor even though the question's id does not: {after}"
    );
    assert_eq!(items[0]["status"], "resolved");
    assert_eq!(items[0]["answer"]["body"], "sure");
    let next = after["next_since"]
        .as_str()
        .expect("an advanced cursor")
        .to_string();
    assert_ne!(next, cursor, "the cursor advanced past the answer");

    let quiet = fleet
        .agent
        .call("inbox_read", json!({"project_id": PROJECT, "since": next}));
    assert_eq!(
        quiet["items"].as_array().expect("items").len(),
        0,
        "a poll above the cursor returns nothing: {quiet}"
    );
}
