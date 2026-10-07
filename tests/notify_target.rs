//! The contentless nudge to an operator-configured target.
//!
//! A receiver bound on a loopback port stands in for the operator's ntfy or
//! webhook endpoint. The hub is driven through its real router, the way an
//! agent reaches it, and the receiver records every POST it is sent.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_hub::config::Config;
use agent_hub::notify::{BODY, NotifyTarget};
use agent_hub::store::projects;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::routing::post;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tower::ServiceExt;

mod common;

use common::state::TestState;

const PROTOCOL_VERSION: &str = "2025-06-18";
const PROJECT: &str = "zebra-launch";
const AGENT: &str = "agent-quokka";
const SUBJECT: &str = "Ship the nightly build to production?";
const TOKEN: &str = "tk_notify_bearer";

/// One POST the receiver was sent.
#[derive(Clone, Debug)]
struct Hit {
    at: Instant,
    authorization: Option<String>,
    content_type: Option<String>,
    body: String,
}

/// A stand-in for the operator's target on a loopback port.
struct Receiver {
    url: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Receiver {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind receiver");
        let addr = listener.local_addr().expect("receiver address");
        let hits = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/agent-hub", post(record))
            .with_state(hits.clone());
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self {
            url: format!("http://{addr}/agent-hub"),
            hits,
        }
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    /// Wait until the receiver holds `count` hits, or fail after `limit`.
    async fn wait_for(&self, count: usize, limit: Duration) -> Vec<Hit> {
        let deadline = Instant::now() + limit;
        loop {
            let hits = self.hits();
            if hits.len() >= count {
                return hits;
            }
            assert!(
                Instant::now() < deadline,
                "expected {count} sends within {limit:?}, got {}",
                hits.len()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn record(State(hits): State<Arc<Mutex<Vec<Hit>>>>, headers: HeaderMap, body: String) {
    let header = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    hits.lock().unwrap().push(Hit {
        at: Instant::now(),
        authorization: header(header::AUTHORIZATION),
        content_type: header(header::CONTENT_TYPE),
        body,
    });
}

/// A hub with one project and one agent, mounted the way it is served.
struct Hub {
    state: TestState,
    app: Router,
    token: String,
}

impl Hub {
    async fn start(tag: &str, adjust: impl FnOnce(&mut Config)) -> Self {
        let state = common::state::open_with(tag, adjust).await;
        projects::create(&state.db, PROJECT, "Zebra launch")
            .await
            .expect("create project");
        let token = common::seed::agent_token(&state.db, AGENT, "Quokka").await;
        let app = agent_hub::http::router(state.clone())
            .merge(agent_hub::mcp::http_router(state.clone()));
        Self { state, app, token }
    }

    /// Initialize an MCP session and return its id.
    async fn session(&self) -> String {
        let (_, session) = self
            .post(
                None,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": {"name": "notify-tests", "version": "0.0.0"},
                    },
                }),
            )
            .await;
        let session = session.expect("the hub names an MCP session");
        self.post(
            Some(&session),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        session
    }

    /// Call one tool and return its structured result, failing on an error.
    async fn call(&self, session: &str, tool: &str, arguments: Value) -> Value {
        let (reply, _) = self
            .post(
                Some(session),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {"name": tool, "arguments": arguments},
                }),
            )
            .await;
        let result = reply
            .get("result")
            .unwrap_or_else(|| panic!("{tool} answered: {reply}"));
        assert_ne!(result["isError"], json!(true), "{tool} failed: {reply}");
        result["structuredContent"].clone()
    }

    async fn ask(&self, session: &str, subject: &str) -> String {
        let asked = self
            .call(
                session,
                "question_post",
                json!({"project_id": PROJECT, "subject": subject, "body": "The release is ready."}),
            )
            .await;
        asked["question_id"]
            .as_str()
            .unwrap_or_else(|| panic!("question_post returns its id: {asked}"))
            .to_string()
    }

    async fn post(&self, session: Option<&str>, message: Value) -> (Value, Option<String>) {
        let wanted = message.get("id").cloned();
        let mut builder = Request::builder()
            .uri("/mcp")
            .method("POST")
            .header(header::HOST, "hub.test")
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(session) = session {
            builder = builder
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", PROTOCOL_VERSION);
        }
        let response = self
            .app
            .clone()
            .oneshot(
                builder
                    .body(Body::from(message.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("response");
        let named = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let Some(wanted) = wanted else {
            return (Value::Null, named);
        };
        let mut stream = response.into_body().into_data_stream();
        let mut text = String::new();
        let reply = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(chunk) = stream.next().await {
                text.push_str(&String::from_utf8_lossy(&chunk.expect("read chunk")));
                for line in text.lines() {
                    let payload = line.strip_prefix("data:").unwrap_or(line).trim();
                    if let Ok(value) = serde_json::from_str::<Value>(payload)
                        && value.get("id") == Some(&wanted)
                    {
                        return value;
                    }
                }
            }
            panic!("the agent transport closed without a reply: {text}");
        })
        .await
        .expect("the agent transport replied in time");
        (reply, named)
    }
}

fn target(url: &str, interval: Duration) -> NotifyTarget {
    NotifyTarget {
        url: url.parse().expect("target URL"),
        token: Some(TOKEN.to_string()),
        interval,
    }
}

#[tokio::test]
async fn a_new_question_sends_one_contentless_post_with_the_bearer() {
    let receiver = Receiver::start().await;
    let interval = Duration::from_millis(300);
    let url = receiver.url.clone();
    let hub = Hub::start("notify-one", |config| {
        config.notify = Some(target(&url, interval))
    })
    .await;
    let session = hub.session().await;

    let question_id = hub.ask(&session, SUBJECT).await;

    let hits = receiver.wait_for(1, Duration::from_secs(5)).await;
    let hit = &hits[0];
    assert_eq!(hit.body, BODY);
    assert_eq!(
        hit.authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(
        hit.content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("text/plain")),
        "{hit:?}"
    );
    for named in [PROJECT, "Zebra", AGENT, "Quokka", SUBJECT, &question_id] {
        assert!(
            !hit.body.contains(named),
            "the body names nothing about the item, but carries '{named}': {hit:?}"
        );
    }
    assert!(
        !hit.body.chars().any(|c| c.is_ascii_digit()),
        "the body carries no count: {hit:?}"
    );

    tokio::time::sleep(interval * 3).await;
    assert_eq!(
        receiver.hits().len(),
        1,
        "one item is one send, with no trailing send when nothing else arrived"
    );
}

#[tokio::test]
async fn items_inside_the_interval_coalesce_into_one_send_and_a_trailing_send() {
    let receiver = Receiver::start().await;
    let interval = Duration::from_secs(1);
    let url = receiver.url.clone();
    let hub = Hub::start("notify-coalesce", |config| {
        config.notify = Some(target(&url, interval))
    })
    .await;
    let session = hub.session().await;

    hub.ask(&session, "First question").await;
    receiver.wait_for(1, Duration::from_secs(5)).await;
    // Inside the quiet period: two questions and an approval.
    hub.ask(&session, "Second question").await;
    hub.ask(&session, "Third question").await;
    hub.call(
        &session,
        "signal_append",
        json!({"project_id": PROJECT, "kind": "approval", "summary": "Rotate the key?"}),
    )
    .await;

    let hits = receiver.wait_for(2, Duration::from_secs(5)).await;
    let gap = hits[1].at.duration_since(hits[0].at);
    assert!(
        gap >= interval - Duration::from_millis(50),
        "the trailing send waits out the interval, came after {gap:?}"
    );
    assert!(hits.iter().all(|hit| hit.body == BODY), "{hits:?}");

    tokio::time::sleep(interval * 2 + Duration::from_millis(500)).await;
    assert_eq!(
        receiver.hits().len(),
        2,
        "the items in the quiet period are one trailing send, not one each"
    );
}

#[tokio::test]
async fn an_ordinary_signal_sends_nothing() {
    let receiver = Receiver::start().await;
    let url = receiver.url.clone();
    let hub = Hub::start("notify-signal", |config| {
        config.notify = Some(target(&url, Duration::from_millis(100)))
    })
    .await;
    let session = hub.session().await;

    hub.call(
        &session,
        "signal_append",
        json!({"project_id": PROJECT, "kind": "finished", "summary": "Nightly report done"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        receiver.hits().is_empty(),
        "finished work does not wait on the human"
    );

    hub.call(
        &session,
        "signal_append",
        json!({"project_id": PROJECT, "kind": "approval", "summary": "Rotate the key?"}),
    )
    .await;
    receiver.wait_for(1, Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_replayed_question_or_approval_sends_nothing() {
    let receiver = Receiver::start().await;
    let interval = Duration::from_millis(200);
    let url = receiver.url.clone();
    let hub = Hub::start("notify-replay", |config| {
        config.notify = Some(target(&url, interval))
    })
    .await;
    let session = hub.session().await;

    let question =
        json!({"project_id": PROJECT, "subject": SUBJECT, "idempotency_key": "ask-once"});
    let approval = json!({
        "project_id": PROJECT,
        "kind": "approval",
        "summary": "Rotate the key?",
        "idempotency_key": "approve-once",
    });
    let asked = hub.call(&session, "question_post", question.clone()).await;
    let approved = hub.call(&session, "signal_append", approval.clone()).await;
    receiver.wait_for(1, Duration::from_secs(5)).await;
    tokio::time::sleep(interval * 3).await;
    let before = receiver.hits().len();

    // Retried outside the quiet period, each returns the original event and
    // posts nothing, so there is nothing new to say.
    let asked_again = hub.call(&session, "question_post", question).await;
    let approved_again = hub.call(&session, "signal_append", approval).await;
    assert_eq!(asked_again["question_id"], asked["question_id"]);
    assert_eq!(approved_again["event_id"], approved["event_id"]);
    tokio::time::sleep(interval * 3).await;
    assert_eq!(
        receiver.hits().len(),
        before,
        "a replay waits on nothing new, so it sends nothing"
    );
}

#[tokio::test]
async fn nothing_is_sent_when_no_target_is_configured() {
    let receiver = Receiver::start().await;
    let hub = Hub::start("notify-off", |_| {}).await;
    assert!(!hub.state.notifier.enabled());
    let session = hub.session().await;

    hub.ask(&session, SUBJECT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(receiver.hits().is_empty());
}

#[tokio::test]
async fn an_unreachable_target_does_not_fail_or_hold_up_the_question() {
    // A port that was bound and released refuses the connection.
    let refused = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("address")
    };
    // A listener that accepts and never answers holds a send until it times
    // out, which is longer than the question may take.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind silent");
    let silent_addr = silent.local_addr().expect("address");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = silent.accept().await {
            held.push(socket);
        }
    });

    for (tag, addr) in [("notify-refused", refused), ("notify-silent", silent_addr)] {
        let failed_before = failed_sends();
        let url = format!("http://{addr}/agent-hub");
        let hub = Hub::start(tag, |config| {
            config.notify = Some(target(&url, Duration::from_millis(100)))
        })
        .await;
        let session = hub.session().await;

        let started = Instant::now();
        let question_id = hub.ask(&session, SUBJECT).await;
        assert!(!question_id.is_empty());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{tag}: the question waited on the target for {:?}",
            started.elapsed()
        );

        if tag == "notify-refused" {
            let deadline = Instant::now() + Duration::from_secs(5);
            while failed_sends() <= failed_before {
                assert!(Instant::now() < deadline, "the failed send is counted");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

/// The failed-send counter in the process registry.
fn failed_sends() -> u64 {
    agent_hub::metrics::render()
        .lines()
        .find_map(|line| line.strip_prefix("agenthub_notify_sends_total{result=\"failed\"} "))
        .and_then(|value| value.parse().ok())
        .expect("the notify family is rendered")
}

#[tokio::test]
async fn a_rejecting_target_is_a_failed_send() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let app = Router::new().route("/agent-hub", post(|| async { StatusCode::UNAUTHORIZED }));
    tokio::spawn(async move { axum::serve(listener, app).await });

    let failed_before = failed_sends();
    let url = format!("http://{addr}/agent-hub");
    let hub = Hub::start("notify-rejected", |config| {
        config.notify = Some(target(&url, Duration::from_millis(100)))
    })
    .await;
    let session = hub.session().await;
    hub.ask(&session, SUBJECT).await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while failed_sends() <= failed_before {
        assert!(Instant::now() < deadline, "a 401 is counted as failed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_enrolment_request_sends_the_same_contentless_post() {
    let receiver = Receiver::start().await;
    let url = receiver.url.clone();
    let hub = Hub::start("notify-enrol", |config| {
        config.notify = Some(target(&url, Duration::from_millis(100)))
    })
    .await;

    let response = hub
        .app
        .clone()
        .oneshot(common::http::post(
            "/api/v1/enrol",
            None,
            Some(json!({
                "suggested_id": "agent-wombat",
                "display_name": "Wombat",
                "why": "Nightly builds",
            })),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let hits = receiver.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(hits[0].body, BODY);
    assert!(!hits[0].body.contains("Wombat"), "{:?}", hits[0]);
}
