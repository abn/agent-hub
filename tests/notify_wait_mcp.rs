use serde_json::{Value, json};
use std::time::{Duration, Instant};

mod common;
use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{mcp_post, rest};

const ADMIN_TOKEN: &str = "notify-wait-admin";
const PROJECT: &str = "proj";
const AGENT: &str = "waiter";
const OTHER: &str = "poster";

#[derive(Clone)]
struct Connection {
    port: u16,
    token: String,
    session: String,
}

impl Connection {
    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = mcp_post(
            self.port,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string(),
            Some(&self.token),
            Some(&self.session),
        );
        let message = response.message();
        assert!(message.get("error").is_none(), "{name} failed: {message}");
        message
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} no structured: {message}"))
    }
}

impl Connection {
    /// The JSON-RPC message a call answered with, error or not.
    fn attempt(&self, name: &str, arguments: Value) -> Value {
        mcp_post(
            self.port,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string(),
            Some(&self.token),
            Some(&self.session),
        )
        .message()
    }

    fn start_session(&self, name: &str) -> String {
        self.call(
            "session_start",
            json!({"project_id": PROJECT, "session_name": name}),
        )["session_id"]
            .as_str()
            .expect("session id")
            .to_string()
    }

    /// The titles a `notify_wait` that polls once delivers.
    fn delivered(&self, arguments: Value) -> Vec<String> {
        self.call("notify_wait", arguments)
            .pointer("/notifications/pending")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["title"].as_str().expect("title").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn connect(port: u16, token: &str) -> Connection {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"x","version":"0"}}}).to_string();
    let response = mcp_post(port, &body, Some(token), None);
    let session = response.header("mcp-session-id").expect("session");
    mcp_post(
        port,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        Some(token),
        Some(&session),
    );
    Connection {
        port,
        token: token.to_string(),
        session,
    }
}

#[tokio::test]
async fn notify_wait_wakes_on_subscribed_event() {
    let dir = TempDir::new("notify-wait-wake");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    let other_token = common::seed::agent_token(&db, OTHER, "Poster").await;
    drop(db);

    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = connect(port, &token);
    let other = connect(port, &other_token);

    let sub = agent.call(
        "notify_subscribe",
        json!({"kinds":"signal","project_id":PROJECT}),
    );
    assert!(sub.get("subscription_id").is_some());

    let waiter = agent.clone();
    let handle = std::thread::spawn(move || {
        let started = Instant::now();
        let result = waiter.call("notify_wait", json!({"wait_seconds": 20}));
        (started.elapsed(), result)
    });

    std::thread::sleep(Duration::from_millis(300));
    let appended = other.call(
        "signal_append",
        json!({"project_id":PROJECT,"kind":"signal","summary":"hello"}),
    );
    assert!(appended.get("event_id").is_some());

    let (elapsed, result) = handle.join().unwrap();
    let pending = result
        .pointer("/notifications/pending")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("notifications pending missing: {result}"));
    assert_eq!(pending.len(), 1, "{result}");
    assert_eq!(pending[0]["title"], "hello");
    assert!(elapsed < Duration::from_secs(20));
    drop(child);
}

#[tokio::test]
async fn notify_wait_times_out_empty() {
    let dir = TempDir::new("notify-wait-timeout");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    drop(db);

    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = connect(port, &token);
    let started = Instant::now();
    let result = agent.call("notify_wait", json!({"wait_seconds": 1}));
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(800));
    let pending = result.pointer("/notifications/pending");
    assert!(
        pending.is_none()
            || pending
                .unwrap()
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true)
    );
    drop(child);
}

