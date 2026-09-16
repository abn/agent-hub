//! Session store tests: identity, resume, lifecycle events, and listing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::store::events::{FeedQuery, read_feed};
use agent_hub::store::sessions;
use agent_hub::store::{migrate, open_engine};

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
    let db = open_engine(&temp_db("sessions"))
        .await
        .expect("open engine");
    migrate(&db).await.expect("migrate");
    db
}

#[tokio::test]
async fn start_is_idempotent_on_the_session_name() {
    let db = open().await;
    let first = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let second = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("resume");

    assert_eq!(
        first.id, second.id,
        "the same name resumes the same session"
    );
    assert_eq!(second.status, "active");
    assert!(second.brain_path.starts_with("sessions/proj/"));

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let starts = events
        .iter()
        .filter(|event| event.kind == "session" && event.summary.contains("started"))
        .count();
    assert_eq!(starts, 1, "a resume does not emit another start event");
}

#[tokio::test]
async fn end_marks_the_session_and_emits_an_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    sessions::end(&db, &session.id, "agent-one")
        .await
        .expect("end");

    let ended = sessions::get(&db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(ended.status, "ended");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert!(
        events.iter().any(|event| event.summary.contains("ended")),
        "an end event lands on the feed"
    );
}

#[tokio::test]
async fn list_returns_active_and_ended_sessions() {
    let db = open().await;
    sessions::start(&db, "proj", "one", "agent-one")
        .await
        .expect("one");
    let two = sessions::start(&db, "proj", "two", "agent-one")
        .await
        .expect("two");
    sessions::end(&db, &two.id, "agent-one")
        .await
        .expect("end two");

    let listed = sessions::list(&db, "proj").await.expect("list");
    assert_eq!(listed.len(), 2);
    let names: Vec<&str> = listed.iter().map(|s| s.session_name.as_str()).collect();
    assert!(names.contains(&"one"));
    assert!(names.contains(&"two"));
}

#[tokio::test]
async fn end_unknown_session_is_not_found() {
    let db = open().await;
    let err = sessions::end(&db, "01900000-0000-0000-0000-000000000000", "agent-one")
        .await
        .expect_err("not found");
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn bad_session_name_is_rejected() {
    let db = open().await;
    let err = sessions::start(&db, "proj", "../escape", "agent-one")
        .await
        .expect_err("bad name");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}
