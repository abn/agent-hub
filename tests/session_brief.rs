//! The session brief over MCP, with real agents against one hub.
//!
//! Each test drives the hub as authenticated agents and the human's REST
//! surface, so the brief is checked against the same records the underlying
//! tools read: the feed, the inbox, the sessions and the knowledge base.

use agent_hub::store::events::{self, BriefFilter, NewEvent};
use agent_hub::store::{identity, projects};
use serde_json::{Value, json};

mod common;

use common::process::HubProcess;
use common::stdio::PROTOCOL_VERSION;
use common::temp::TempDir;
use common::wire::{mcp_post, rest};

const ADMIN_TOKEN: &str = "session-brief-admin";
const PROJECT: &str = "homelab";

/// One agent's authenticated MCP connection.
struct Agent {
    port: u16,
    token: String,
    session: String,
}

impl Agent {
    /// The full JSON-RPC reply to one tool call.
    fn raw(&self, name: &str, arguments: Value) -> Value {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string();
        mcp_post(self.port, &body, Some(&self.token), Some(&self.session)).message()
    }

    /// Call a tool and return its structured result, failing on an error.
    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = self.raw(name, arguments);
        assert!(
            !response
                .pointer("/result/isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "{name} failed: {response}"
        );
        response
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{name} returned no structured content: {response}"))
    }

    fn brief(&self) -> Value {
        self.call("session_brief", json!({"project_id": PROJECT}))
    }

    fn signal(&self, summary: &str) -> String {
        self.call(
            "signal_append",
            json!({"project_id": PROJECT, "kind": "signal", "summary": summary}),
        )["event_id"]
            .as_str()
            .expect("an event id")
            .to_string()
    }
}

fn connect(port: u16, token: &str) -> Agent {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "session-brief", "version": "0.0.0"},
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
    Agent {
        port,
        token: token.to_string(),
        session,
    }
}

/// A hub with one project and the named agents, each with a token.
struct Fleet {
    _child: HubProcess,
    _dir: TempDir,
    port: u16,
    tokens: Vec<String>,
}

impl Fleet {
    async fn new(tag: &str, agents: &[&str], confidential: bool) -> Self {
        Self::build(tag, agents, confidential, 0).await
    }

    /// A fleet whose project already holds `events` signals, written in one
    /// transaction before the hub starts.
    async fn seeded(tag: &str, agents: &[&str], events: usize) -> Self {
        Self::build(tag, agents, false, events).await
    }

    async fn build(tag: &str, agents: &[&str], confidential: bool, events: usize) -> Self {
        let dir = TempDir::new(tag);
        let db = common::store::open(&dir).await;
        projects::create_with_confidential(&db, PROJECT, "Homelab", confidential)
            .await
            .expect("create project");
        if events > 0 {
            let mut conn = db.connect().expect("connect");
            let tx = conn.transaction().await.expect("begin");
            for n in 0..events {
                tx.execute(
                    "INSERT INTO events(id, project_id, kind, actor, summary, created_at)
                     VALUES (?1, ?2, 'signal', 'seed', ?3, '2026-01-01T00:00:00Z')",
                    vec![
                        turso::Value::Text(format!("01SEED{n:020}")),
                        turso::Value::Text(PROJECT.to_string()),
                        turso::Value::Text(format!("seed {n}")),
                    ],
                )
                .await
                .expect("seed an event");
            }
            tx.commit().await.expect("commit");
        }
        let mut tokens = Vec::new();
        for (index, agent) in agents.iter().enumerate() {
            identity::create_agent(&db, agent, agent)
                .await
                .expect("create agent");
            if confidential && index == 0 {
                identity::add_grant(&db, agent, PROJECT)
                    .await
                    .expect("add grant");
            }
            tokens.push(
                identity::issue_token(&db, agent)
                    .await
                    .expect("issue token")
                    .token,
            );
        }
        drop(db);
        let child = HubProcess::serve(&dir, ADMIN_TOKEN, &[]);
        let port = child.port();
        Self {
            _child: child,
            _dir: dir,
            port,
            tokens,
        }
    }

    /// A new connection for the agent at `index`.
    fn agent(&self, index: usize) -> Agent {
        connect(self.port, &self.tokens[index])
    }

    fn admin(&self, method: &str, path: &str, body: Value) {
        let response = rest(
            self.port,
            method,
            path,
            Some(ADMIN_TOKEN),
            Some(&body.to_string()),
        );
        assert_eq!(response.status, 200, "{path}: {}", response.body());
    }
}

fn items(section: &Value) -> &Vec<Value> {
    section["items"].as_array().expect("a section has items")
}

fn ids(section: &Value) -> Vec<String> {
    items(section)
        .iter()
        .map(|item| item["id"].as_str().expect("an id").to_string())
        .collect()
}

