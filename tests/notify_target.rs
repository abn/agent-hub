//! Notices to the operator's targets.
//!
//! A receiver bound on a loopback port stands in for the operator's ntfy,
//! webhook or chat endpoint. The hub is driven through its real router, the
//! way an agent and the operator reach it, and the receiver records every
//! request it is sent, answering with the statuses a check scripts.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_hub::config::Config;
use agent_hub::notify::{BODY, NotifyTarget};
use agent_hub::store::projects;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri, header};
use axum::routing::post;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tower::ServiceExt;

mod common;

use common::state::{ADMIN_TOKEN, TestState};

const PROTOCOL_VERSION: &str = "2025-06-18";
const PROJECT: &str = "zebra-launch";
const AGENT: &str = "agent-quokka";
const SUBJECT: &str = "Ship the nightly build to production?";
const TOKEN: &str = "tk_notify_bearer";
const PUBLIC_URL: &str = "https://hub.example";

/// One request the receiver was sent.
#[derive(Clone, Debug)]
struct Hit {
    at: Instant,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    authorization: Option<String>,
    content_type: Option<String>,
    body: String,
}

impl Hit {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("a JSON body: {}", self.body))
    }
}

#[derive(Clone, Default)]
struct Shared {
    hits: Arc<Mutex<Vec<Hit>>>,
    replies: Arc<Mutex<VecDeque<StatusCode>>>,
}

/// A stand-in for the operator's target on a loopback port. It takes any
/// method on any path.
struct Receiver {
    base: String,
    url: String,
    shared: Shared,
}

