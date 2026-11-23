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
        agent_hub::store::questions::answer(
            &db,
            0,
            &common::seed::writer("human"),
            question_id,
            "Yes, ship it",
            None,
        )
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
        agent_hub::store::questions::decide(
            &db,
            0,
            &common::seed::writer("human"),
            approval_id,
            true,
            None,
            None,
        )
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
    assert_eq!(
        items[0]["direct"], true,
        "an answer on its own question is direct: {next}"
    );
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

/// Ask a question in the data directory, optionally inside a session, and
/// return its id. The server is dropped afterwards so the answer can land.
fn ask(data_dir: &Path, session: Option<&str>, subject: &str) -> String {
    let mut server = spawn(data_dir);
    server.initialize();
    if let Some(name) = session {
        server.call_tool(
            "session_start",
            json!({"project_id": "proj", "session_name": name}),
        );
    }
    let asked = server.call_tool(
        "question_post",
        json!({"project_id": "proj", "subject": subject, "body": "Please."}),
    );
    structured(&asked)["question_id"]
        .as_str()
        .expect("question_post returns the question id")
        .to_string()
}

fn titles(response: &Value) -> Vec<String> {
    pending(response)
        .map(|items| {
            items
                .iter()
                .map(|item| item["title"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_answer_to_a_question_asked_in_a_session_waits_for_that_session() {
    let data_dir = TempDir::new("notify-session-bound");
    common::seed::seed_project(data_dir.path(), "proj");
    let bound = ask(data_dir.path(), Some("work"), "In the session");
    let free = ask(data_dir.path(), None, "Outside any session");
    answer_question(data_dir.path(), &bound);
    answer_question(data_dir.path(), &free);

    let mut server = spawn(data_dir.path());
    server.initialize();
    let sessionless = server.call_tool("version", json!({}));
    assert_eq!(
        titles(&sessionless),
        vec!["answered: Outside any session"],
        "a call in no session is shown only what was asked outside one: {sessionless}"
    );

    // Starting the same named session joins it, and the result of that very
    // call carries what the session was waiting for.
    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "work"}),
    );
    assert_eq!(
        titles(&started),
        vec!["answered: In the session"],
        "the session is shown its own answer: {started}"
    );
    let after = server.call_tool("version", json!({}));
    assert!(pending(&after).is_none(), "delivered once: {after}");
}

#[test]
fn a_session_is_also_shown_what_was_asked_outside_any_session() {
    let data_dir = TempDir::new("notify-session-sees-free");
    common::seed::seed_project(data_dir.path(), "proj");
    let free = ask(data_dir.path(), None, "Outside any session");
    answer_question(data_dir.path(), &free);

    let mut server = spawn(data_dir.path());
    server.initialize();
    let started = server.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "work"}),
    );
    assert_eq!(
        titles(&started),
        vec!["answered: Outside any session"],
        "{started}"
    );
}

#[test]
fn notify_wait_in_a_named_session_reads_that_scope_and_leaves_the_others() {
    let data_dir = TempDir::new("notify-wait-named");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut first = spawn(data_dir.path());
    first.initialize();
    let work = structured(&first.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "work"}),
    ))["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let other = structured(&first.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "other"}),
    ))["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    // The client follows the session it started last, so start the first again
    // and ask under it, then step aside.
    first.call_tool(
        "session_start",
        json!({"project_id": "proj", "session_name": "work"}),
    );
    let asked = first.call_tool(
        "question_post",
        json!({"project_id": "proj", "subject": "Which?", "body": "A or B."}),
    );
    let question = structured(&asked)["question_id"]
        .as_str()
        .expect("question id")
        .to_string();
    drop(first);
    answer_question(data_dir.path(), &question);

    let mut server = spawn(data_dir.path());
    server.initialize();
    let elsewhere = server.call_tool(
        "notify_wait",
        json!({"wait_seconds": 0, "session": {"session_id": other}}),
    );
    assert!(
        titles(&elsewhere).is_empty(),
        "another session's scope is not shown it: {elsewhere}"
    );
    let named = server.call_tool(
        "notify_wait",
        json!({"wait_seconds": 0, "session": {"session_id": work}}),
    );
    assert_eq!(titles(&named), vec!["answered: Which?"], "{named}");
    let again = server.call_tool(
        "notify_wait",
        json!({"wait_seconds": 0, "session": {"session_id": work}}),
    );
    assert!(titles(&again).is_empty(), "delivered once: {again}");
}
