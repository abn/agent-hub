//! Prune tests: soft delete, undo within the window, and commit after it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::brain::BrainStore;
use agent_hub::store::events::{FeedQuery, read_feed};
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
async fn a_fresh_prune_is_not_committed() {
    let dir = temp_dir("prune-fresh");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
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