impl Receiver {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind receiver");
        let addr = listener.local_addr().expect("receiver address");
        let shared = Shared::default();
        let app = Router::new().fallback(record).with_state(shared.clone());
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self {
            base: format!("http://{addr}"),
            url: format!("http://{addr}/agent-hub"),
            shared,
        }
    }

    /// Answer the next requests with these statuses, then 200.
    fn reply(&self, statuses: &[StatusCode]) {
        self.shared.replies.lock().unwrap().extend(statuses);
    }

    fn hits(&self) -> Vec<Hit> {
        self.shared.hits.lock().unwrap().clone()
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

async fn record(
    State(shared): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> StatusCode {
    // The hub only ever POSTs or PUTs. A loopback port can be probed by other
    // software on the machine, and its GET is not a send.
    if method == Method::GET {
        return StatusCode::NOT_FOUND;
    }
    let header = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    shared.hits.lock().unwrap().push(Hit {
        at: Instant::now(),
        method,
        uri,
        authorization: header(header::AUTHORIZATION),
        content_type: header(header::CONTENT_TYPE),
        headers,
        body,
    });
    shared
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(StatusCode::OK)
}

/// A hub with one project and one agent, mounted the way it is served.
struct Hub {
    state: TestState,
    app: Router,
    token: String,
}

impl Hub {
    async fn start(tag: &str, adjust: impl FnOnce(&mut Config)) -> Self {
        let state = common::state::open_with(tag, |config| {
            // A refused send is retried; the checks wait on the outcome, not
            // on the production backoff.
            config.notify_delivery.backoff = vec![Duration::from_millis(20)];
            adjust(config);
        })
        .await;
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
    assert!(hub.state.notifier.targets().is_empty());
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
                "member": "agent-wombat",
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

impl Hub {
    /// One call on the operator's control surface, with the admin token.
    async fn admin(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(common::http::request(
                method,
                uri,
                Some(&format!("Bearer {ADMIN_TOKEN}")),
                body,
            ))
            .await
            .expect("response");
        let status = response.status();
        let bytes = common::http::body_bytes(response).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Add a target and return its id.
    async fn add_target(&self, body: Value) -> String {
        let (status, created) = self
            .admin("POST", "/api/v1/notify/targets", Some(body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["id"].as_str().expect("an id").to_string()
    }

    async fn send_test(&self, id: &str) -> Value {
        let (status, outcome) = self
            .admin("POST", &format!("/api/v1/notify/targets/{id}/test"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{outcome}");
        outcome
    }
}

/// A hub that knows its public address and makes three quick attempts.
async fn targets_hub(tag: &str) -> Hub {
    Hub::start(tag, |config| {
        config.public_url = Some(PUBLIC_URL.to_string());
        config.notify_delivery.interval = Duration::from_millis(100);
        config.notify_delivery.backoff = vec![Duration::from_millis(20); 2];
    })
    .await
}

#[tokio::test]
async fn each_preset_sends_its_own_shape() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-presets").await;
    let base = &receiver.base;
    let cases = [
        ("webhook", format!("{base}/hook"), Some(TOKEN)),
        ("ntfy", format!("{base}/ntfy/topic"), Some(TOKEN)),
        ("gotify", format!("{base}/message"), Some(TOKEN)),
        (
            "matrix",
            format!("{base}/_matrix/client/v3/rooms/!r:m/send/m.room.message"),
            Some(TOKEN),
        ),
        ("slack", format!("{base}/services/T/B/X"), None),
        ("discord", format!("{base}/api/webhooks/1/x"), None),
        (
            "telegram",
            format!("{base}/bot1:abc/sendMessage?chat_id=42"),
            None,
        ),
        (
            "pushover",
            format!("{base}/1/messages.json?token=app&user=me"),
            None,
        ),
    ];
    for (index, (preset, url, token)) in cases.iter().enumerate() {
        let id = hub
            .add_target(json!({
                "name": preset, "preset": preset, "url": url, "token": token, "content": "title",
            }))
            .await;
        let outcome = hub.send_test(&id).await;
        assert_eq!(outcome["delivered"], json!(true), "{preset}: {outcome}");
        let hit = receiver.wait_for(index + 1, Duration::from_secs(5)).await[index].clone();
        let headline = "Test notification from Agent Hub";
        let link = format!("{PUBLIC_URL}/");
        let text = format!("{headline}\n{link}");
        let bearer = Some(format!("Bearer {TOKEN}"));
        match *preset {
            "webhook" => {
                assert_eq!(hit.method, Method::POST);
                assert_eq!(hit.authorization, bearer);
                let body = hit.json();
                assert_eq!(body["version"], 1);
                assert_eq!(body["event"], "test");
                assert_eq!(body["text"], headline);
                assert_eq!(body["url"], link.as_str());
            }
            "ntfy" => {
                assert_eq!(hit.authorization, bearer);
                assert!(
                    hit.content_type
                        .as_deref()
                        .unwrap()
                        .starts_with("text/plain")
                );
                assert_eq!(hit.body, headline);
                let query = hit.uri.query().unwrap_or_default();
                assert!(query.contains("click="), "{query}");
            }
            "gotify" => {
                assert_eq!(hit.headers["x-gotify-key"], TOKEN);
                let body = hit.json();
                assert_eq!(body["message"], headline);
                assert_eq!(
                    body["extras"]["client::notification"]["click"]["url"],
                    link.as_str()
                );
            }
            "matrix" => {
                assert_eq!(hit.method, Method::PUT);
                assert_eq!(hit.authorization, bearer);
                assert!(
                    hit.uri.path().contains("/send/m.room.message/test-"),
                    "{}",
                    hit.uri
                );
                assert_eq!(
                    hit.json(),
                    json!({"msgtype": "m.text", "body": text, "m.mentions": {}})
                );
            }
            "slack" => assert_eq!(hit.json(), json!({"text": text})),
            "discord" => assert_eq!(
                hit.json(),
                json!({"content": text, "allowed_mentions": {"parse": []}})
            ),
            "telegram" => {
                assert_eq!(hit.uri.query(), None, "the chat id moved into the body");
                assert_eq!(hit.json()["chat_id"], "42");
                assert_eq!(hit.json()["text"], text.as_str());
            }
            "pushover" => {
                assert_eq!(hit.uri.query(), None, "the keys moved into the body");
                let body = hit.json();
                assert_eq!(
                    (body["token"].as_str(), body["user"].as_str()),
                    (Some("app"), Some("me"))
                );
                assert_eq!(body["message"], headline);
                assert_eq!(body["url"], link.as_str());
            }
            other => panic!("no case for {other}"),
        }
        if token.is_none() {
            assert_eq!(
                hit.authorization, None,
                "{preset} carries its secret in the URL"
            );
        }
    }
}

#[tokio::test]
async fn an_ask_says_what_the_level_allows_and_links_back() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-levels").await;
    let mut ids = Vec::new();
    for content in ["none", "title", "full"] {
        ids.push(
            hub.add_target(json!({
                "name": content, "preset": "webhook",
                "url": format!("{}/{content}", receiver.base), "content": content,
            }))
            .await,
        );
    }
    let session = hub.session().await;
    let question_id = hub.ask(&session, SUBJECT).await;
    let hits = receiver.wait_for(3, Duration::from_secs(5)).await;
    let by_path = |path: &str| {
        hits.iter()
            .find(|hit| hit.uri.path() == path)
            .unwrap_or_else(|| panic!("nothing reached {path}"))
            .json()
    };

    let none = by_path("/none");
    assert_eq!(none["text"], BODY);
    assert!(
        none.get("project").is_none() && none.get("url").is_none(),
        "{none}"
    );

    let title = by_path("/title");
    assert_eq!(title["kind"], "question");
    assert_eq!(title["project"], PROJECT);
    assert_eq!(title["text"], "Question waiting in Zebra launch");
    assert_eq!(title["url"], format!("{PUBLIC_URL}/id/{question_id}"));
    assert!(title.get("summary").is_none(), "{title}");

    let full = by_path("/full");
    assert_eq!(full["summary"], SUBJECT);
}

#[tokio::test]
async fn a_confidential_project_is_sent_with_no_content() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-confidential").await;
    projects::set_confidential(&hub.state.db, PROJECT, true)
        .await
        .expect("make confidential");
    agent_hub::store::identity::add_grant(&hub.state.db, AGENT, PROJECT)
        .await
        .expect("grant");
    hub.add_target(json!({
        "name": "full", "preset": "slack", "url": format!("{}/s", receiver.base), "content": "full",
    }))
    .await;
    let session = hub.session().await;
    hub.ask(&session, SUBJECT).await;
    let hits = receiver.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(hits[0].json(), json!({"text": BODY}));
}

#[tokio::test]
async fn a_failing_send_is_retried_and_the_status_says_why() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-retry").await;
    let retried = hub
        .add_target(json!({"name": "flaky", "preset": "webhook", "url": format!("{}/flaky", receiver.base)}))
        .await;
    receiver.reply(&[
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::SERVICE_UNAVAILABLE,
    ]);
    let session = hub.session().await;
    hub.ask(&session, SUBJECT).await;
    receiver.wait_for(3, Duration::from_secs(5)).await;

    // A refusal is not retried, and the operator reads why.
    hub.admin(
        "PATCH",
        &format!("/api/v1/notify/targets/{retried}"),
        Some(json!({"url": format!("{}/gone", receiver.base)})),
    )
    .await;
    receiver.reply(&[StatusCode::NOT_FOUND]);
    let outcome = hub.send_test(&retried).await;
    assert_eq!(outcome["delivered"], json!(false));
    assert!(
        outcome["error"].as_str().unwrap().contains("404"),
        "{outcome}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let hits = receiver.hits();
    assert_eq!(hits.len(), 4, "two retries and one test, no retry of a 404");
    let status = &outcome["target"]["status"];
    assert_eq!(status["failures"], 1);
    assert!(status["last_delivered_at"].is_string(), "{status}");
}

#[tokio::test]
async fn reads_show_the_origin_and_never_the_secret() {
    let receiver = Receiver::start().await;
    let hub = Hub::start("notify-redacted", |config| {
        config.notify = Some(target(&receiver.url, Duration::from_millis(100)));
    })
    .await;
    let id = hub
        .add_target(json!({
            "name": "chat", "preset": "slack", "url": format!("{}/services/T/B/secret-path", receiver.base),
        }))
        .await;
    hub.add_target(json!({
        "name": "own", "preset": "webhook", "url": format!("{}/hook", receiver.base), "token": TOKEN,
    }))
    .await;
    let (status, listed) = hub.admin("GET", "/api/v1/notify/targets", None).await;
    assert_eq!(status, StatusCode::OK);
    let text = listed.to_string();
    assert!(
        !text.contains("secret-path") && !text.contains(TOKEN),
        "{text}"
    );
    let targets = listed["targets"].as_array().unwrap();
    assert_eq!(
        targets[0]["source"], "config",
        "the configuration's target is first"
    );
    assert_eq!(targets[1]["url"], format!("{}/...", receiver.base));
    assert_eq!(targets[2]["token_set"], true);
    assert_eq!(listed["presets"].as_array().unwrap().len(), 8);

    // A change that names no URL keeps the stored one.
    let (status, _) = hub
        .admin(
            "PATCH",
            &format!("/api/v1/notify/targets/{id}"),
            Some(json!({"name": "renamed"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    hub.send_test(&id).await;
    let hits = receiver.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(hits[0].uri.path(), "/services/T/B/secret-path");

    let (status, _) = hub
        .admin("DELETE", "/api/v1/notify/targets/config", None)
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the configuration's target is changed there"
    );
    let (status, _) = hub
        .admin("DELETE", &format!("/api/v1/notify/targets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let response = hub
        .app
        .clone()
        .oneshot(common::http::get("/api/v1/notify/targets", None))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_bad_target_is_refused_before_it_is_stored() {
    let hub = targets_hub("notify-refused-targets").await;
    for body in [
        json!({"name": "x", "preset": "pager", "url": "https://p.example/x"}),
        json!({"name": "x", "preset": "webhook", "url": "https://me:pw@h.example/x"}),
        json!({"name": "x", "preset": "telegram", "url": "https://api.telegram.org/bot1:a/sendMessage"}),
        json!({"name": "x", "preset": "slack", "url": "https://hooks.slack.com/s", "token": "tk"}),
        json!({"name": "x", "preset": "webhook", "url": "https://h.example/x", "content": "everything"}),
        json!({"name": "x", "preset": "webhook", "url": "https://h.example/x", "events": ["failed"]}),
        json!({"name": "", "preset": "webhook", "url": "https://h.example/x"}),
    ] {
        let (status, problem) = hub
            .admin("POST", "/api/v1/notify/targets", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {problem}");
    }
    let (_, listed) = hub.admin("GET", "/api/v1/notify/targets", None).await;
    assert!(listed["targets"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_switched_off_target_sends_nothing() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-off-switch").await;
    let id = hub
        .add_target(
            json!({"name": "hook", "preset": "webhook", "url": format!("{}/h", receiver.base)}),
        )
        .await;
    let (_, changed) = hub
        .admin(
            "PATCH",
            &format!("/api/v1/notify/targets/{id}"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(changed["enabled"], false);
    let session = hub.session().await;
    hub.ask(&session, SUBJECT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(receiver.hits().is_empty());
}

#[tokio::test]
async fn an_item_near_its_deadline_is_announced_once() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-deadline").await;
    hub.add_target(json!({
        "name": "hook", "preset": "webhook", "url": format!("{}/h", receiver.base),
        "content": "title", "events": ["deadline"],
    }))
    .await;
    let session = hub.session().await;
    let posted = hub
        .call(
            &session,
            "signal_append",
            json!({"project_id": PROJECT, "kind": "approval", "summary": "Deploy?", "expires_in_seconds": 600}),
        )
        .await;
    let now = time::OffsetDateTime::now_utc();
    hub.state.settle_deadlines_at(now).await.expect("sweep");
    hub.state
        .settle_deadlines_at(now + time::Duration::seconds(301))
        .await
        .expect("sweep");
    hub.state
        .settle_deadlines_at(now + time::Duration::seconds(400))
        .await
        .expect("sweep");
    let hits = receiver.wait_for(1, Duration::from_secs(5)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(receiver.hits().len(), 1, "warned once, not per sweep");
    let body = hits[0].json();
    assert_eq!(body["event"], "deadline");
    assert_eq!(body["id"], posted["event_id"]);
    assert_eq!(body["text"], "Approval in Zebra launch is due in 5 min");
}

#[tokio::test]
async fn an_agent_ending_its_session_is_announced() {
    let receiver = Receiver::start().await;
    let hub = targets_hub("notify-ended").await;
    hub.add_target(json!({
        "name": "hook", "preset": "webhook", "url": format!("{}/h", receiver.base),
        "content": "title", "events": ["ended"],
    }))
    .await;
    let session = hub.session().await;
    let started = hub
        .call(
            &session,
            "session_start",
            json!({"project_id": PROJECT, "session_name": "nightly"}),
        )
        .await;
    let session_id = started["session_id"]
        .as_str()
        .expect("a session id")
        .to_string();
    hub.call(&session, "session_end", json!({"session_id": session_id}))
        .await;
    let hits = receiver.wait_for(1, Duration::from_secs(5)).await;
    let body = hits[0].json();
    assert_eq!(body["event"], "ended");
    assert_eq!(body["session"], "nightly");
    assert_eq!(body["text"], "Session nightly ended in Zebra launch");
    assert_eq!(body["url"], format!("{PUBLIC_URL}/id/{session_id}"));
}

#[tokio::test]
async fn raising_a_level_while_a_notice_waits_does_not_widen_it() {
    let receiver = Receiver::start().await;
    let hub = Hub::start("notify-fail-closed", |config| {
        config.public_url = Some(PUBLIC_URL.to_string());
        config.notify_delivery.interval = Duration::from_secs(1);
    })
    .await;
    projects::set_confidential(&hub.state.db, PROJECT, true)
        .await
        .expect("make confidential");
    agent_hub::store::identity::add_grant(&hub.state.db, AGENT, PROJECT)
        .await
        .expect("grant");
    let id = hub
        .add_target(json!({
            "name": "chat", "preset": "slack", "url": format!("{}/s", receiver.base), "content": "none",
        }))
        .await;
    let session = hub.session().await;
    hub.ask(&session, "First ask").await;
    receiver.wait_for(1, Duration::from_secs(5)).await;
    // The second notice waits out the quiet period while the level is raised.
    hub.ask(&session, SUBJECT).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (status, _) = hub
        .admin(
            "PATCH",
            &format!("/api/v1/notify/targets/{id}"),
            Some(json!({"content": "full"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let hits = receiver.wait_for(2, Duration::from_secs(5)).await;
    assert_eq!(
        hits[1].json(),
        json!({"text": BODY}),
        "the queued notice was widened"
    );
}

#[tokio::test]
async fn a_url_moved_to_another_origin_drops_the_token() {
    let first = Receiver::start().await;
    let second = Receiver::start().await;
    let hub = targets_hub("notify-token-origin").await;
    let id = hub
        .add_target(json!({
            "name": "hook", "preset": "webhook", "url": format!("{}/a", first.base), "token": TOKEN,
        }))
        .await;

    // A new path on the same origin keeps the token.
    let (_, kept) = hub
        .admin(
            "PATCH",
            &format!("/api/v1/notify/targets/{id}"),
            Some(json!({"url": format!("{}/b", first.base)})),
        )
        .await;
    assert_eq!(kept["token_set"], true);

    let (status, moved) = hub
        .admin(
            "PATCH",
            &format!("/api/v1/notify/targets/{id}"),
            Some(json!({"url": format!("{}/a", second.base)})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(moved["token_set"], false, "{moved}");
    hub.send_test(&id).await;
    let hits = second.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(
        hits[0].authorization, None,
        "the old token went to the new host"
    );

    // Naming a token with the move keeps that one.
    let (_, renamed) = hub
        .admin(
            "PATCH",
            &format!("/api/v1/notify/targets/{id}"),
            Some(json!({"url": format!("{}/a", first.base), "token": "tk_new"})),
        )
        .await;
    assert_eq!(renamed["token_set"], true);
}

#[tokio::test]
async fn a_project_made_confidential_while_a_notice_waits_is_sent_as_a_ping() {
    let receiver = Receiver::start().await;
    let hub = Hub::start("notify-late-confidential", |config| {
        config.public_url = Some(PUBLIC_URL.to_string());
        config.notify_delivery.interval = Duration::from_secs(1);
    })
    .await;
    hub.add_target(json!({
        "name": "chat", "preset": "slack", "url": format!("{}/s", receiver.base), "content": "full",
    }))
    .await;
    let session = hub.session().await;
    hub.ask(&session, "First ask").await;
    let first = receiver.wait_for(1, Duration::from_secs(5)).await;
    assert_ne!(
        first[0].json(),
        json!({"text": BODY}),
        "an open project is named"
    );
    // The second notice waits out the quiet period while the project closes.
    hub.ask(&session, SUBJECT).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    projects::set_confidential(&hub.state.db, PROJECT, true)
        .await
        .expect("make confidential");
    let hits = receiver.wait_for(2, Duration::from_secs(5)).await;
    assert_eq!(
        hits[1].json(),
        json!({"text": BODY}),
        "the queued notice named a confidential project"
    );
}
