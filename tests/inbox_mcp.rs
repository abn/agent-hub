//! End-to-end tests for the MCP inbox and question tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, appends a finished event,
//! reads the inbox, posts a question, answers it, and reads the inbox again
//! to prove unread, action, and resolved states. Any non-JSON stdout line
//! fails the test because it would corrupt the protocol.

use std::path::Path;

use serde_json::{Value, json};

mod common;

use common::stdio::{StdioClient as McpServer, structured};
use common::temp::TempDir;

/// The stdio server as the agent these tests act as.
fn spawn(data_dir: &Path, envs: &[(&str, &str)]) -> McpServer {
    let mut all = vec![("HUB_AGENT_ID", AGENT_ID)];
    all.extend_from_slice(envs);
    McpServer::mcp(data_dir, &all)
}

const AGENT_ID: &str = "mcp-inbox";

fn error_code(response: &Value) -> &str {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool error carries the hub code: {response}"))
}

fn items(response: &Value) -> &Vec<Value> {
    structured(response)["items"]
        .as_array()
        .unwrap_or_else(|| panic!("inbox_read returns items: {response}"))
}

fn item_with_id<'a>(items: &'a [Value], event_id: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["event_id"] == event_id)
        .unwrap_or_else(|| panic!("inbox carries {event_id}, got {items:?}"))
}

#[test]
fn inbox_and_question_tools_round_trip_over_stdio() {
    let data_dir = TempDir::new("roundtrip");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path(), &[]);
    server.initialize();

    let finished = server.call_tool(
        "signal_append",
        json!({"project_id": "proj", "kind": "finished", "summary": "nightly report done"}),
    );
    let finished_id = structured(&finished)["event_id"]
        .as_str()
        .expect("signal_append returns an event id")
        .to_string();

    let all = server.call_tool("inbox_read", json!({}));
    let all_items = items(&all);
    let finished_item = item_with_id(all_items, &finished_id);
    assert_eq!(
        finished_item["status"], "unread",
        "finished work lands as unread"
    );
    assert_eq!(finished_item["kind"], "finished");
    assert_eq!(
        finished_item["project_display_name"], "Project",
        "an item says where it is from by name"
    );
    assert_eq!(
        finished_item["actor"], AGENT_ID,
        "the actor is the resolved principal"
    );

    let unread = server.call_tool("inbox_read", json!({"status": "unread"}));
    assert_eq!(items(&unread).len(), 1, "one finished event is unread");

    let asked = server.call_tool(
        "question_post",
        json!({
            "project_id": "proj",
            "subject": "Deploy tonight?",
            "body": "The release is ready.",
        }),
    );
    let asked_result = structured(&asked);
    let question_id = asked_result["question_id"]
        .as_str()
        .expect("question_post returns the question id")
        .to_string();
    assert_eq!(
        asked_result["event_id"].as_str().expect("event id"),
        question_id,
        "the question id is the question event's id"
    );
    assert_eq!(
        asked_result["thread_id"].as_str().expect("thread id"),
        question_id,
        "the question roots its own thread"
    );

    let action = server.call_tool("inbox_read", json!({"status": "action"}));
    let action_items = items(&action);
    assert_eq!(
        action_items.len(),
        1,
        "the question is the only action item"
    );
    let question_item = item_with_id(action_items, &question_id);
    assert_eq!(question_item["status"], "action");
    assert_eq!(question_item["kind"], "question");

    let scoped = server.call_tool("inbox_read", json!({"project_id": "proj"}));
    assert_eq!(
        items(&scoped).len(),
        2,
        "the project inbox carries both entries"
    );

    let answered = server.call_tool(
        "answer_post",
        json!({"question_id": question_id, "body": "Yes, ship it"}),
    );
    let answer_id = structured(&answered)["event_id"]
        .as_str()
        .expect("answer_post returns an event id")
        .to_string();

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events");
    let answer = events
        .iter()
        .find(|event| event["id"] == answer_id)
        .unwrap_or_else(|| panic!("the feed carries the answer, got {events:?}"));
    assert_eq!(answer["kind"], "answer");
    assert_eq!(
        answer["thread_id"].as_str().expect("thread id"),
        question_id,
        "the answer lands on the question's thread"
    );

    let resolved = server.call_tool("inbox_read", json!({"status": "resolved"}));
    let resolved_items = items(&resolved);
    assert_eq!(
        resolved_items.len(),
        1,
        "the answer resolves exactly one item"
    );
    let resolved_item = item_with_id(resolved_items, &question_id);
    assert_eq!(resolved_item["status"], "resolved");
    assert_eq!(resolved_item["answer"]["body"], "Yes, ship it");
    assert_eq!(resolved_item["answer"]["actor"], "mcp-inbox");
    assert_eq!(resolved_item["answer"]["event_id"], answer_id);

    let action = server.call_tool("inbox_read", json!({"status": "action"}));
    assert_eq!(
        items(&action).len(),
        0,
        "the answered question leaves the action queue"
    );

    let invalid = server.call_tool("inbox_read", json!({"status": "nonsense"}));
    assert_eq!(
        error_code(&invalid),
        "invalid_argument",
        "an unknown status is rejected"
    );
}

