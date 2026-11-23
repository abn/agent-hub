//! The peek route: `GET /api/v1/me/pending` reads what the notification
//! trailer would deliver and delivers none of it.
//!
//! Each test runs a hub as its own process, drives agents over MCP, and reads
//! the route over plain HTTP, so what is asserted is what a hook or a watcher
//! sees.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;
use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{HttpResponse, mcp_post, rest, send};

const ADMIN_TOKEN: &str = "me-pending-admin";
const PROJECT: &str = "proj";

/// One MCP connection of one agent.
#[derive(Clone)]
struct Mcp {
    port: u16,
    token: String,
    session: String,
    /// The hub session every call but `session_start` passes, when bound.
    bound: Option<Value>,
}

impl Mcp {
    /// Pass this session on every later call, as a harness does.
    fn bind(mut self, name: &str) -> Self {
        self.bound = Some(json!({"project_id": PROJECT, "name": name}));
        self
    }

    fn connect(port: u16, token: &str) -> Self {
        let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"cli","version":"0"}}}).to_string();
        let response = mcp_post(port, &body, Some(token), None);
        let session = response.header("mcp-session-id").expect("session");
        mcp_post(
            port,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            Some(token),
            Some(&session),
        );
        Self {
            port,
            token: token.to_string(),
            session,
            bound: None,
        }
    }

    fn call(&self, name: &str, arguments: Value) -> Value {
        let mut arguments = arguments;
        if let (Some(bound), Some(object)) = (&self.bound, arguments.as_object_mut())
            && name != "session_start"
        {
            object.insert("session".to_string(), bound.clone());
        }
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

/// A hub with a project and two agents, each with a token.
struct World {
    dir: TempDir,
    agent_token: String,
    other_token: String,
}

async fn world(tag: &str) -> World {
    let dir = TempDir::new(tag);
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, PROJECT, "Project")
        .await
        .expect("project");
    let agent_token = common::seed::agent_token(&db, "walker", "Walker").await;
    let other_token = common::seed::agent_token(&db, "poster", "Poster").await;
    drop(db);
    World {
        dir,
        agent_token,
        other_token,
    }
}

fn json_of(response: &HttpResponse) -> Value {
    serde_json::from_str(response.body().trim())
        .unwrap_or_else(|err| panic!("a JSON body: {err}: {}", response.raw))
}

/// A peek as the agent, answered 200, parsed.
fn peek(port: u16, token: &str, query: &str) -> Value {
    let response = rest(
        port,
        "GET",
        &format!("/api/v1/me/pending{query}"),
        Some(token),
        None,
    );
    assert_eq!(response.status, 200, "{}", response.raw);
    json_of(&response)
}

fn titles(peeked: &Value) -> Vec<String> {
    peeked["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["title"].as_str().expect("title").to_string())
        .collect()
}

fn signal(from: &Mcp, summary: &str) -> String {
    from.call(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "signal", "summary": summary}),
    )["event_id"]
        .as_str()
        .expect("event id")
        .to_string()
}

fn subscribe(agent: &Mcp, kinds: &str) {
    agent.call(
        "notify_subscribe",
        json!({"kinds": kinds, "project_id": PROJECT}),
    );
}

fn answer(port: u16, question_id: &str) {
    let response = rest(
        port,
        "POST",
        &format!("/api/v1/questions/{question_id}/answer"),
        Some(ADMIN_TOKEN),
        Some(&json!({"body": "yes"}).to_string()),
    );
    assert_eq!(response.status, 200, "{}", response.raw);
}

