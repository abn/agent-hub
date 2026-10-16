use serde_json::{Value, json};
use std::time::{Duration, Instant};

mod common;
use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::mcp_post;

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