#[tokio::test]
async fn notify_list_returns_registered_subscriptions() {
    let dir = TempDir::new("notify-list");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    drop(db);

    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = connect(port, &token);

    let sub = agent.call(
        "notify_subscribe",
        json!({"kinds":"signal,finished","project_id":PROJECT}),
    );
    let id = sub["subscription_id"].as_str().expect("id").to_string();

    let listed = agent.call("notify_list", json!({}));
    let rows = listed["subscriptions"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["subscription_id"], id);
    assert_eq!(rows[0]["kinds"], json!(["signal", "finished"]));
    assert_eq!(rows[0]["project_id"], PROJECT);
    drop(child);
}

/// Two connections of one actor, each in a session of its own.
struct Pair {
    first: Connection,
    second: Connection,
    first_id: String,
    second_id: String,
    other: Connection,
    port: u16,
}

fn pair(port: u16, agent_token: &str, other_token: &str) -> Pair {
    let first = connect(port, agent_token);
    let second = connect(port, agent_token);
    let first_id = first.start_session("first");
    let second_id = second.start_session("second");
    Pair {
        first,
        second,
        first_id,
        second_id,
        other: connect(port, other_token),
        port,
    }
}

#[tokio::test]
async fn a_signal_reaches_each_session_of_one_actor_once() {
    let dir = TempDir::new("notify-two-sessions");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    let other_token = common::seed::agent_token(&db, OTHER, "Poster").await;
    drop(db);
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let pair = pair(child.port(), &token, &other_token);

    for connection in [&pair.first, &pair.second] {
        connection.call(
            "notify_subscribe",
            json!({"kinds":"signal","project_id":PROJECT}),
        );
    }
    pair.other.call(
        "signal_append",
        json!({"project_id":PROJECT,"kind":"signal","summary":"for both"}),
    );

    assert_eq!(
        pair.first.delivered(json!({"wait_seconds": 0})),
        vec!["for both"]
    );
    assert_eq!(
        pair.second.delivered(json!({"wait_seconds": 0})),
        vec!["for both"],
        "the first session's read did not take the second's copy"
    );
    for connection in [&pair.first, &pair.second] {
        assert!(
            connection.delivered(json!({"wait_seconds": 0})).is_empty(),
            "each session is shown it once"
        );
    }
    drop(child);
}

#[tokio::test]
async fn an_answer_goes_to_the_session_that_asked() {
    let dir = TempDir::new("notify-session-answer");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    let other_token = common::seed::agent_token(&db, OTHER, "Poster").await;
    drop(db);
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let pair = pair(child.port(), &token, &other_token);

    let asked = pair.first.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Deploy?", "body": "Ready."}),
    );
    let answered = rest(
        pair.port,
        "POST",
        &format!(
            "/api/v1/questions/{}/answer",
            asked["question_id"].as_str().expect("question id")
        ),
        Some(ADMIN_TOKEN),
        Some(&json!({"body": "yes"}).to_string()),
    );
    assert_eq!(answered.status, 200, "{}", answered.raw);

    assert!(
        pair.second.delivered(json!({"wait_seconds": 0})).is_empty(),
        "the second session is not shown the first's answer"
    );
    let plain = pair.second.call("version", json!({}));
    assert!(plain.get("notifications").is_none(), "{plain}");
    assert_eq!(
        pair.first.delivered(json!({"wait_seconds": 0})),
        vec!["answered: Deploy?"]
    );
    drop(child);
}