#[test]
fn question_post_is_refused_at_the_inbox_cap() {
    let data_dir = TempDir::new("cap");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path(), &[("HUB_INBOX_ACTION_PER_AGENT", "2")]);
    server.initialize();

    for subject in ["one", "two"] {
        let asked = server.call_tool(
            "question_post",
            json!({"project_id": "proj", "subject": subject}),
        );
        structured(&asked)["question_id"]
            .as_str()
            .unwrap_or_else(|| panic!("under the cap the question is admitted: {asked}"));
    }

    let refused = server.call_tool(
        "question_post",
        json!({"project_id": "proj", "subject": "three"}),
    );
    assert_eq!(
        error_code(&refused),
        "rate_limited",
        "the cap refuses the third open item"
    );
    assert_eq!(
        refused["error"]["data"]["error"]["retryable"], false,
        "a capped write is not retried blindly; the human frees the slot"
    );
}

#[test]
fn approval_signal_is_refused_at_the_inbox_cap() {
    let data_dir = TempDir::new("approval-cap");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path(), &[("HUB_INBOX_ACTION_PER_AGENT", "1")]);
    server.initialize();

    let first = server.call_tool(
        "signal_append",
        json!({"project_id": "proj", "kind": "approval", "summary": "deploy?"}),
    );
    structured(&first)["event_id"]
        .as_str()
        .unwrap_or_else(|| panic!("under the cap the approval is admitted: {first}"));

    let refused = server.call_tool(
        "signal_append",
        json!({"project_id": "proj", "kind": "approval", "summary": "again"}),
    );
    assert_eq!(
        error_code(&refused),
        "rate_limited",
        "the cap refuses a second open approval"
    );
}

/// Seed one finished event and let the human read it, before the hub starts.
///
/// Only one process may hold the engine, so the human's side of this happens
/// in the test rather than over the REST surface of a running hub.
fn seed_read_report(data_dir: &Path, summary: &str) -> String {
    common::seed::block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        let event_id = agent_hub::store::events::append(
            &db,
            0,
            AGENT_ID,
            None,
            agent_hub::store::events::NewEvent {
                project_id: "proj".to_string(),
                kind: "finished".to_string(),
                summary: summary.to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append finished");
        agent_hub::store::inbox::mark_read(&db, &event_id)
            .await
            .expect("the human reads it");
        event_id
    })
}

#[test]
fn an_agent_is_not_told_what_the_human_has_read() {
    let data_dir = TempDir::new("read-state");
    common::seed::seed_project(data_dir.path(), "proj");
    let event_id = seed_read_report(data_dir.path(), "nightly report done");
    let mut server = spawn(data_dir.path(), &[]);
    server.initialize();

    let all = server.call_tool("inbox_read", json!({}));
    let item = item_with_id(items(&all), &event_id);
    assert_eq!(
        item["status"], "unread",
        "the human opening a report is not the agent's business"
    );
    assert_eq!(
        item["updated_at"], item["created_at"],
        "and neither is the moment they opened it"
    );

    let unread = server.call_tool("inbox_read", json!({"status": "unread"}));
    assert_eq!(
        items(&unread).len(),
        1,
        "a read report does not disappear from the agent's unread filter"
    );

    let refused = server.call_tool("inbox_read", json!({"status": "read"}));
    assert_eq!(
        error_code(&refused),
        "invalid_argument",
        "there is no read status for an agent to ask about"
    );
}

/// The human declines an approval with a note, as the decision route does.
fn decline_with_note(data_dir: &Path, approval_id: &str, note: &str) -> String {
    common::seed::block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        agent_hub::store::questions::decide(&db, 0, "human", approval_id, false, Some(note), None)
            .await
            .expect("decide")
    })
}

#[test]
fn an_agent_reads_the_outcome_of_its_approval_and_the_note_left_with_it() {
    let data_dir = TempDir::new("decision-note");
    common::seed::seed_project(data_dir.path(), "proj");

    let mut server = spawn(data_dir.path(), &[]);
    server.initialize();
    let asked = server.call_tool(
        "signal_append",
        json!({"project_id": "proj", "kind": "approval", "summary": "Restart the node?"}),
    );
    let approval_id = structured(&asked)["event_id"]
        .as_str()
        .expect("event id")
        .to_string();
    // Only one process may hold the engine, so the agent steps aside while
    // the human decides.
    drop(server);
    let answer_id = decline_with_note(data_dir.path(), &approval_id, "wait for the backup");

    let mut server = spawn(data_dir.path(), &[]);
    server.initialize();
    let resolved = server.call_tool("inbox_read", json!({"status": "resolved"}));
    let item = item_with_id(items(&resolved), &approval_id);
    assert_eq!(item["decision"]["decision"], "declined", "{item}");
    assert_eq!(item["decision"]["note"], "wait for the backup", "{item}");
    assert_eq!(item["decision"]["event_id"], answer_id.as_str());

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let events = structured(&feed)["events"]
        .as_array()
        .unwrap_or_else(|| panic!("feed_read returns events: {feed}"))
        .clone();
    let answer = events
        .iter()
        .find(|event| event["id"] == answer_id.as_str())
        .unwrap_or_else(|| panic!("the decision is on the feed: {events:?}"));
    assert_eq!(answer["thread_id"], approval_id.as_str());
    assert_eq!(answer["payload"]["decision"], "declined");
    assert_eq!(answer["payload"]["note"], "wait for the backup");
}