#[tokio::test]
async fn a_peek_reads_without_delivering_and_a_later_wait_still_delivers() {
    let world = world("me-pending-read").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let other = Mcp::connect(port, &world.other_token);
    subscribe(&agent, "signal");
    signal(&other, "one");

    let first = peek(port, &world.agent_token, "");
    assert_eq!(first["count"], 1, "{first}");
    assert_eq!(titles(&first), vec!["one"]);
    let item = &first["items"][0];
    assert_eq!(item["source"], "subscription");
    assert_eq!(item["kind"], "signal");
    assert_eq!(item["project_id"], PROJECT);
    assert!(item["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(item["at"].as_str().is_some_and(|at| !at.is_empty()));

    let second = peek(port, &world.agent_token, "");
    assert_eq!(second["items"], first["items"], "a peek moves nothing");

    let waited = agent.call("notify_wait", json!({"wait_seconds": 0}));
    assert_eq!(
        waited.pointer("/notifications/pending/0/title"),
        Some(&json!("one")),
        "the item a peek showed is still delivered: {waited}"
    );
    let third = peek(port, &world.agent_token, "");
    assert_eq!(third["count"], 0, "{third}");
    assert_eq!(third["items"], json!([]));
    drop(child);
}

#[tokio::test]
async fn a_held_peek_returns_when_a_newer_item_arrives_and_still_advances_nothing() {
    let world = world("me-pending-hold").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let other = Mcp::connect(port, &world.other_token);
    subscribe(&agent, "signal");
    signal(&other, "old");

    let seen = peek(port, &world.agent_token, "");
    let after = seen["cursor"].as_str().expect("cursor").to_string();
    assert!(!after.is_empty(), "{seen}");

    let token = world.agent_token.clone();
    let query = format!("?wait=20&after={after}");
    let held = std::thread::spawn(move || {
        let started = Instant::now();
        let peeked = peek(port, &token, &query);
        (started.elapsed(), peeked)
    });
    std::thread::sleep(Duration::from_millis(400));
    signal(&other, "new");
    let (elapsed, peeked) = held.join().expect("held peek");

    assert!(
        elapsed < Duration::from_secs(10),
        "the peek ended on the new item, not the wait: {elapsed:?}"
    );
    assert_eq!(
        titles(&peeked),
        vec!["new"],
        "the items are those newer than the cursor"
    );
    assert_eq!(peeked["count"], 1, "and the count is what is newer than it");
    assert!(
        peeked["cursor"].as_str().expect("cursor") > after.as_str(),
        "the cursor moved to the new item: {peeked}"
    );

    let waited = agent.call("notify_wait", json!({"wait_seconds": 0}));
    let delivered = waited
        .pointer("/notifications/pending")
        .and_then(Value::as_array)
        .expect("both items are still delivered");
    assert_eq!(delivered.len(), 2, "{waited}");
    drop(child);
}

#[tokio::test]
async fn a_held_peek_with_nothing_newer_answers_at_the_deadline() {
    let world = world("me-pending-timeout").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let other = Mcp::connect(port, &world.other_token);
    subscribe(&agent, "signal");
    signal(&other, "old");
    let seen = peek(port, &world.agent_token, "");
    let after = seen["cursor"].as_str().expect("cursor").to_string();

    let started = Instant::now();
    let peeked = peek(port, &world.agent_token, &format!("?wait=1&after={after}"));
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "nothing newer, so the peek waited: {:?}",
        started.elapsed()
    );
    assert!(titles(&peeked).is_empty(), "nothing is newer: {peeked}");
    assert_eq!(peeked["count"], 0, "nothing is newer than what was seen");
    assert_eq!(peeked["cursor"], after.as_str(), "{peeked}");
    drop(child);
}

#[tokio::test]
async fn a_peek_in_a_session_reads_that_sessions_items_and_no_other() {
    let world = world("me-pending-session").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let first = Mcp::connect(port, &world.agent_token).bind("first");
    let second = Mcp::connect(port, &world.agent_token).bind("second");
    let first_id = first.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "first"}),
    )["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let second_id = second.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "second"}),
    )["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let asked = first.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Ship it?", "body": "Ready."}),
    );
    answer(port, asked["question_id"].as_str().expect("question id"));

    let mine = peek(port, &world.agent_token, &format!("?session={first_id}"));
    assert_eq!(titles(&mine), vec!["answered: Ship it?"], "{mine}");
    assert_eq!(mine["items"][0]["source"], "attention");
    assert_eq!(mine["items"][0]["direct"], true);
    for other in [
        peek(port, &world.agent_token, &format!("?session={second_id}")),
        peek(port, &world.agent_token, ""),
    ] {
        assert_eq!(
            other["count"], 0,
            "the answer is not the other scope's: {other}"
        );
    }

    let unknown = rest(
        port,
        "GET",
        "/api/v1/me/pending?session=no-such-session",
        Some(&world.agent_token),
        None,
    );
    assert_eq!(unknown.status, 404, "{}", unknown.raw);
    assert_eq!(
        json_of(&unknown)["details"]["reason"],
        "session_unknown",
        "{}",
        unknown.raw
    );

    let delivered = second.call("notify_wait", json!({"wait_seconds": 0}));
    assert_eq!(
        delivered.pointer("/notifications/pending"),
        Some(&json!([])),
        "the other session's connection is not shown it: {delivered}"
    );
    let delivered = first.call("notify_wait", json!({"wait_seconds": 0}));
    assert_eq!(
        delivered.pointer("/notifications/pending/0/title"),
        Some(&json!("answered: Ship it?"))
    );
    let after = peek(port, &world.agent_token, &format!("?session={first_id}"));
    assert_eq!(after["count"], 0, "{after}");
    drop(child);
}