fn pending(result: &Value) -> Vec<Value> {
    result
        .pointer("/notifications/pending")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn an_agent_with_no_prior_session_gets_the_newest_events_and_nothing_else() {
    let fleet = Fleet::new("brief-first", &["newcomer", "other"], false).await;
    let other = fleet.agent(1);
    for n in 0..3 {
        other.signal(&format!("deployed build {n}"));
    }

    let brief = fleet.agent(0).brief();
    assert_eq!(brief["agent"], "newcomer");
    assert_eq!(brief["since"]["basis"], "none", "{brief}");
    assert!(brief["since"]["event_id"].is_null(), "{brief}");
    assert!(brief["previous_session"].is_null(), "{brief}");
    let events = &brief["events"];
    let summaries: Vec<&str> = items(events)
        .iter()
        .map(|event| event["summary"].as_str().expect("a summary"))
        .collect();
    assert_eq!(
        summaries,
        ["deployed build 2", "deployed build 1", "deployed build 0"],
        "newest first: {brief}"
    );
    assert_eq!(events["more"], 0);
    assert!(items(&brief["answers"]).is_empty(), "{brief}");
    assert!(items(&brief["stale_pages"]).is_empty(), "{brief}");
}

#[tokio::test]
async fn events_since_the_cursor_are_ranked_capped_and_leave_the_cursor_alone() {
    let fleet = Fleet::new("brief-events", &["reader", "writer"], false).await;
    let reader = fleet.agent(0);
    let writer = fleet.agent(1);
    let seen = writer.signal("before the reader looked");
    let page = reader.call("feed_read", json!({"project_id": PROJECT}));
    assert_eq!(page["next_since"], seen);

    let approval = writer.call(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "approval", "summary": "Restart the cache"}),
    )["event_id"]
        .as_str()
        .expect("an approval id")
        .to_string();
    for n in 0..25 {
        writer.signal(&format!("step {n}"));
    }
    let long = "x".repeat(300);
    let clipped = writer.signal(&long);
    let own = reader.signal("my own note");

    let brief = reader.brief();
    assert_eq!(brief["since"]["basis"], "feed_cursor", "{brief}");
    assert_eq!(brief["since"]["event_id"], seen);
    let events = &brief["events"];
    let shown = ids(events);
    assert_eq!(shown.len(), 20, "the section is capped: {brief}");
    assert!(!shown.contains(&seen), "nothing at or below the cursor");
    assert_eq!(shown[0], approval, "an open item ranks first: {brief}");
    assert_eq!(items(events)[0]["open"], true);
    assert_eq!(shown[1], clipped, "then the newest other work");
    assert!(
        !shown.contains(&own),
        "the reader's own note ranks below other work: {brief}"
    );
    // The approval, 26 signals and the reader's note are above the cursor.
    assert_eq!(events["more"], 28 - 20);
    let first_clipped = &items(events)[1];
    assert_eq!(first_clipped["truncated"], true);
    assert_eq!(
        first_clipped["summary"].as_str().expect("a summary").len(),
        200
    );

    // The brief read nothing on the cursor's behalf.
    let after = reader.call("feed_read", json!({"project_id": PROJECT, "limit": 1}));
    assert_eq!(after["events"][0]["id"], approval, "{after}");
}

#[tokio::test]
async fn the_readers_own_open_item_ranks_first_and_says_it_is_open() {
    let fleet = Fleet::new("brief-own-open", &["reader", "writer"], false).await;
    let reader = fleet.agent(0);
    let writer = fleet.agent(1);
    let question = reader.call(
        "question_post",
        json!({"project_id": PROJECT, "subject": "Which pool?"}),
    )["question_id"]
        .as_str()
        .expect("a question id")
        .to_string();
    let others = writer.signal("others' work");
    let own = reader.signal("my own note");

    let brief = reader.brief();
    let events = &brief["events"];
    assert_eq!(ids(events), [question.clone(), others, own], "{brief}");
    assert_eq!(items(events)[0]["open"], true, "{brief}");
    assert!(items(events)[1].get("open").is_none(), "{brief}");
    assert!(items(events)[2].get("open").is_none(), "{brief}");
}

#[tokio::test]
async fn the_count_of_the_rest_stops_at_its_cap() {
    let db = common::store::fresh("brief-count-cap").await;
    projects::create(&db, PROJECT, "Homelab")
        .await
        .expect("create project");
    for n in 0..6 {
        events::append(
            &db,
            0,
            "writer",
            None,
            NewEvent {
                project_id: PROJECT.to_string(),
                kind: "signal".to_string(),
                summary: format!("step {n}"),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append");
    }

    let page = events::brief_events(&db, PROJECT, None, BriefFilter::All, 2)
        .await
        .expect("brief events");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].summary, "step 5", "newest first");
    let total = events::brief_count(&db, PROJECT, None, BriefFilter::All, 4)
        .await
        .expect("brief count");
    assert_eq!(total, 4, "the count stops at its cap");
    let total = events::brief_count(&db, PROJECT, None, BriefFilter::All, 100)
        .await
        .expect("brief count");
    assert_eq!(total, 6, "under the cap the count is exact");
}

