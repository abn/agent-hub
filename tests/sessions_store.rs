//! Session store tests: identity, resume, lifecycle events, and listing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::store::events::{FeedQuery, read_feed};
use agent_hub::store::{migrate, open_engine, prune, sessions};

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
async fn two_agents_using_one_name_get_two_sessions() {
    let db = open().await;
    let one = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("first agent");
    let two = sessions::start(&db, "proj", "nightly", "agent-two")
        .await
        .expect("second agent");

    assert_ne!(
        one.id, two.id,
        "a name under another owner is another session"
    );
    assert_ne!(
        one.brain_path, two.brain_path,
        "two sessions never share a brain file"
    );
    assert_eq!(two.agent, "agent-two");

    let resumed = sessions::start(&db, "proj", "nightly", "agent-two")
        .await
        .expect("resume");
    assert_eq!(resumed.id, two.id, "each owner resumes its own session");
}

#[tokio::test]
async fn resuming_a_pruned_name_is_a_conflict() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end");
    let token = prune::prune_session(&db, &session.id).await.expect("prune");

    let err = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect_err("the pruned name is not free");
    assert_eq!(err.code(), ErrorCode::Conflict);
    assert!(
        err.to_string()
            .contains(&format!("pruned_session_id={}", session.id)),
        "the refusal names the pruned session: {err}"
    );

    let listed = sessions::list(&db, "proj").await.expect("list");
    assert!(listed.is_empty(), "no second session was created");

    prune::undo(&db, &token.undo_token)
        .await
        .expect("the undo token still works");
    let restored = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("resume after undo");
    assert_eq!(restored.id, session.id);
}

#[tokio::test]
async fn end_marks_the_session_and_emits_an_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    sessions::end(&db, &session.id, "agent-one", None)
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
async fn ending_twice_does_not_emit_a_second_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end");
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end again");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let ends = events
        .iter()
        .filter(|event| event.kind == "session" && event.summary.contains("ended"))
        .count();
    assert_eq!(ends, 1, "a retried end does not append a second event");
}

