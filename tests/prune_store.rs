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
    sessions::end(&db, &session.id, "agent-one", None)
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

    sessions::end(&db, &session.id, "agent-one", None)
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
async fn sweep_skips_a_session_it_cannot_commit() {
    let dir = temp_dir("prune-sweep-skip");
    let db = open(&dir).await;

    let blocked = sessions::start(&db, "proj", "blocked", "agent-one")
        .await
        .expect("start");
    let next = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    for session in [&blocked, &next] {
        sessions::end(&db, &session.id, "agent-one", None)
            .await
            .expect("end");
        prune::prune_session(&db, &session.id).await.expect("prune");
    }

    // A directory where the brain file belongs cannot be removed as a file, so
    // this session's commit fails every time it is swept.
    std::fs::create_dir_all(dir.join(&blocked.brain_path)).expect("brain directory");

    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE deleted_at IS NOT NULL",
        vec![turso::Value::Text(
            old.format(&time::format_description::well_known::Rfc3339)
                .expect("format"),
        )],
    )
    .await
    .expect("age");

    let committed = prune::sweep(&db, &dir).await.expect("sweep");
    assert_eq!(committed, 1, "the session that can be committed is");
    assert!(
        sessions::get(&db, &next.id).await.expect("get").is_none(),
        "the session behind the blocked one is committed"
    );
    assert!(
        sessions::get(&db, &blocked.id)
            .await
            .expect("get")
            .is_some(),
        "the session that could not be committed keeps its tombstone"
    );
}

/// Age a session's tombstone past the undo window so the next sweep commits it.
async fn age_tombstone(db: &turso::Database, session_id: &str) {
    let old = time::OffsetDateTime::now_utc() - time::Duration::seconds(120);
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2",
        vec![
            turso::Value::Text(
                old.format(&time::format_description::well_known::Rfc3339)
                    .expect("format"),
            ),
            turso::Value::Text(session_id.to_string()),
        ],
    )
    .await
    .expect("age");
}

/// Every event id in the store, oldest first.
async fn event_ids(db: &turso::Database) -> Vec<String> {
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query("SELECT id FROM events ORDER BY id", ())
        .await
        .expect("query events");
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        ids.push(row.get::<String>(0).expect("id"));
    }
    ids
}

/// The event ids the payload substring scan used to select for a session.
async fn ids_matching_payload_scan(db: &turso::Database, session_id: &str) -> Vec<String> {
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT id FROM events WHERE kind = 'session' AND payload LIKE ?1 ORDER BY id",
            vec![turso::Value::Text(format!(
                "%\"session_id\":\"{session_id}\"%"
            ))],
        )
        .await
        .expect("query events");
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        ids.push(row.get::<String>(0).expect("id"));
    }
    ids
}

#[tokio::test]
async fn a_committed_prune_removes_exactly_the_sessions_own_lifecycle_events() {
    let dir = temp_dir("prune-set");
    let db = open(&dir).await;

    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    // Ordinary work written during the session, one of every kind storage must
    // never touch, each carrying the session id on the new column.
    for (kind, summary) in [
        ("signal", "a signal"),
        ("finished", "work done"),
        ("approval", "may I"),
        ("artifact", "published"),
    ] {
        events::append(
            &db,
            "agent-one",
            None,
            NewEvent {
                project_id: "proj".to_string(),
                kind: kind.to_string(),
                summary: summary.to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: Some(session.id.clone()),
            },
        )
        .await
        .expect("append");
    }
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end");

    // What the payload substring scan selected, captured before the prune so
    // the two predicates are compared over the same rows.
    let scanned = ids_matching_payload_scan(&db, &session.id).await;
    assert_eq!(scanned.len(), 2, "a started and an ended lifecycle event");
    let before = event_ids(&db).await;

    prune::prune_session(&db, &session.id).await.expect("prune");
    age_tombstone(&db, &session.id).await;
    assert_eq!(prune::sweep(&db, &dir).await.expect("sweep"), 1);

    let after = event_ids(&db).await;
    let removed: Vec<String> = before
        .into_iter()
        .filter(|id| !after.contains(id))
        .collect();
    assert_eq!(
        removed, scanned,
        "the indexed column removes the same events the payload scan did"
    );
    assert_eq!(
        after.len(),
        4,
        "every signal, question, approval and artifact the session wrote stays"
    );
}