#[tokio::test]
async fn answers_and_decisions_on_own_items_are_carried_once_and_capped() {
    let fleet = Fleet::new("brief-answers", &["asker"], false).await;
    let asker = fleet.agent(0);
    let mut questions = Vec::new();
    for n in 0..12 {
        let posted = asker.call(
            "question_post",
            json!({"project_id": PROJECT, "subject": format!("Which disk {n}?")}),
        );
        questions.push(
            posted["question_id"]
                .as_str()
                .expect("a question id")
                .to_string(),
        );
    }
    let approval = asker.call(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "approval", "summary": "Wipe the old pool"}),
    )["event_id"]
        .as_str()
        .expect("an approval id")
        .to_string();
    for (n, question) in questions.iter().enumerate() {
        fleet.admin(
            "POST",
            &format!("/api/v1/questions/{question}/answer"),
            json!({"body": format!("disk {n}")}),
        );
    }
    fleet.admin(
        "POST",
        &format!("/api/v1/approvals/{approval}/decision"),
        json!({"decision": "decline", "note": "keep it a week"}),
    );

    let brief = asker.brief();
    let answers = &brief["answers"];
    let shown = ids(answers);
    assert_eq!(shown.len(), 10, "{brief}");
    assert_eq!(answers["more"], 3);
    assert_eq!(shown[0], approval, "the newest decision first: {brief}");
    assert_eq!(items(answers)[0]["outcome"], "declined");
    assert_eq!(items(answers)[0]["text"], "keep it a week");
    assert_eq!(items(answers)[1]["outcome"], "answered");
    assert_eq!(items(answers)[1]["text"], "disk 11");
    // Events leave out exactly the answers the section lists.
    let listed: Vec<&Value> = items(answers)
        .iter()
        .map(|item| &item["answer_id"])
        .collect();
    let answer_events: Vec<&Value> = items(&brief["events"])
        .iter()
        .filter(|event| event["kind"] == "answer")
        .map(|event| &event["id"])
        .collect();
    assert_eq!(answer_events.len(), 3, "{brief}");
    assert!(
        answer_events.iter().all(|id| !listed.contains(id)),
        "{brief}"
    );
    assert!(answers.get("more_capped").is_none(), "{brief}");

    // The trailer delivers the three the brief left out, and only those.
    let nudged: Vec<Value> = pending(&brief)
        .into_iter()
        .filter(|item| item["source"] == "attention")
        .map(|item| item["id"].clone())
        .collect();
    assert_eq!(nudged.len(), 3, "{brief}");
    assert!(nudged.iter().all(|id| !shown.iter().any(|s| id == s)));

    // The window has not moved, so the brief still carries them, and the
    // trailer has nothing left to deliver.
    let again = asker.brief();
    assert_eq!(ids(&again["answers"]), shown, "{again}");
    assert!(pending(&again).is_empty(), "{again}");
}

#[tokio::test]
async fn an_answer_already_nudged_is_still_in_the_brief_newest_answer_first() {
    let fleet = Fleet::new("brief-nudged", &["asker"], false).await;
    let asker = fleet.agent(0);
    let ask = |subject: &str| {
        asker.call(
            "question_post",
            json!({"project_id": PROJECT, "subject": subject}),
        )["question_id"]
            .as_str()
            .expect("a question id")
            .to_string()
    };
    let first = ask("Which disk?");
    let second = ask("Which pool?");
    fleet.admin(
        "POST",
        &format!("/api/v1/questions/{second}/answer"),
        json!({"body": "pool b"}),
    );
    fleet.admin(
        "POST",
        &format!("/api/v1/questions/{first}/answer"),
        json!({"body": "disk a"}),
    );

    let who = asker.call("whoami", json!({}));
    assert_eq!(pending(&who).len(), 2, "the trailer nudged first: {who}");

    let brief = asker.brief();
    assert_eq!(brief["since"]["basis"], "none", "{brief}");
    let answers = &brief["answers"];
    assert_eq!(
        ids(answers),
        [first, second],
        "the newest answer first, not the newest question: {brief}"
    );
    assert_eq!(items(answers)[0]["text"], "disk a");
    assert_eq!(items(answers)[1]["text"], "pool b");
}

