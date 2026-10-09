//! A concurrent writer waits for its turn at the write lock instead of
//! surfacing Busy.
//!
//! Every hub store write queues for the lock in arrival order, so these pin
//! what that gives a caller: many same-key writers serialise to a single
//! result, a steady stream of writers starves none of them, and none reports
//! "database is locked".

use std::sync::Arc;

use agent_hub::error::Error;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::events::{self, FeedQuery};
use agent_hub::store::questions::{self, NewQuestion};

use agent_hub::store::projects;

mod common;

use common::store::open;
use common::temp::TempDir;

async fn ensure_project(db: &turso::Database) {
    let _ = projects::create(db, "proj", "Project").await;
}

/// More writers than worker threads, so the lock is contended for real.
const WRITERS: usize = 8;

/// A writer that lost the lock must wait, not fail. Any other error is a real
/// failure and is left to the caller.
fn assert_not_locked<T>(result: &Result<T, Error>) {
    if let Err(err) = result {
        assert!(
            !err.to_string().contains("locked"),
            "a concurrent writer surfaced the lock instead of waiting: {err}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_same_key_publishes_serialize() {
    let dir = TempDir::new("lock-artifact");
    let db = Arc::new(open(&dir).await);
    ensure_project(&db).await;

    let mut handles = Vec::new();
    for _ in 0..WRITERS {
        let db = db.clone();
        let dir = dir.to_path_buf();
        handles.push(tokio::spawn(async move {
            artifacts::publish(
                &db,
                &dir,
                NewArtifact {
                    actor: "agent-one",
                    project_id: "proj",
                    title: "Report",
                    kind: "html",
                    content: b"body",
                    envelope: None,
                    description: "",
                    label: None,
                    session_id: None,
                },
                Some("pub-key"),
            )
            .await
        }));
    }

    let mut ids = Vec::new();
    for handle in handles {
        let result = handle.await.expect("join");
        assert_not_locked(&result);
        ids.push(result.expect("publish").id);
    }
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "every same-key writer returns one artifact id: {ids:?}"
    );

    let listed = artifacts::list(&db, "proj").await.expect("list");
    assert_eq!(listed.len(), 1, "the key produces one artifact");

    let events = events::read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let published = events
        .iter()
        .filter(|event| event.kind == "artifact" && event.summary.contains("published"))
        .count();
    assert_eq!(published, 1, "the key produces one event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_same_key_questions_serialize() {
    let dir = TempDir::new("lock-question");
    let db = Arc::new(open(&dir).await);
    ensure_project(&db).await;

    let mut handles = Vec::new();
    for _ in 0..WRITERS {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            questions::post(
                &db,
                &agent_hub::limits::InboxCaps::disabled(),
                0,
                NewQuestion {
                    actor: "agent-one",
                    project_id: "proj",
                    subject: "Ship it?",
                    body: None,
                    context: None,
                    options: None,
                    idempotency_key: Some("q-key"),
                    session_id: None,
                    deadline: None,
                },
            )
            .await
        }));
    }

    let mut ids = Vec::new();
    for handle in handles {
        let result = handle.await.expect("join");
        assert_not_locked(&result);
        ids.push(result.expect("post"));
    }
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "every same-key writer returns one event id: {ids:?}"
    );

    let events = events::read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let questions = events
        .iter()
        .filter(|event| event.kind == "question")
        .count();
    assert_eq!(questions, 1, "the key produces one question");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_same_key_answers_serialize() {
    let dir = TempDir::new("lock-answer");
    let db = Arc::new(open(&dir).await);
    ensure_project(&db).await;
    let question_id = questions::post(
        &db,
        &agent_hub::limits::InboxCaps::disabled(),
        0,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject: "Ship it?",
            body: None,
            context: None,
            options: None,
            idempotency_key: None,
            session_id: None,
            deadline: None,
        },
    )
    .await
    .expect("post");

    let mut handles = Vec::new();
    for _ in 0..WRITERS {
        let db = db.clone();
        let question_id = question_id.clone();
        handles.push(tokio::spawn(async move {
            questions::answer(&db, 0, "agent-two", &question_id, "yes", Some("a-key")).await
        }));
    }

    let mut ids = Vec::new();
    for handle in handles {
        let result = handle.await.expect("join");
        assert_not_locked(&result);
        ids.push(result.expect("answer"));
    }
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "every same-key writer returns one answer id: {ids:?}"
    );

    let events = events::read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let answers = events
        .iter()
        .filter(|event| event.kind == "answer" && event.thread_id.as_deref() == Some(&question_id))
        .count();
    assert_eq!(answers, 1, "the key produces one answer");
}

/// Before writers queued, the engine's busy handler let a waiter miss every
/// free moment behind writers that commit and begin again back to back, and
/// this failed in most runs on an idle machine.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_steady_stream_of_appends_starves_no_writer() {
    // Writers that commit and begin again back to back keep the write lock
    // taken almost all the time. Each must still get its turn rather than
    // miss every free moment for the whole lock wait, and so must a
    // single-statement write that runs outside a transaction.
    const STREAMS: usize = 12;
    const PER_STREAM: usize = 10;
    const MARKERS: usize = 3;

    let dir = TempDir::new("lock-stream");
    let db = Arc::new(open(&dir).await);
    ensure_project(&db).await;

    let mut handles = Vec::new();
    for stream in 0..STREAMS {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            for index in 0..PER_STREAM {
                let result = events::append(
                    &db,
                    0,
                    "agent-one",
                    None,
                    events::NewEvent {
                        project_id: "proj".to_string(),
                        kind: "signal".to_string(),
                        summary: format!("stream {stream} event {index}"),
                        payload: None,
                        needs_action: false,
                        thread_id: None,
                        session_id: None,
                    },
                )
                .await;
                assert_not_locked(&result);
                result.expect("append");
            }
        }));
    }
    for _ in 0..MARKERS {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..PER_STREAM {
                let result = agent_hub::store::inbox::mark_all_read(&db, Some("proj")).await;
                assert_not_locked(&result);
                result.expect("mark all read");
            }
        }));
    }
    for handle in handles {
        handle.await.expect("join");
    }

    let feed = events::read_feed(
        &db,
        "proj",
        &FeedQuery {
            limit: agent_hub::limits::FEED_LIMIT_MAX,
            ..Default::default()
        },
    )
    .await
    .expect("feed");
    assert_eq!(feed.events.len(), STREAMS * PER_STREAM);
}