#[tokio::test]
async fn a_connection_with_no_lease_reads_a_session_it_names() {
    let dir = TempDir::new("notify-named-session");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    let other_token = common::seed::agent_token(&db, OTHER, "Poster").await;
    drop(db);
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let pair = pair(child.port(), &token, &other_token);

    pair.first.call(
        "notify_subscribe",
        json!({"kinds":"signal","project_id":PROJECT}),
    );
    pair.other.call(
        "signal_append",
        json!({"project_id":PROJECT,"kind":"signal","summary":"hello"}),
    );

    // A watcher holds no lease: it names the session it works under.
    let watcher = connect(pair.port, &token);
    assert!(
        watcher
            .delivered(json!({"wait_seconds": 0, "session": {"session_id": pair.second_id}}))
            .is_empty(),
        "the second session never subscribed"
    );
    assert!(
        watcher.delivered(json!({"wait_seconds": 0})).is_empty(),
        "with no session the watcher reads only what is not session-bound"
    );
    assert_eq!(
        watcher.delivered(json!({"wait_seconds": 0, "session": {"session_id": pair.first_id}})),
        vec!["hello"]
    );
    assert!(
        pair.first.delivered(json!({"wait_seconds": 0})).is_empty(),
        "the read was a delivery: the first session is not shown it again"
    );

    // A subscription made by name is bound to that session, and shows it.
    let bound = watcher.call(
        "notify_subscribe",
        json!({"kinds":"finished","session": {"session_id": pair.second_id}}),
    );
    assert_eq!(bound["session_id"], pair.second_id.as_str());
    let listed = watcher.call("notify_list", json!({}));
    let rows = listed["subscriptions"].as_array().expect("rows");
    assert!(
        rows.iter()
            .any(|row| row["session_id"] == pair.second_id.as_str()
                && row["kinds"] == json!(["finished"])),
        "{listed}"
    );
    drop(child);
}

#[tokio::test]
async fn a_wait_that_finds_nothing_in_the_named_scope_leaves_the_connections_own_items() {
    let dir = TempDir::new("notify-wait-empty-named");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    let other_token = common::seed::agent_token(&db, OTHER, "Poster").await;
    drop(db);
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let pair = pair(child.port(), &token, &other_token);

    let asked = pair.first.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Deploy?", "body": "Ready."}),
    );
    let answered = rest(
        pair.port,
        "POST",
        &format!(
            "/api/v1/questions/{}/answer",
            asked["question_id"].as_str().expect("question id")
        ),
        Some(ADMIN_TOKEN),
        Some(&json!({"body": "yes"}).to_string()),
    );
    assert_eq!(answered.status, 200, "{}", answered.raw);

    // The first connection is leased to the first session and asks about the
    // second: nothing is there, and the answer to its own question stays put.
    let elsewhere = pair.first.call(
        "notify_wait",
        json!({"wait_seconds": 0, "session": {"session_id": pair.second_id}}),
    );
    assert_eq!(
        elsewhere.pointer("/notifications/pending"),
        Some(&json!([])),
        "an empty wait says so itself: {elsewhere}"
    );
    assert_eq!(
        pair.first.delivered(json!({"wait_seconds": 0})),
        vec!["answered: Deploy?"],
        "the connection's own scope was not drained on top of it"
    );
    drop(child);
}

#[tokio::test]
async fn only_the_callers_own_running_session_may_be_named() {
    let dir = TempDir::new("notify-foreign-session");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .unwrap();
    let token = common::seed::agent_token(&db, AGENT, "Waiter").await;
    let other_token = common::seed::agent_token(&db, OTHER, "Poster").await;
    drop(db);
    let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
    let pair = pair(child.port(), &token, &other_token);

    for tool in ["notify_wait", "notify_subscribe"] {
        let arguments = if tool == "notify_wait" {
            json!({"wait_seconds": 0, "session": {"session_id": pair.first_id}})
        } else {
            json!({"kinds": "signal", "session": {"session_id": pair.first_id}})
        };
        let refused = pair.other.attempt(tool, arguments);
        assert_eq!(
            refused.pointer("/error/data/error/code"),
            Some(&json!("forbidden")),
            "{tool} on another agent's session: {refused}"
        );
    }

    pair.first
        .call("session_end", json!({"session_id": pair.first_id}));
    let ended = pair.first.attempt(
        "notify_wait",
        json!({"wait_seconds": 0, "session": {"session_id": pair.first_id}}),
    );
    assert_eq!(
        ended.pointer("/error/data/error/code"),
        Some(&json!("conflict")),
        "an ended session is not one to read: {ended}"
    );
    drop(child);
}