#[test]
fn question_options_round_trip_into_inbox_read_and_the_feed() {
    let data_dir = TempDir::new("options");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path(), &[]);
    server.initialize();

    let asked = server.call_tool(
        "question_post",
        json!({
            "project_id": "proj",
            "subject": "Keep the old export format?",
            "options": ["  Keep it ", "Drop it", "Ask me next week"],
        }),
    );
    let question_id = structured(&asked)["question_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a question with options is admitted: {asked}"))
        .to_string();

    let action = server.call_tool("inbox_read", json!({"status": "action"}));
    let item = item_with_id(items(&action), &question_id);
    assert_eq!(
        item["payload"]["options"],
        json!(["Keep it", "Drop it", "Ask me next week"]),
        "the options are stored trimmed and in the order given"
    );

    let feed = server.call_tool("feed_read", json!({"project_id": "proj"}));
    let event = structured(&feed)["events"]
        .as_array()
        .expect("feed_read returns events")
        .iter()
        .find(|event| event["id"] == question_id.as_str())
        .unwrap_or_else(|| panic!("the feed carries the question: {feed}"))
        .clone();
    assert_eq!(
        event["payload"]["options"], item["payload"]["options"],
        "the feed event carries the same options"
    );

    // A pick is an ordinary answer whose body is the option's text.
    let answered = server.call_tool(
        "answer_post",
        json!({"question_id": question_id, "body": "Drop it"}),
    );
    structured(&answered)["event_id"]
        .as_str()
        .unwrap_or_else(|| panic!("answer_post accepts an option's text: {answered}"));
    let resolved = server.call_tool("inbox_read", json!({"status": "resolved"}));
    let item = item_with_id(items(&resolved), &question_id);
    assert_eq!(item["answer"]["body"], "Drop it");
    assert_eq!(
        item["payload"]["options"],
        json!(["Keep it", "Drop it", "Ask me next week"]),
        "a resolved question still says what it offered"
    );

    let plain = server.call_tool(
        "question_post",
        json!({"project_id": "proj", "subject": "Anything else?", "body": "Say so."}),
    );
    let plain_id = structured(&plain)["question_id"]
        .as_str()
        .expect("a question without options is admitted")
        .to_string();
    let action = server.call_tool("inbox_read", json!({"status": "action"}));
    let item = item_with_id(items(&action), &plain_id);
    assert_eq!(
        item["payload"],
        json!({"body": "Say so."}),
        "a question without options carries the payload it always did"
    );
}

#[test]
fn malformed_question_options_are_refused_and_post_nothing() {
    let data_dir = TempDir::new("options-refused");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut server = spawn(data_dir.path(), &[]);
    server.initialize();

    let long = "x".repeat(81);
    let refused: [(Value, &str); 9] = [
        (json!([]), "an empty list"),
        (json!(["Yes"]), "a single option"),
        (json!(["a", "b", "c", "d", "e", "f", "g"]), "seven options"),
        (json!(["Yes", "   "]), "a blank option"),
        (json!(["Yes", " Yes"]), "a repeated option"),
        (json!(["Yes", long]), "an option over 80 characters"),
        (json!(["Yes", "No\nreally"]), "an option over two lines"),
        (
            json!(["Yes", "No\u{2028}really"]),
            "an option split by a line separator",
        ),
        (
            json!(["Yes", "No\u{2029}really"]),
            "an option split by a paragraph separator",
        ),
    ];
    for (options, what) in refused {
        let response = server.call_tool(
            "question_post",
            json!({"project_id": "proj", "subject": "Ship?", "options": options}),
        );
        assert_eq!(
            error_code(&response),
            "invalid_argument",
            "{what} is refused: {response}"
        );
    }
    let action = server.call_tool("inbox_read", json!({"status": "action"}));
    assert_eq!(items(&action).len(), 0, "a refused list posts no question");

    let widest = "y".repeat(80);
    let admitted = server.call_tool(
        "question_post",
        json!({
            "project_id": "proj",
            "subject": "Ship?",
            "options": ["a", "b", "c", "d", "e", widest],
        }),
    );
    structured(&admitted)["question_id"]
        .as_str()
        .unwrap_or_else(|| panic!("six options of up to 80 characters are admitted: {admitted}"));
}
