//! A concurrent writer waits for the engine lock instead of surfacing Busy.
//!
//! The engine's busy handler is per-connection, so this pins the behaviour the
//! store's connection helper adds: many same-key writers serialise to a single
//! result and none reports "database is locked".

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
                    idempotency_key: Some("q-key"),
                    session_id: None,
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
            idempotency_key: None,
            session_id: None,
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
