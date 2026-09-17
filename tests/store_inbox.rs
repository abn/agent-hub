//! Inbox and question tests: projection, threads, resolution, and counts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::store::events::{self, FeedQuery, NewEvent, append, read_feed};
use agent_hub::store::questions::NewQuestion;
use agent_hub::store::{home, inbox, migrate, open_engine, questions};

static NEXT_DB: AtomicU64 = AtomicU64::new(0);

fn temp_db(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DB.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-{tag}-{}-{nanos}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("hub.db")
}

async fn open() -> turso::Database {
    let db = open_engine(&temp_db("inbox")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    db
}

fn question(subject: &str) -> NewQuestion<'_> {
    NewQuestion {
        actor: "agent-one",
        project_id: "proj",
        subject,
        body: None,
        context: None,
        to: None,
        idempotency_key: None,
    }
}

fn finished(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "finished".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
    }
}

fn approval(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "approval".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
    }
}

#[tokio::test]
async fn an_approval_is_decided_and_leaves_the_waiting_queue() {
    let db = open().await;
    let approval_id = events::append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    // An approval needs the human, so it enters the inbox as an action.
    let event = events::get(&db, &approval_id)
        .await
        .expect("get")
        .expect("exists");
    assert!(event.needs_action, "an approval waits on the human");
    assert_eq!(
        event.inbox_status.as_deref(),
        Some("action"),
        "the approval is open in the inbox"
    );
    let waiting = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("list");
    assert_eq!(waiting.len(), 1, "the approval is an action item");

    let decision = questions::decide(&db, "human", &approval_id, true, Some("ship it"))
        .await
        .expect("decide");
    let decision_event = events::get(&db, &decision)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        decision_event.kind, "answer",
        "the decision is a feed reply"
    );
    assert_eq!(
        decision_event.thread_id.as_deref(),
        Some(approval_id.as_str()),
        "the decision is on the approval's thread"
    );
    let payload = decision_event.payload.expect("decision payload");
    assert_eq!(payload["body"], "Approved: ship it");
    assert_eq!(payload["decision"], "approved");

    let resolved = events::get(&db, &approval_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        resolved.inbox_status.as_deref(),
        Some("resolved"),
        "the feed carries the resolved status"
    );
    let counts = inbox::counts(&db).await.expect("counts");
    assert_eq!(counts.waiting, 0, "the decision clears the waiting item");

    let bad = questions::decide(&db, "human", "missing", true, None)
        .await
        .expect_err("unknown approval");
    assert_eq!(bad.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn an_approval_is_decided_once() {
    let db = open().await;
    let approval_id = events::append(&db, "agent-one", None, approval("Deploy 0.4.2"))
        .await
        .expect("append approval");

    questions::decide(&db, "human", &approval_id, true, None)
        .await
        .expect("first decision");

    let again = questions::decide(&db, "human", &approval_id, false, None)
        .await
        .expect_err("a second decision conflicts");
    assert_eq!(again.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn a_decision_note_is_trimmed_and_a_decline_reads_plainly() {
    let db = open().await;

    let padded = events::append(&db, "agent-one", None, approval("Ship it"))
        .await
        .expect("append approval");
    let decision = questions::decide(&db, "human", &padded, true, Some("  go ahead  "))
        .await
        .expect("decide");
    let payload = events::get(&db, &decision)
        .await
        .expect("get")
        .expect("exists")
        .payload
        .expect("payload");
    assert_eq!(payload["body"], "Approved: go ahead", "the note is trimmed");

    let blank = events::append(&db, "agent-one", None, approval("Roll it back"))
        .await
        .expect("append approval");
    let decision = questions::decide(&db, "human", &blank, false, Some("   "))
        .await
        .expect("decide");
    let payload = events::get(&db, &decision)
        .await
        .expect("get")
        .expect("exists")
        .payload
        .expect("payload");
    assert_eq!(payload["body"], "Declined", "a blank note adds nothing");
    assert_eq!(payload["decision"], "declined");
}

#[tokio::test]
async fn a_non_approval_cannot_be_decided() {
    let db = open().await;
    let signal = events::append(&db, "agent-one", None, finished("done"))
        .await
        .expect("append");

    let err = questions::decide(&db, "human", &signal, true, None)
        .await
        .expect_err("not an approval");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn question_opens_a_thread_and_lands_in_the_inbox() {
    let db = open().await;
    let mut ask = question("Deploy tonight?");
    ask.body = Some("The release is ready.");
    ask.to = Some("human");
    let question_id = questions::post(&db, ask).await.expect("post question");

    let event = agent_hub::store::events::get(&db, &question_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(event.kind, "question");
    assert_eq!(
        event.thread_id.as_deref(),
        Some(question_id.as_str()),
        "the question roots its own thread"
    );

    let items = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("inbox");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].event_id, question_id);
    assert_eq!(items[0].status, "action");
}

#[tokio::test]
async fn answer_closes_the_thread_and_resolves_the_item() {
    let db = open().await;
    let question_id = questions::post(&db, question("Ship it?"))
        .await
        .expect("post");

    let answer_id = questions::answer(&db, "human", &question_id, "Yes, ship it", None)
        .await
        .expect("answer");

    let answer = agent_hub::store::events::get(&db, &answer_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(answer.kind, "answer");
    assert_eq!(answer.thread_id.as_deref(), Some(question_id.as_str()));

    let items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert_eq!(
        items.len(),
        1,
        "the answer does not itself become an inbox item"
    );
    assert_eq!(items[0].kind, "question");
    assert_eq!(
        items[0].status, "resolved",
        "the question is resolved by the answer"
    );

    let counts = inbox::counts(&db).await.expect("counts");
    assert_eq!(counts.waiting, 0);
}

#[tokio::test]
async fn finished_work_lands_as_unread_and_counts() {
    let db = open().await;
    append(&db, "agent-one", None, finished("nightly report done"))
        .await
        .expect("append");

    let items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, "unread");

    let summary = home::home(&db, 10).await.expect("home");
    assert_eq!(summary.unread, 1);
    assert_eq!(summary.waiting, 0);
    assert_eq!(summary.recent.len(), 1);
}

#[tokio::test]
async fn signals_do_not_land_in_the_inbox() {
    let db = open().await;
    append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "just a note".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
        },
    )
    .await
    .expect("append");

    let items = inbox::list(&db, None, None, 50).await.expect("inbox");
    assert!(items.is_empty(), "a signal is not an inbox item");
}

#[tokio::test]
async fn answering_a_non_question_is_rejected() {
    let db = open().await;
    let id = append(&db, "agent-one", None, finished("not a question"))
        .await
        .expect("append");
    let err = questions::answer(&db, "human", &id, "nope", None)
        .await
        .expect_err("reject");
    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn feed_still_reads_the_thread() {
    let db = open().await;
    let question_id = questions::post(&db, question("Anyone there?"))
        .await
        .expect("post");
    questions::answer(&db, "human", &question_id, "here", None)
        .await
        .expect("answer");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn question_and_answer_honour_idempotency_keys() {
    let db = open().await;

    let mut first = question("repeatable?");
    first.idempotency_key = Some("q1");
    let q1 = questions::post(&db, first).await.expect("post");
    let mut again = question("repeatable?");
    again.idempotency_key = Some("q1");
    let q2 = questions::post(&db, again).await.expect("post again");
    assert_eq!(q1, q2, "a repeated question key returns the same event");

    let a1 = questions::answer(&db, "human", &q1, "yes", Some("a1"))
        .await
        .expect("answer");
    let a2 = questions::answer(&db, "human", &q1, "yes", Some("a1"))
        .await
        .expect("answer again");
    assert_eq!(a1, a2, "a repeated answer key returns the same event");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert_eq!(events.len(), 2, "one question and one answer were stored");
}

#[tokio::test]
async fn signal_append_question_still_lands_in_the_inbox() {
    let db = open().await;
    let id = append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "question".to_string(),
            summary: "written through the generic writer".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
        },
    )
    .await
    .expect("append question");

    let event = agent_hub::store::events::get(&db, &id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(event.thread_id.as_deref(), Some(id.as_str()));
    assert!(event.needs_action);

    let items = inbox::list(&db, Some("action"), None, 50)
        .await
        .expect("inbox");
    assert_eq!(
        items.len(),
        1,
        "a question written directly still needs action"
    );
}

#[tokio::test]
async fn an_orphan_answer_is_rejected() {
    let db = open().await;
    let err = append(
        &db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "answer".to_string(),
            summary: "no question".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
        },
    )
    .await
    .expect_err("reject orphan answer");
    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}
