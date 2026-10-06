//! End-to-end tests for the notification trailer on MCP tool results.
//!
//! Spawns the built binary as `agent-hub mcp`, posts a question and an
//! approval, resolves them while the agent is away, and proves the agent's
//! next tool call carries each resolution once, then carries nothing.

use std::path::Path;

use serde_json::{Value, json};

mod common;

use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

const AGENT_ID: &str = "mcp-notify";

/// The stdio server as the agent these tests act as.
fn spawn(data_dir: &Path) -> McpServer {
    McpServer::mcp(data_dir, &[("HUB_AGENT_ID", AGENT_ID)])
}

/// The pending list on a tool result, when the trailer is present.
fn pending(response: &Value) -> Option<&Vec<Value>> {
    structured(response)
        .get("notifications")?
        .get("pending")?
        .as_array()
}

/// The text view of a tool result, parsed as JSON.
fn text_view(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result carries a text view: {response}"));
    serde_json::from_str(text)
        .unwrap_or_else(|err| panic!("the text view parses as JSON: {err}: {response}"))
}

/// Answer a question directly in the store, while no hub holds the engine.
fn answer_question(data_dir: &Path, question_id: &str) {
    common::seed::block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::questions::answer(&db, 0, "human", question_id, "Yes, ship it", None)
            .await
            .expect("answer");
    });
}

/// Decide an approval directly in the store, while no hub holds the engine.
fn decide_approval(data_dir: &Path, approval_id: &str) {
    common::seed::block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::questions::decide(&db, 0, "human", approval_id, true, None, None)
            .await
            .expect("decide");
    });
}

#[test]
fn an_answer_to_the_agent_question_is_delivered_once() {
    let data_dir = TempDir::new("notify-once");
    common::seed::seed_project(data_dir.path(), "proj");

    let mut server = spawn(data_dir.path());
    server.initialize();
    let asked = server.call_tool(
        "question_post",
        json!({
            "project_id": "proj",
            "subject": "Deploy tonight?",
            "body": "The release is ready.",
        }),
    );
    let question_id = structured(&asked)["question_id"]
        .as_str()
        .expect("question_post returns the question id")
        .to_string();
    assert!(
        pending(&asked).is_none(),
        "an open question is not attention yet: {asked}"
    );
    // Only one process may hold the engine, so the agent steps aside while
    // the answer lands.
    drop(server);
    answer_question(data_dir.path(), &question_id);

    let mut server = spawn(data_dir.path());
    server.initialize();
    let next = server.call_tool("version", json!({}));
    let items =
        pending(&next).unwrap_or_else(|| panic!("the next call carries the answer: {next}"));
    assert_eq!(items.len(), 1, "exactly one item is pending: {next}");
    assert_eq!(items[0]["source"], "attention");
    assert_eq!(items[0]["kind"], "question_answered");
    assert_eq!(items[0]["id"], question_id.as_str());
    assert_eq!(items[0]["project_id"], "proj");
    assert_eq!(items[0]["title"], "answered: Deploy tonight?");
    assert!(
        items[0]["at"].as_str().is_some_and(|at| !at.is_empty()),
        "the item says when it was resolved: {next}"
    );
    assert_eq!(
        text_view(&next)["notifications"]["pending"],
        Value::Array(items.clone()),
        "the text view carries the same trailer"
    );

    let after = server.call_tool("version", json!({}));
    assert!(
        pending(&after).is_none(),
        "the item is delivered once: {after}"
    );
}

#[test]
fn a_decision_on_the_agent_approval_is_delivered_once() {
    let data_dir = TempDir::new("notify-approval");
    common::seed::seed_project(data_dir.path(), "proj");

    let mut server = spawn(data_dir.path());
    server.initialize();
    let asked = server.call_tool(
        "signal_append",
        json!({"project_id": "proj", "kind": "approval", "summary": "Restart the node?"}),
    );
    let approval_id = structured(&asked)["event_id"]
        .as_str()
        .expect("signal_append returns an event id")
        .to_string();
    drop(server);
    decide_approval(data_dir.path(), &approval_id);

    let mut server = spawn(data_dir.path());
    server.initialize();
    let next = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let items =
        pending(&next).unwrap_or_else(|| panic!("the next call carries the decision: {next}"));
    assert_eq!(items.len(), 1, "exactly one item is pending: {next}");
    assert_eq!(items[0]["source"], "attention");
    assert_eq!(items[0]["kind"], "approval_decided");
    assert_eq!(items[0]["id"], approval_id.as_str());
    assert_eq!(items[0]["project_id"], "proj");
    assert!(
        items[0]["title"]
            .as_str()
            .is_some_and(|title| title.contains("Restart the node?")),
        "the title names the approval: {next}"
    );

    let after = server.call_tool("feed_read", json!({"project_id": "proj"}));
    assert!(
        pending(&after).is_none(),
        "the item is delivered once: {after}"
    );
}

#[test]
fn a_fresh_agent_gets_no_trailer() {
    let data_dir = TempDir::new("notify-empty");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path());
    server.initialize();

    let answered = server.call_tool("version", json!({}));
    assert!(
        pending(&answered).is_none(),
        "nothing pending means no member: {answered}"
    );
    assert!(
        structured(&answered).get("notifications").is_none(),
        "the member is absent rather than empty: {answered}"
    );
}

#[test]
fn an_error_result_carries_no_trailer() {
    let data_dir = TempDir::new("notify-error");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path());
    server.initialize();

    let unknown = server.call_tool("no_such_tool", json!({}));
    assert!(
        structured(&unknown).get("notifications").is_none(),
        "a tool-result error is undecorated: {unknown}"
    );

    let invalid = server.call_tool("inbox_read", json!({"status": "nonsense"}));
    assert!(
        invalid.get("error").is_some(),
        "an invalid status is a protocol error: {invalid}"
    );
    assert!(
        invalid
            .pointer("/result/structuredContent/notifications")
            .is_none()
            && invalid.pointer("/result/notifications").is_none(),
        "a protocol error carries no trailer: {invalid}"
    );
}