#[tokio::test]
async fn concurrent_ends_emit_one_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let (a, b) = tokio::join!(
        sessions::end(&db, &session.id, "agent-one", None),
        sessions::end(&db, &session.id, "agent-one", None),
    );
    assert!(
        a.is_ok() || b.is_ok(),
        "at least one end succeeds: {a:?} {b:?}"
    );

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let ends = events
        .iter()
        .filter(|event| event.kind == "session" && event.summary.contains("ended"))
        .count();
    assert_eq!(ends, 1, "concurrent ends append one event");
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
    sessions::end(&db, &two.id, "agent-one", None)
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
    let err = sessions::end(
        &db,
        "01900000-0000-0000-0000-000000000000",
        "agent-one",
        None,
    )
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_pickups_adopt_a_session_once() {
    // Ten rounds, because one pass of a race proves nothing: the losing path
    // is the one that must never report an adoption it did not make.
    for round in 0..10 {
        let db = open().await;
        let session = sessions::start(&db, "proj", "handover", "agent-one")
            .await
            .expect("start");
        sessions::end(&db, &session.id, "agent-one", None)
            .await
            .expect("end");

        // Both callers wait on the same barrier, so neither can have finished
        // resolving the source before the other starts.
        let db = std::sync::Arc::new(db);
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut racers = Vec::new();
        for agent in ["agent-two", "agent-three"] {
            let db = db.clone();
            let gate = gate.clone();
            let source_id = session.id.clone();
            racers.push(tokio::spawn(async move {
                gate.wait().await;
                sessions::start_from(&db, "proj", "handover", agent, &source_id).await
            }));
        }
        let second = racers.pop().expect("second racer").await.expect("join");
        let first = racers.pop().expect("first racer").await.expect("join");

        let adopted: Vec<&sessions::Session> = [&first, &second]
            .into_iter()
            .filter_map(|outcome| match outcome {
                Ok(sessions::Pickup::Adopted { session, .. }) => Some(session),
                _ => None,
            })
            .collect();
        assert_eq!(
            adopted.len(),
            1,
            "round {round}: one adoption: {first:?} {second:?}"
        );
        assert_eq!(adopted[0].id, session.id);

        let owner = sessions::get(&db, &session.id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(
            owner.agent, adopted[0].agent,
            "round {round}: the row has the winner's owner"
        );

        let events = read_feed(&db, "proj", &FeedQuery::default())
            .await
            .expect("feed")
            .events;
        let adoptions = events
            .iter()
            .filter(|event| event.summary.contains("picked up"))
            .count();
        assert_eq!(adoptions, 1, "round {round}: one adoption, one event");
    }
}

#[tokio::test]
async fn a_handoff_note_rides_the_session_and_its_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one", Some("half applied"))
        .await
        .expect("end");

    let ended = sessions::get(&db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(ended.handoff.as_deref(), Some("half applied"));

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let payload = events
        .iter()
        .find(|event| event.summary.contains("ended"))
        .and_then(|event| event.payload.clone())
        .expect("the end event carries a payload");
    assert_eq!(payload["handoff"], "half applied");

    // A retried end does not clear the note.
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end again");
    let again = sessions::get(&db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(again.handoff.as_deref(), Some("half applied"));
}

#[tokio::test]
async fn a_handoff_over_the_cap_is_refused() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let note = "n".repeat(agent_hub::limits::HANDOFF_CHARS_MAX + 1);

    let err = sessions::end(&db, &session.id, "agent-one", Some(&note))
        .await
        .expect_err("over the cap");
    assert_eq!(err.code(), ErrorCode::PayloadTooLarge);
    assert!(err.to_string().contains("limit="), "{err}");

    let untouched = sessions::get(&db, &session.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(untouched.status, "active", "a refused end changes nothing");
}

#[tokio::test]
async fn a_listing_narrows_by_project_status_and_agent() {
    let db = open().await;
    sessions::start(&db, "proj", "one", "agent-one")
        .await
        .expect("one");
    let two = sessions::start(&db, "proj", "two", "agent-two")
        .await
        .expect("two");
    sessions::end(&db, &two.id, "agent-two", Some("over to you"))
        .await
        .expect("end two");

    let all = sessions::query(
        &db,
        &sessions::SessionQuery {
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .expect("list all");
    assert_eq!(all.len(), 2);

    let ended = sessions::query(
        &db,
        &sessions::SessionQuery {
            status: Some("ended"),
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .expect("list ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].handoff.as_deref(), Some("over to you"));

    let owned = sessions::query(
        &db,
        &sessions::SessionQuery {
            agent: Some("agent-one"),
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .expect("list by owner");
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].session_name, "one");

    // A caller that may reach no project sees nothing, whatever exists.
    let confined = sessions::query(
        &db,
        &sessions::SessionQuery {
            visible: Some(&[]),
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .expect("list confined");
    assert!(confined.is_empty());

    let bad = sessions::query(
        &db,
        &sessions::SessionQuery {
            status: Some("retired"),
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .expect_err("unknown status");
    assert_eq!(bad.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn a_pruned_session_is_omitted_from_a_listing() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "old", "agent-one")
        .await
        .expect("start");
    sessions::end(&db, &session.id, "agent-one", None)
        .await
        .expect("end");
    prune::prune_session(&db, &session.id).await.expect("prune");

    let listed = sessions::query(
        &db,
        &sessions::SessionQuery {
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .expect("list");
    assert!(listed.is_empty(), "a pruned session is gone from a listing");
}

#[tokio::test]
async fn the_human_reassigns_an_active_session() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "stuck", "agent-one")
        .await
        .expect("start");
    agent_hub::store::identity::create_agent(
        &db,
        "agent-two",
        "Agent two",
        agent_hub::principal::Trust::Trusted,
    )
    .await
    .expect("create the agent the session moves to");

    let moved = sessions::reassign(&db, &session.id, "agent-two", "human")
        .await
        .expect("reassign");
    assert_eq!(moved.agent, "agent-two");
    assert_eq!(moved.id, session.id, "the brain does not move");

    let resumed = sessions::start(&db, "proj", "stuck", "agent-two")
        .await
        .expect("the new owner resumes it by name");
    assert_eq!(resumed.id, session.id);

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert!(
        events
            .iter()
            .any(|event| event.summary.contains("reassigned to agent-two")),
        "the human feed shows the reassignment"
    );
}

/// A fixed instant, so the coalescing window and the active window are read
/// against explicit times rather than a sleep.
fn at(offset_secs: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("a valid instant")
        + time::Duration::seconds(offset_secs)
}

fn stamp(offset_secs: i64) -> String {
    at(offset_secs)
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format")
}

#[tokio::test]
async fn a_touch_is_written_once_per_coalescing_window() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let activity = sessions::Activity::new();

    assert!(
        activity
            .touch_at(&db, &session.id, at(0))
            .await
            .expect("touch"),
        "the first touch of a session is written"
    );
    assert!(
        !activity
            .touch_at(&db, &session.id, at(59))
            .await
            .expect("touch"),
        "a busy agent does not turn every call into a write"
    );
    assert_eq!(
        sessions::get(&db, &session.id)
            .await
            .expect("get")
            .expect("exists")
            .last_activity,
        stamp(0),
        "the coalesced call leaves the row as the first touch wrote it"
    );

    assert!(
        activity
            .touch_at(&db, &session.id, at(60))
            .await
            .expect("touch"),
        "once the window has passed the next call is written"
    );
    assert_eq!(
        sessions::get(&db, &session.id)
            .await
            .expect("get")
            .expect("exists")
            .last_activity,
        stamp(60)
    );
}

#[tokio::test]
async fn a_touch_never_moves_activity_backwards() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let activity = sessions::Activity::new();

    activity
        .touch_at(&db, &session.id, at(600))
        .await
        .expect("touch");
    // A second server clock reading behind the first must not undo it.
    sessions::Activity::new()
        .touch_at(&db, &session.id, at(60))
        .await
        .expect("touch");

    assert_eq!(
        sessions::get(&db, &session.id)
            .await
            .expect("get")
            .expect("exists")
            .last_activity,
        stamp(600)
    );
}

#[tokio::test]
async fn an_agent_is_active_while_its_session_is_inside_the_window() {
    let db = open().await;
    let activity = sessions::Activity::new();
    for name in ["nightly", "backfill"] {
        let session = sessions::start(&db, "proj", name, "agent-one")
            .await
            .expect("start");
        activity
            .touch_at(&db, &session.id, at(0))
            .await
            .expect("touch");
    }
    let other = sessions::start(&db, "proj", "nightly", "agent-two")
        .await
        .expect("start");
    activity
        .touch_at(&db, &other.id, at(0))
        .await
        .expect("touch");
    let elsewhere = sessions::start(&db, "other", "nightly", "agent-three")
        .await
        .expect("start");
    activity
        .touch_at(&db, &elsewhere.id, at(0))
        .await
        .expect("touch");

    // One second before the boundary every agent still counts, and an agent
    // with two sessions counts once.
    assert_eq!(
        sessions::agents_active(&db, &stamp(-899), None)
            .await
            .expect("count"),
        3
    );
    assert_eq!(
        sessions::agents_active(&db, &stamp(-899), Some("proj"))
            .await
            .expect("count"),
        2,
        "a project counts the agents working in it"
    );

    // The boundary is inclusive, so a session touched exactly a window ago is
    // still active; a second later it is not.
    assert_eq!(
        sessions::agents_active(&db, &stamp(0), None)
            .await
            .expect("count"),
        3
    );
    assert_eq!(
        sessions::agents_active(&db, &stamp(1), None)
            .await
            .expect("count"),
        0,
        "an agent drops out one window after its last call"
    );
}

#[tokio::test]
async fn an_ended_or_pruned_session_makes_no_agent_active() {
    let db = open().await;
    let activity = sessions::Activity::new();
    let ended = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let pruned = sessions::start(&db, "proj", "scratch", "agent-two")
        .await
        .expect("start");
    for session in [&ended, &pruned] {
        activity
            .touch_at(&db, &session.id, at(0))
            .await
            .expect("touch");
    }
    let live = sessions::start(&db, "proj", "live", "agent-three")
        .await
        .expect("start");
    activity
        .touch_at(&db, &live.id, at(0))
        .await
        .expect("touch");

    sessions::end(&db, &ended.id, "agent-one", None)
        .await
        .expect("end");
    sessions::end(&db, &pruned.id, "agent-two", None)
        .await
        .expect("end");
    prune::prune_session(&db, &pruned.id).await.expect("prune");

    assert_eq!(
        sessions::agents_active(&db, &stamp(-900), None)
            .await
            .expect("count"),
        1,
        "only the agent still running a session counts"
    );
}
