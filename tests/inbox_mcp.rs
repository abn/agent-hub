//! End-to-end tests for the MCP inbox and question tools over stdio.
//!
//! Spawns the built binary as `agent-hub mcp`, appends a finished event,
//! reads the inbox, posts a question, answers it, and reads the inbox again
//! to prove unread, action, and resolved states. Any non-JSON stdout line
//! fails the test because it would corrupt the protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

mod common;

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const PROTOCOL_VERSION: &str = "2025-06-18";
const AGENT_ID: &str = "mcp-inbox";

/// A temp data directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-hub-inbox-mcp-{}-{nanos}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp data dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stdio MCP client: spawn, send, and wait for one response id.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn spawn(data_dir: &Path, envs: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hub"));
        command
            .arg("mcp")
            .env("RUST_LOG", "error")
            .env("HUB_DATA_DIR", data_dir)
            .env("HUB_AGENT_ID", AGENT_ID)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .expect("failed to spawn the agent-hub binary");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).expect("serialise message");
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to child stdin");
        self.stdin.flush().expect("flush child stdin");
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.wait_for(id)
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn wait_for(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(READ_TIMEOUT)
                .unwrap_or_else(|_| panic!("timed out waiting for a response to request {id}"));
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|err| {
                panic!(
                    "stdout carried a non-JSON line, which corrupts the protocol: {err}: {line:?}"
                )
            });
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    fn initialize(&mut self) {
        let init = self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-inbox", "version": "0.0.0"},
            }),
        );
        assert!(init.get("result").is_some(), "initialize returns a result");
        self.notify("notifications/initialized");
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name": name, "arguments": arguments}))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn structured(response: &Value) -> &Value {
    response
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        .unwrap_or_else(|| panic!("tool result carries structured content: {response}"))
}

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
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0, &[]);
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
            "to": "human",
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
    assert_eq!(
        item_with_id(resolved_items, &question_id)["status"],
        "resolved"
    );

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
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0, &[("HUB_INBOX_ACTION_PER_AGENT", "2")]);
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
    common::seed::seed_project(&data_dir.0, "proj");
    let mut server = McpServer::spawn(&data_dir.0, &[("HUB_INBOX_ACTION_PER_AGENT", "1")]);
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    runtime.block_on(async {
        let db = agent_hub::store::open_engine(&data_dir.join("hub.db"))
            .await
            .expect("open engine");
        agent_hub::store::migrate(&db).await.expect("migrate");
        let event_id = agent_hub::store::events::append(
            &db,
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
    common::seed::seed_project(&data_dir.0, "proj");
    let event_id = seed_read_report(&data_dir.0, "nightly report done");
    let mut server = McpServer::spawn(&data_dir.0, &[]);
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