#[tokio::test]
async fn the_cursor_stops_at_the_last_item_returned_so_a_direct_one_behind_the_limit_is_next() {
    let world = world("me-pending-limit").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let other = Mcp::connect(port, &world.other_token);
    let asked = agent.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Which?", "body": "A or B."}),
    );
    subscribe(&agent, "signal");
    for n in 0..20 {
        signal(&other, &format!("ambient {n:02}"));
    }
    answer(port, asked["question_id"].as_str().expect("question id"));

    let first = peek(port, &world.agent_token, "?limit=20");
    assert_eq!(first["count"], 21, "{first}");
    let expected: Vec<String> = (0..20).map(|n| format!("ambient {n:02}")).collect();
    assert_eq!(titles(&first), expected, "oldest first: {first}");
    assert!(
        first["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["direct"] == false),
        "{first}"
    );

    let cursor = first["cursor"].as_str().expect("cursor").to_string();
    let next = peek(
        port,
        &world.agent_token,
        &format!("?limit=20&after={cursor}"),
    );
    assert_eq!(
        titles(&next),
        vec!["answered: Which?"],
        "the item behind the limit was not stepped over: {next}"
    );
    assert_eq!(next["items"][0]["direct"], true);
    drop(child);
}

#[tokio::test]
async fn a_peek_pages_through_a_backlog_of_answers_by_its_cursor() {
    let world = world("me-pending-backlog").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    // Posting rides the trailer, which would deliver earlier answers, so every
    // question is asked before the first is answered.
    let asked: Vec<String> = (0..60)
        .map(|n| {
            agent.call(
                "question_post",
                json!({"project_id": PROJECT, "subject": format!("Q{n:02}"), "body": "A or B."}),
            )["question_id"]
                .as_str()
                .expect("question id")
                .to_string()
        })
        .collect();
    for question_id in &asked {
        answer(port, question_id);
    }

    let first = peek(port, &world.agent_token, "?limit=100");
    let first_titles = titles(&first);
    assert_eq!(first_titles.len(), 50, "one page of the backlog: {first}");
    assert_eq!(first_titles[0], "answered: Q00");

    let cursor = first["cursor"].as_str().expect("cursor").to_string();
    let next = peek(
        port,
        &world.agent_token,
        &format!("?limit=100&after={cursor}"),
    );
    let expected: Vec<String> = (50..60).map(|n| format!("answered: Q{n:02}")).collect();
    assert_eq!(
        titles(&next),
        expected,
        "the rest follows the cursor: {next}"
    );
    drop(child);
}

#[tokio::test]
async fn an_ended_session_leaves_the_lease_reading_only_what_was_asked_outside_any_session() {
    let world = world("me-pending-ended-lease").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let session_id = agent.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "work"}),
    )["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let asked = agent.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Which?", "body": "A or B."}),
    );
    answer(port, asked["question_id"].as_str().expect("question id"));

    let ended = rest(
        port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/end"),
        Some(ADMIN_TOKEN),
        None,
    );
    assert_eq!(ended.status, 200, "{}", ended.raw);

    for _ in 0..2 {
        let after = agent.call("notify_wait", json!({"wait_seconds": 0}));
        assert_eq!(
            after["notifications"]["pending"],
            json!([]),
            "the ended session's answer is not delivered, once or again: {after}"
        );
    }
    drop(child);
}