#[tokio::test]
async fn an_open_item_older_than_the_scan_still_ranks_first() {
    let fleet = Fleet::new("brief-old-open", &["reader", "writer"], false).await;
    let writer = fleet.agent(1);
    let approval = writer.call(
        "signal_append",
        json!({"project_id": PROJECT, "kind": "approval", "summary": "Rotate the keys"}),
    )["event_id"]
        .as_str()
        .expect("an approval id")
        .to_string();
    for n in 0..205 {
        writer.signal(&format!("step {n}"));
    }

    let events = &fleet.agent(0).brief()["events"];
    let shown = ids(events);
    assert_eq!(shown.len(), 20, "{events}");
    assert_eq!(shown[0], approval, "{events}");
    assert_eq!(items(events)[0]["open"], true);
    assert_eq!(
        shown.iter().filter(|id| **id == approval).count(),
        1,
        "no duplicate"
    );
    assert_eq!(events["more"], 206 - 20);
}

#[tokio::test]
async fn a_count_that_reaches_its_cap_says_so() {
    let fleet = Fleet::seeded("brief-capped", &["reader"], 10_001).await;
    let events = &fleet.agent(0).brief()["events"];
    assert_eq!(items(events).len(), 20, "{events}");
    assert_eq!(events["more"], 10_000 - 20);
    assert_eq!(events["more_capped"], true, "{events}");
}

#[tokio::test]
async fn the_previous_session_brings_its_handoff_and_bounds_the_events() {
    let fleet = Fleet::new("brief-handoff", &["worker", "other"], false).await;
    let other = fleet.agent(1);
    let worker = fleet.agent(0);
    let started = worker.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "nightly"}),
    );
    let nightly = started["session_id"].as_str().expect("an id").to_string();
    let before = other.signal("before the session ended");
    std::thread::sleep(std::time::Duration::from_millis(5));
    worker.call(
        "session_end",
        json!({"session_id": nightly, "handoff": "see RECOVERY.md; the pool is half migrated"}),
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    let after = other.signal("after the session ended");

    // Before a session starts, on a fresh connection.
    let brief = fleet.agent(0).brief();
    let previous = &brief["previous_session"];
    assert_eq!(previous["session_id"], nightly, "{brief}");
    assert_eq!(previous["session_name"], "nightly");
    assert_eq!(previous["status"], "ended");
    assert_eq!(
        previous["handoff"],
        "see RECOVERY.md; the pool is half migrated"
    );
    assert_eq!(previous["recovery_path"], "/fs/RECOVERY.md");
    assert_eq!(brief["since"]["basis"], "previous_session", "{brief}");
    let shown = ids(&brief["events"]);
    assert!(shown.contains(&after), "{brief}");
    assert!(!shown.contains(&before), "{brief}");

    // After starting a new session, the brief still names the one before it.
    let next = fleet.agent(0);
    next.call(
        "session_start",
        json!({"project_id": PROJECT, "session_name": "daytime"}),
    );
    assert_eq!(next.brief()["previous_session"]["session_id"], nightly);
}

#[tokio::test]
async fn knowledge_base_pages_past_stale_after_are_named_oldest_first() {
    let fleet = Fleet::new("brief-stale", &["keeper"], false).await;
    let keeper = fleet.agent(0);
    let page = |path: &str, stale_after: &str| {
        keeper.call(
            "brain_put",
            json!({
                "path": path,
                "store": "project",
                "project_id": PROJECT,
                "content": format!(
                    "---\ntype: Concept\ntitle: {path}\ndescription: A page.\nstale_after: {stale_after}\n---\n\n# Page\n"
                ),
            }),
        );
    };
    page("/fs/fresh.md", "2999-01-01");
    for n in 0..11 {
        page(&format!("/fs/old-{n:02}.md"), "2020-06-01");
    }
    page("/fs/oldest.md", "2019-01-01");

    let stale = &keeper.brief()["stale_pages"];
    let paths: Vec<&str> = items(stale)
        .iter()
        .map(|page| page["path"].as_str().expect("a path"))
        .collect();
    assert_eq!(paths.len(), 10, "{stale}");
    assert_eq!(stale["more"], 2);
    assert_eq!(paths[0], "/fs/oldest.md", "the longest overdue first");
    assert_eq!(items(stale)[0]["stale_after"], "2019-01-01");
    assert_eq!(paths[1], "/fs/old-00.md");
    assert!(!paths.contains(&"/fs/fresh.md"));
}

#[tokio::test]
async fn a_confidential_project_needs_a_grant() {
    let fleet = Fleet::new("brief-grant", &["granted", "outsider"], true).await;
    let granted = fleet.agent(0).brief();
    assert_eq!(granted["project_id"], PROJECT);

    let refused = fleet
        .agent(1)
        .raw("session_brief", json!({"project_id": PROJECT}));
    let code = refused
        .pointer("/error/data/error/code")
        .and_then(Value::as_str);
    assert_eq!(code, Some("forbidden"), "{refused}");
}
