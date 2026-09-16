//! Inbox and question tests: projection, threads, resolution, and counts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::store::events::{FeedQuery, NewEvent, append, read_feed};
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