#[tokio::test]
async fn a_session_ended_over_rest_takes_its_subscriptions_and_refuses_new_ones() {
    let world = world("me-pending-end").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let session_id = agent.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "work"}),
    )["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let bound = agent.call(
        "notify_subscribe",
        json!({"kinds": "signal", "project_id": PROJECT}),
    );
    assert_eq!(bound["session_id"], session_id.as_str(), "{bound}");

    let ended = rest(
        port,
        "POST",
        &format!("/api/v1/sessions/{session_id}/end"),
        Some(ADMIN_TOKEN),
        None,
    );
    assert_eq!(ended.status, 200, "{}", ended.raw);

    let listed = agent.call("notify_list", json!({}));
    assert_eq!(
        listed["subscriptions"],
        json!([]),
        "the subscription went with the session: {listed}"
    );
    // The connection still holds the lease the human ended under it.
    let refused = mcp_post(
        port,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"notify_subscribe","arguments":{"kinds":"signal"}}}).to_string(),
        Some(&agent.token),
        Some(&agent.session),
    )
    .message();
    assert_eq!(
        refused.pointer("/error/data/error/code"),
        Some(&json!("conflict")),
        "an ended session takes no subscription: {refused}"
    );
    drop(child);
}

#[tokio::test]
async fn direct_marks_what_names_the_caller_and_not_ambient_activity() {
    let world = world("me-pending-direct").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let other = Mcp::connect(port, &world.other_token);

    let artifact = agent.call(
        "artifact_publish",
        json!({"project_id": PROJECT, "title": "Plan", "kind": "html", "content": "<p>x</p>"}),
    )["artifact_id"]
        .as_str()
        .expect("artifact id")
        .to_string();
    let asked = agent.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Which?", "body": "A or B."}),
    );
    // Every tool result carries the trailer, so the agent subscribes after its
    // own calls and what follows is left for the peek.
    subscribe(&agent, "signal,comment");
    signal(&other, "ambient");
    let commented = rest(
        port,
        "POST",
        &format!("/api/v1/artifacts/{artifact}/comments"),
        Some(ADMIN_TOKEN),
        Some(&json!({"body": "Move this up"}).to_string()),
    );
    assert_eq!(commented.status, 200, "{}", commented.raw);
    answer(port, asked["question_id"].as_str().expect("question id"));

    let peeked = peek(port, &world.agent_token, "");
    let direct_of = |title: &str| {
        peeked["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["title"] == title)
            .unwrap_or_else(|| panic!("no item titled {title}: {peeked}"))["direct"]
            .clone()
    };
    assert_eq!(
        direct_of("answered: Which?"),
        true,
        "an answer on its own question"
    );
    assert_eq!(
        direct_of("comment on Plan"),
        true,
        "a comment on its own artifact"
    );
    assert_eq!(direct_of("ambient"), false, "an ambient signal");

    let waited = agent.call("notify_wait", json!({"wait_seconds": 0}));
    let delivered = waited
        .pointer("/notifications/pending")
        .and_then(Value::as_array)
        .expect("the trailer");
    let ambient = delivered
        .iter()
        .find(|item| item["title"] == "ambient")
        .expect("the signal");
    assert_eq!(ambient["direct"], false, "the trailer carries the flag too");
    assert!(
        delivered
            .iter()
            .filter(|item| item["title"] != "ambient")
            .all(|item| item["direct"] == true),
        "{waited}"
    );
    drop(child);
}