#[tokio::test]
async fn pruning_a_source_session_keeps_the_fork_it_left_behind() {
    let dir = temp_dir("prune-fork");
    let db = open(&dir).await;

    let source = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let fork = sessions::insert_fork(&db, &source, "nightly-copy", "agent-two", "forked-one")
        .await
        .expect("fork");
    sessions::end(&db, &source.id, "agent-one", None)
        .await
        .expect("end");

    prune::prune_session(&db, &source.id).await.expect("prune");
    age_tombstone(&db, &source.id).await;
    assert_eq!(prune::sweep(&db, &dir).await.expect("sweep"), 1);

    let feed = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed");
    let summaries: Vec<&str> = feed
        .events
        .iter()
        .map(|event| event.summary.as_str())
        .collect();
    assert_eq!(
        summaries,
        vec!["session nightly-copy forked from agent-one"],
        "the fork's own lifecycle event names the source and belongs to the fork"
    );
    assert!(
        sessions::get(&db, &fork.id)
            .await
            .expect("get")
            .is_some_and(|session| session.deleted_at.is_none()),
        "pruning the source leaves the fork alone"
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
        session_id: None,
    };
    let kept = events::append(&db, "agent-one", Some("keep-key"), event())
        .await
        .expect("append keyed event");

    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one", None)
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
    sessions::end(&db, &session.id, "agent-one", None)
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
        sessions::end(&db, &session.id, "agent-one", None)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prune_that_loses_to_another_prune_says_the_session_is_pruned() {
    let dir = temp_dir("prune-twice");
    let db = Arc::new(open(&dir).await);

    // Two humans, or two tabs, prune the same ended session. The loser's write
    // misses on the tombstone, which is only reached when the two overlap.
    for round in 0..20 {
        let session = sessions::start(&db, "proj", &format!("nightly-{round}"), "agent-one")
            .await
            .expect("start");
        sessions::end(&db, &session.id, "agent-one", None)
            .await
            .expect("end");

        let one = db.clone();
        let id = session.id.clone();
        let first = tokio::spawn(async move { prune::prune_session(&one, &id).await });
        let two = db.clone();
        let id = session.id.clone();
        let second = tokio::spawn(async move { prune::prune_session(&two, &id).await });
        let results = [
            first.await.expect("join first"),
            second.await.expect("join second"),
        ];

        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "round {round}: exactly one prune takes the session"
        );
        let err = results
            .into_iter()
            .find_map(|result| result.err())
            .expect("the prune that lost");
        assert!(
            err.to_string().contains("already pruned"),
            "round {round}: the losing prune reported {err}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undo_the_commit_beat_is_not_reported_as_a_restore() {
    let dir = temp_dir("prune-undo-commit-race");
    let db = Arc::new(open(&dir).await);

    // The gap between the undo's window check and its write is narrow, so the
    // race is run repeatedly against a fresh session each round.
    for round in 0..20 {
        let session = sessions::start(&db, "proj", &format!("nightly-{round}"), "agent-one")
            .await
            .expect("start");
        sessions::end(&db, &session.id, "agent-one", None)
            .await
            .expect("end");
        let token = prune::prune_session(&db, &session.id).await.expect("prune");

        let undoing = db.clone();
        let id = token.undo_token.clone();
        let undo = tokio::spawn(async move { prune::undo(&undoing, &id).await });
        // The sweep's commit removes the row only while the prune still
        // stands, which is the predicate that makes the race observable.
        let committing = db.clone();
        let id = session.id.clone();
        let commit = tokio::spawn(async move {
            let conn = committing.connect().expect("connect");
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .expect("busy timeout");
            conn.execute(
                "DELETE FROM sessions WHERE id = ?1 AND deleted_at IS NOT NULL",
                vec![turso::Value::Text(id)],
            )
            .await
            .expect("commit the prune")
        });
        let undone = undo.await.expect("join undo");
        commit.await.expect("join commit");

        let row = sessions::get(&db, &session.id).await.expect("get");
        if row.is_none() {
            assert!(
                undone.is_err(),
                "round {round}: the undo reported a restore for a session the commit removed"
            );
        }
    }
}

#[tokio::test]
async fn undo_after_the_window_is_rejected() {
    let dir = temp_dir("prune-late-undo");
    let db = open(&dir).await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one", None)
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
