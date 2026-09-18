//! Prune tests: soft delete, undo within the window, and commit after it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::brain::BrainStore;
use agent_hub::store::events::{self, FeedQuery, NewEvent, read_feed};
use agent_hub::store::{migrate, open_engine, prune, sessions};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-{tag}-{}-{nanos}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn open(dir: &std::path::Path) -> turso::Database {
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    db
}

#[tokio::test]
async fn prune_hides_a_session_and_undo_restores_it() {
    let dir = temp_dir("prune");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one")
        .await
        .expect("end");

    let token = prune::prune_session(&db, &session.id).await.expect("prune");
    assert_eq!(token.undo_token, session.id);

    // Hidden from the list, but the row is still there with a timestamp.
    assert!(sessions::list(&db, "proj").await.expect("list").is_empty());
    let soft = sessions::get(&db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert!(soft.deleted_at.is_some());

    prune::undo(&db, &token.undo_token).await.expect("undo");
    let restored = sessions::list(&db, "proj").await.expect("list");
    assert_eq!(restored.len(), 1, "undo restores the session");
}

#[tokio::test]
async fn sweep_commits_an_expired_prune() {
    let dir = temp_dir("prune-sweep");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    // Give the session a brain file so the commit has something to remove.
    let store = BrainStore::new(dir.join("sessions"));
    let brain = store.open("proj", &session.id).await.expect("open brain");
    brain.put("/kv/note", b"to be pruned").await.expect("put");
    let brain_file = dir.join(&session.brain_path);
    assert!(brain_file.exists());

    sessions::end(&db, &session.id, "agent-one")
        .await
        .expect("end");
    prune::prune_session(&db, &session.id).await.expect("prune");

    // Age the tombstone past the window, then sweep.
    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
        vec![
            turso::Value::Text(
                old.format(&time::format_description::well_known::Rfc3339)
                    .expect("format"),
            ),
            turso::Value::Text(session.id.clone()),
        ],
    )
    .await
    .expect("age");

    let committed = prune::sweep(&db, &dir).await.expect("sweep");
    assert_eq!(committed, 1);

    assert!(
        sessions::get(&db, &session.id)
            .await
            .expect("get")
            .is_none(),
        "the session row is gone"
    );
    assert!(!brain_file.exists(), "the brain file is gone");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert!(
        events.iter().all(|event| event.kind != "session"),
        "the session's feed events are gone"
    );
}

#[tokio::test]
async fn prune_keeps_a_keyed_event_and_its_idempotency_row() {
    let dir = temp_dir("prune-keys");
    let db = open(&dir).await;

    let event = || NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: "keep me".to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
    };
    let kept = events::append(&db, "agent-one", Some("keep-key"), event())
        .await
        .expect("append keyed event");

    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one")
        .await
        .expect("end");
    prune::prune_session(&db, &session.id).await.expect("prune");

    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
        vec![
            turso::Value::Text(
                old.format(&time::format_description::well_known::Rfc3339)
                    .expect("format"),
            ),
            turso::Value::Text(session.id.clone()),
        ],
    )
    .await
    .expect("age");
    prune::sweep(&db, &dir).await.expect("sweep");

    let retry = events::append(&db, "agent-one", Some("keep-key"), event())
        .await
        .expect("retry");
    assert_eq!(
        kept, retry,
        "a key whose event still exists is not garbage-collected"
    );
    assert!(
        read_feed(&db, "proj", &FeedQuery::default())
            .await
            .expect("feed")
            .events
            .iter()
            .any(|event| event.id == kept),
        "the keyed event survives the prune"
    );
}

#[tokio::test]
async fn a_fresh_prune_is_not_committed() {
    let dir = temp_dir("prune-fresh");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one")
        .await
        .expect("end");
    prune::prune_session(&db, &session.id).await.expect("prune");

    let committed = prune::sweep(&db, &dir).await.expect("sweep");
    assert_eq!(committed, 0, "the window has not passed");
    assert!(
        sessions::get(&db, &session.id)
            .await
            .expect("get")
            .is_some()
    );
}

#[tokio::test]
async fn pruning_an_active_session_is_rejected() {
    let dir = temp_dir("prune-active");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let err = prune::prune_session(&db, &session.id)
        .await
        .expect_err("an active session cannot be pruned");
    assert_eq!(err.code(), agent_hub::error::ErrorCode::Conflict);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_racing_a_prune_never_leaves_a_session_active_and_pruned() {
    let dir = temp_dir("prune-resume-race");
    let db = Arc::new(open(&dir).await);

    // The window between the prune's status check and its write is narrow, so
    // the race is run repeatedly against a fresh session each round.
    for round in 0..20 {
        let name = format!("nightly-{round}");
        let session = sessions::start(&db, "proj", &name, "agent-one")
            .await
            .expect("start");
        sessions::end(&db, &session.id, "agent-one")
            .await
            .expect("end");

        let pruning = db.clone();
        let id = session.id.clone();
        let prune = tokio::spawn(async move { prune::prune_session(&pruning, &id).await });
        let resuming = db.clone();
        let resume =
            tokio::spawn(
                async move { sessions::start(&resuming, "proj", &name, "agent-one").await },
            );
        let _ = prune.await.expect("join prune");
        let _ = resume.await.expect("join resume");

        let row = sessions::get(&db, &session.id)
            .await
            .expect("get")
            .expect("the row is still there");
        assert!(
            row.status != "active" || row.deleted_at.is_none(),
            "round {round}: a pruned session is active, so the sweep would remove a live brain"
        );
    }
}

#[tokio::test]
async fn undo_after_the_window_is_rejected() {
    let dir = temp_dir("prune-late-undo");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one")
        .await
        .expect("end");
    let token = prune::prune_session(&db, &session.id).await.expect("prune");

    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
        vec![
            turso::Value::Text(
                old.format(&time::format_description::well_known::Rfc3339)
                    .expect("format"),
            ),
            turso::Value::Text(session.id.clone()),
        ],
    )
    .await
    .expect("age");

    let err = prune::undo(&db, &token.undo_token)
        .await
        .expect_err("the window has passed");
    assert_eq!(err.code(), agent_hub::error::ErrorCode::Conflict);
}