#[tokio::test]
async fn the_route_answers_active_members_only() {
    let world = world("me-pending-auth").await;
    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();

    let admin = rest(port, "GET", "/api/v1/me/pending", Some(ADMIN_TOKEN), None);
    assert_eq!(admin.status, 403, "{}", admin.raw);
    assert_eq!(json_of(&admin)["code"], "forbidden");

    let anonymous = rest(port, "GET", "/api/v1/me/pending", None, None);
    assert_eq!(anonymous.status, 401, "{}", anonymous.raw);
    assert_eq!(json_of(&anonymous)["code"], "unauthenticated");
    assert_eq!(json_of(&anonymous)["details"]["reason"], "token_missing");

    let enrolled = rest(
        port,
        "POST",
        "/api/v1/enrol",
        None,
        Some(r#"{"member":"newcomer","display_name":"Newcomer","why":"testing"}"#),
    );
    assert_eq!(enrolled.status, 202, "{}", enrolled.raw);
    let pending_token = json_of(&enrolled)["token"]
        .as_str()
        .expect("pending token")
        .to_string();
    let pending = rest(
        port,
        "GET",
        "/api/v1/me/pending",
        Some(&pending_token),
        None,
    );
    assert_eq!(pending.status, 401, "{}", pending.raw);
    assert_eq!(json_of(&pending)["details"]["reason"], "token_pending");

    let unknown = rest(port, "GET", "/api/v1/me/pending", Some("not-a-token"), None);
    assert_eq!(unknown.status, 401, "{}", unknown.raw);

    let bad = rest(
        port,
        "GET",
        "/api/v1/me/pending?wait=soon",
        Some(&world.agent_token),
        None,
    );
    assert_eq!(bad.status, 400, "{}", bad.raw);
    drop(child);
}

#[test]
fn a_loopback_hub_with_no_admin_token_answers_a_declared_agent() {
    let dir = TempDir::new("me-pending-loopback");
    common::seed::seed_project(dir.path(), PROJECT);
    let child = HubProcess::serve(&dir, "", &[]);
    let port = child.port();

    let peeked = send(
        port,
        "GET",
        "/api/v1/me/pending",
        &[("X-Agent-Id", "local-agent")],
        None,
    );
    assert_eq!(peeked.status, 200, "{}", peeked.raw);
    assert_eq!(json_of(&peeked)["count"], 0);
    drop(child);
}

#[tokio::test]
async fn a_peek_pages_a_migration_backlog_past_its_first_page_and_moves_nothing() {
    let world = world("me-pending-migrated-backlog").await;
    let mut child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let agent = Mcp::connect(port, &world.agent_token);
    let asked: Vec<String> = (0..60)
        .map(|n| {
            agent.call(
                "question_post",
                json!({"project_id": PROJECT, "subject": format!("Q{n:02}"), "body": "A or B."}),
            )["question_id"]
                .as_str()
                .expect("question id")
                .to_string()
        })
        .collect();
    for question_id in &asked {
        answer(port, question_id);
    }
    child.stop();

    // A migration leaves the cursor past rows it still owes the participant.
    let db = common::store::open(&world.dir).await;
    let conn = db.connect().expect("connect");
    conn.execute(
        "INSERT INTO notify_cursors(member_id, agent, session_id, last_seen_event_id, updated_at)
         VALUES ('walker', 'cli', '', 'zzzz', '2020-01-01T00:00:00Z')",
        (),
    )
    .await
    .expect("cursor");
    for question_id in &asked {
        conn.execute(
            "INSERT INTO notify_backlog(member_id, agent, session_id, event_id)
             VALUES ('walker', 'cli', '', ?1)",
            vec![turso::Value::Text(question_id.clone())],
        )
        .await
        .expect("backlog row");
    }
    drop(conn);
    drop(db);

    let child = HubProcess::serve(&world.dir, ADMIN_TOKEN, &[]);
    let port = child.port();
    let mut seen = Vec::new();
    let mut after = String::new();
    for _ in 0..6 {
        let query = if after.is_empty() {
            "?limit=100".to_string()
        } else {
            format!("?limit=100&after={after}")
        };
        let page = peek(port, &world.agent_token, &query);
        let got = titles(&page);
        if got.is_empty() {
            break;
        }
        assert!(
            got.len() <= 50,
            "a page is at most one backlog page: {page}"
        );
        after = page["cursor"].as_str().expect("cursor").to_string();
        seen.extend(got);
    }
    let expected: Vec<String> = (0..60).map(|n| format!("answered: Q{n:02}")).collect();
    assert_eq!(seen, expected, "every backlog item once, in order");

    let again = peek(port, &world.agent_token, "?limit=100");
    assert_eq!(
        titles(&again),
        expected[..50].to_vec(),
        "a peek with no after still starts at the first page: {again}"
    );
    drop(child);
}
