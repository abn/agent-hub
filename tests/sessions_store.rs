//! Session store tests: identity, resume, lifecycle events, and listing.

use agent_hub::error::ErrorCode;
use agent_hub::store::events::{FeedQuery, read_feed};
use agent_hub::store::{prune, sessions};

mod common;

use common::store::TestDb;
use common::temp::TempDir;

async fn open() -> TestDb {
    let db = common::store::fresh("sessions").await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;
    let _ = agent_hub::store::projects::create(&db, "other", "Other Project").await;
    db
}

#[tokio::test]
async fn start_is_idempotent_on_the_session_name() {
    let db = open().await;
    let first = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let second = common::seed::start_session(&db, "proj", "nightly", "agent-one")
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
async fn two_agents_using_one_name_share_one_session() {
    let db = open().await;
    let one = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("first agent");
    let two = common::seed::start_session(&db, "proj", "nightly", "agent-two")
        .await
        .expect("second agent");

    assert_eq!(one.id, two.id, "a name is one session in its project");
    assert_eq!(one.brain_path, two.brain_path, "and one shared brain");
    assert_eq!(two.started_label, "agent-one", "the starter is kept");
}

#[tokio::test]
async fn resuming_a_pruned_name_is_a_conflict() {
    let db = open().await;
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    common::seed::end_session(&db, &session.id, "agent-one", None)
        .await
        .expect("end");
    let token = prune::prune_session(&db, &session.id).await.expect("prune");

    let err = common::seed::start_session(&db, "proj", "nightly", "agent-one")
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
    let restored = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("resume after undo");
    assert_eq!(restored.id, session.id);
}

#[tokio::test]
async fn end_marks_the_session_and_emits_an_event() {
    let db = open().await;
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    common::seed::end_session(&db, &session.id, "agent-one", None)
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
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    common::seed::end_session(&db, &session.id, "agent-one", None)
        .await
        .expect("end");
    common::seed::end_session(&db, &session.id, "agent-one", None)
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
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let (a, b) = tokio::join!(
        common::seed::end_session(&db, &session.id, "agent-one", None),
        common::seed::end_session(&db, &session.id, "agent-one", None),
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
    common::seed::start_session(&db, "proj", "one", "agent-one")
        .await
        .expect("one");
    let two = common::seed::start_session(&db, "proj", "two", "agent-one")
        .await
        .expect("two");
    common::seed::end_session(&db, &two.id, "agent-one", None)
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
    let err = common::seed::end_session(
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
    let err = common::seed::start_session(&db, "proj", "../escape", "agent-one")
        .await
        .expect_err("bad name");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_pickups_of_an_ended_session_both_reopen_it() {
    // Ten rounds, because one pass of a race proves nothing.
    for round in 0..10 {
        let db = open().await;
        let session = common::seed::start_session(&db, "proj", "handover", "agent-one")
            .await
            .expect("start");
        common::seed::end_session(&db, &session.id, "agent-one", None)
            .await
            .expect("end");

        let db = std::sync::Arc::new(db);
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut racers = Vec::new();
        for agent in ["agent-two", "agent-three"] {
            let db = db.clone();
            let gate = gate.clone();
            racers.push(tokio::spawn(async move {
                gate.wait().await;
                common::seed::start_session(&db, "proj", "handover", agent).await
            }));
        }
        for racer in racers {
            let resumed = racer
                .await
                .expect("join")
                .unwrap_or_else(|err| panic!("round {round}: the start failed: {err}"));
            assert_eq!(resumed.id, session.id, "round {round}: the one session");
        }
        let row = sessions::get(&db, &session.id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(row.status, "active");
        let events = read_feed(&db, "proj", &FeedQuery::default())
            .await
            .expect("feed")
            .events;
        assert!(
            !events
                .iter()
                .any(|event| event.summary.contains("picked up")),
            "round {round}: nothing is adopted"
        );
    }
}

#[tokio::test]
async fn a_handoff_note_rides_the_session_and_its_event() {
    let db = open().await;
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    common::seed::end_session(&db, &session.id, "agent-one", Some("half applied"))
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
    common::seed::end_session(&db, &session.id, "agent-one", None)
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
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let note = "n".repeat(agent_hub::limits::HANDOFF_CHARS_MAX + 1);

    let err = common::seed::end_session(&db, &session.id, "agent-one", Some(&note))
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
    common::seed::start_session(&db, "proj", "one", "agent-one")
        .await
        .expect("one");
    let two = common::seed::start_session(&db, "proj", "two", "agent-two")
        .await
        .expect("two");
    common::seed::end_session(&db, &two.id, "agent-two", Some("over to you"))
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
    let session = common::seed::start_session(&db, "proj", "old", "agent-one")
        .await
        .expect("start");
    common::seed::end_session(&db, &session.id, "agent-one", None)
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

/// A fixed instant, so the coalescing window and the active window are read
/// against explicit times rather than a sleep.
fn at(offset_secs: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(1_890_000_000).expect("a valid instant")
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
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
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
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
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
async fn a_session_is_live_while_a_participant_is_inside_the_window() {
    let db = open().await;
    for name in ["nightly", "backfill"] {
        common::seed::start_session(&db, "proj", name, "agent-one")
            .await
            .expect("start");
    }
    common::seed::start_session(&db, "other", "nightly", "agent-two")
        .await
        .expect("start");
    let since = |offset: i64| {
        (time::OffsetDateTime::now_utc() + time::Duration::seconds(offset))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format")
    };

    // Every session the members joined just now is live, counted per session.
    assert_eq!(
        sessions::live_count(&db, &since(-900), None)
            .await
            .expect("count"),
        3
    );
    assert_eq!(
        sessions::live_count(&db, &since(-900), Some("proj"))
            .await
            .expect("count"),
        2,
        "a project counts the sessions live in it"
    );
    // A window that starts after the last call holds no live session.
    assert_eq!(
        sessions::live_count(&db, &since(60), None)
            .await
            .expect("count"),
        0,
        "a session drops out one window after its participants' last call"
    );
}

#[tokio::test]
async fn an_ended_or_pruned_session_is_not_live() {
    let db = open().await;
    let ended = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let pruned = common::seed::start_session(&db, "proj", "scratch", "agent-two")
        .await
        .expect("start");
    common::seed::start_session(&db, "proj", "live", "agent-three")
        .await
        .expect("start");

    common::seed::end_session(&db, &ended.id, "agent-one", None)
        .await
        .expect("end");
    common::seed::end_session(&db, &pruned.id, "agent-two", None)
        .await
        .expect("end");
    prune::prune_session(&db, &pruned.id).await.expect("prune");

    let since = (time::OffsetDateTime::now_utc() - time::Duration::seconds(900))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format");
    assert_eq!(
        sessions::live_count(&db, &since, None)
            .await
            .expect("count"),
        1,
        "only the session still running counts"
    );
}

/// The session-side twin of the artifact reconcile: a brain file no session
/// row references is removed at startup, while one a row names survives. A
/// `.tmp` or `.quarantine` file belongs to a fork or a prune and is left alone.
#[tokio::test]
async fn reconcile_removes_an_orphan_session_file_and_keeps_a_referenced_one() {
    let dir = TempDir::new("session-reconcile");
    let db = common::store::open(&dir).await;
    agent_hub::store::projects::create(&db, "proj", "Project")
        .await
        .expect("create project");
    let live = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");

    let project_dir = dir.join("sessions").join("proj");
    std::fs::create_dir_all(&project_dir).expect("create sessions project dir");
    let referenced = dir.join(&live.brain_path);
    std::fs::write(&referenced, b"live brain").expect("write referenced file");
    // A live session's write-ahead log rides the same stem and must survive.
    let referenced_wal = project_dir.join(format!("{}.db-wal", live.id));
    std::fs::write(&referenced_wal, b"live wal").expect("write referenced sidecar");
    let orphan = project_dir.join("01ORPHANSESSION0000000000.db");
    std::fs::write(&orphan, b"orphan brain").expect("write orphan file");
    let orphan_wal = project_dir.join("01ORPHANSESSION0000000000.db-wal");
    std::fs::write(&orphan_wal, b"orphan wal").expect("write orphan sidecar");
    let fork_tmp = project_dir.join("01FORKSESSION000000000000.db.tmp");
    std::fs::write(&fork_tmp, b"fork in flight").expect("write tmp file");
    let quarantine = project_dir.join("01PRUNEDSESSION0000000000.db.quarantine");
    std::fs::write(&quarantine, b"pruned").expect("write quarantine file");

    let removed = agent_hub::brain::reconcile_sessions(&db, &dir)
        .await
        .expect("reconcile");

    assert!(!orphan.exists(), "an orphan brain file is reclaimed");
    assert!(!orphan_wal.exists(), "its sidecar goes with it");
    assert!(referenced.exists(), "a referenced brain file survives");
    assert!(
        referenced_wal.exists(),
        "a referenced session's write-ahead log survives"
    );
    assert!(fork_tmp.exists(), "an in-flight fork's tmp is left alone");
    assert!(
        quarantine.exists(),
        "a prune's quarantine is left to the prune recovery"
    );
    assert_eq!(removed, 1, "the orphan file, its sidecar going with it");
}

/// A failed activity write is retried on the next call rather than skipped for
/// the coalescing window: the window is recorded only once the write lands.
#[tokio::test]
async fn a_failed_activity_write_is_not_marked_as_served() {
    let db = open().await;
    let session = common::seed::start_session(&db, "proj", "nightly", "agent-one")
        .await
        .expect("start");
    let activity = sessions::Activity::new();
    let now = time::OffsetDateTime::now_utc();

    // Make the write fail, then restore the table so the retry can land.
    let conn = db.connect().expect("connect");
    conn.execute("DROP TABLE sessions", ()).await.expect("drop");
    let failed = activity.touch_at(&db, &session.id, now).await;
    assert!(failed.is_err(), "the write failed against a missing table");

    conn.execute(
        "CREATE TABLE sessions(id TEXT PRIMARY KEY, last_activity TEXT)",
        (),
    )
    .await
    .expect("recreate a minimal sessions table");

    assert!(
        activity
            .touch_at(&db, &session.id, now)
            .await
            .expect("retry"),
        "a failed write leaves the window unserved, so the next call retries"
    );
    assert!(
        !activity
            .touch_at(&db, &session.id, now)
            .await
            .expect("second"),
        "a landed write serves the window, so the next call coalesces"
    );
}

// Shared sessions: participants, delivery, children, gates and the write order.

mod shared {
    use agent_hub::author::{Author, ParticipantKey, Writer};
    use agent_hub::brain::shared::{self, BrainOp, SessionWrite};
    use agent_hub::error::{Error, SessionReason};
    use agent_hub::store::events::{self, NewEvent};
    use agent_hub::store::{notifications, prune, questions, sessions};

    use super::common;

    fn key(member: &str, agent: &str) -> ParticipantKey {
        ParticipantKey {
            member: member.to_string(),
            agent: agent.to_string(),
        }
    }

    fn writer(member: &str, agent: &str, session: Option<&str>) -> Writer {
        Writer::call(Author::Participant {
            member: member.to_string(),
            agent: agent.to_string(),
            session: session.map(str::to_string),
        })
    }

    fn reason(err: &Error) -> Option<SessionReason> {
        match err {
            Error::Session(reason, _, _) => Some(*reason),
            _ => None,
        }
    }

    async fn project(db: &turso::Database) {
        let _ = agent_hub::store::projects::create(db, "proj", "Project").await;
    }

    async fn start(db: &turso::Database, name: &str, by: &Writer) -> sessions::Session {
        sessions::start_session(
            db,
            &sessions::StartRequest {
                project_id: "proj",
                session_name: name,
                writer: by,
                exclusive: false,
                fork_from: None,
            },
        )
        .await
        .expect("start")
        .session
    }

    #[tokio::test]
    async fn a_reply_posted_before_a_newcomer_drains_still_reaches_it() {
        let db = common::store::fresh("shared-floor").await;
        project(&db).await;
        let asker = writer("laptop", "claude-code", None);
        let session = start(&db, "feat-x", &asker).await;
        let asking = writer("laptop", "claude-code", Some(&session.id));
        let question = events::append(
            &db,
            0,
            &asking,
            None,
            NewEvent {
                project_id: "proj".to_string(),
                kind: "question".to_string(),
                summary: "which way?".to_string(),
                payload: None,
                needs_action: true,
                thread_id: None,
                session_id: Some(session.id.clone()),
            },
        )
        .await
        .expect("ask");
        questions::answer(
            &db,
            0,
            &common::seed::writer("human"),
            &question,
            "left",
            None,
        )
        .await
        .expect("answer");

        let newcomer = key("desktop", "opencode");
        let joined = sessions::join(&db, &session.id, &newcomer)
            .await
            .expect("join");
        assert!(joined.first);
        let page = notifications::attention(&db, &newcomer, &session.id, 10, None)
            .await
            .expect("attention");
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.event_id.clone())
                .collect::<Vec<_>>(),
            vec![question.clone()],
            "the answer reaches a participant that joined after it"
        );
        assert_eq!(page.items[0].author.member.as_deref(), Some("laptop"));
    }

    #[tokio::test]
    async fn two_settles_of_one_page_deliver_it_once() {
        let db = common::store::fresh("shared-settle").await;
        project(&db).await;
        let by = writer("laptop", "cli", None);
        let session = start(&db, "feat-y", &by).await;
        let asking = writer("laptop", "cli", Some(&session.id));
        let question = events::append(
            &db,
            0,
            &asking,
            None,
            NewEvent {
                project_id: "proj".to_string(),
                kind: "question".to_string(),
                summary: "ok?".to_string(),
                payload: None,
                needs_action: true,
                thread_id: None,
                session_id: Some(session.id.clone()),
            },
        )
        .await
        .expect("ask");
        questions::answer(
            &db,
            0,
            &common::seed::writer("human"),
            &question,
            "yes",
            None,
        )
        .await
        .expect("answer");
        let me = key("laptop", "cli");
        let one = notifications::attention(&db, &me, &session.id, 10, None)
            .await
            .expect("one");
        let two = notifications::attention(&db, &me, &session.id, 10, None)
            .await
            .expect("two");
        assert_eq!(one.items.len(), 1);
        let (a, b) = tokio::join!(
            notifications::settle(&db, &one.settle),
            notifications::settle(&db, &two.settle)
        );
        let delivered = [a.expect("a"), b.expect("b")]
            .iter()
            .filter(|won| **won)
            .count();
        assert_eq!(delivered, 1, "exactly one settle wins");
        let peeked = notifications::peek(&db, &me, &session.id, None, 10, None)
            .await
            .expect("peek");
        assert!(peeked.items.is_empty());
    }

    #[tokio::test]
    async fn leaving_ends_nothing_and_ending_takes_the_live_children() {
        let state = common::state::open("shared-end").await;
        project(&state.db).await;
        let by = writer("laptop", "claude-code", None);
        let parent = start(&state.db, "feat-x", &by).await;
        let child = start(&state.db, "feat-x/review", &by).await;
        assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));

        sessions::leave(&state.db, &parent.id, &key("laptop", "claude-code"))
            .await
            .expect("leave");
        let still = sessions::get(&state.db, &parent.id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(still.status, "active", "leave ends nothing");
        assert_eq!(
            sessions::live_subagent_count(&state.db, "", Some("proj"))
                .await
                .expect("live"),
            1,
            "the child keeps a participant that has not left"
        );
        assert_eq!(
            sessions::live_count(&state.db, "", Some("proj"))
                .await
                .expect("live"),
            0,
            "the parent's only participant left"
        );

        let family = sessions::family(&state.db, &parent.id)
            .await
            .expect("family");
        let gates = state.session_gates.exclusive_all(&family).await;
        let ended = sessions::end_session(&state.db, &gates, &parent.id, &Writer::human(), None)
            .await
            .expect("end");
        drop(gates);
        assert_eq!(ended, vec![parent.id.clone(), child.id.clone()]);

        prune::prune_session(&state.db, &parent.id)
            .await
            .expect("prune");
        let child_row = sessions::get(&state.db, &child.id)
            .await
            .expect("get")
            .expect("row");
        assert!(
            child_row.deleted_at.is_some(),
            "a parent's prune takes its children"
        );
        prune::undo(&state.db, &parent.id).await.expect("undo");
        let child_row = sessions::get(&state.db, &child.id)
            .await
            .expect("get")
            .expect("row");
        assert!(child_row.deleted_at.is_none(), "and undo brings them back");

        let orphan = sessions::start_session(
            &state.db,
            &sessions::StartRequest {
                project_id: "proj",
                session_name: "nothing/here",
                writer: &by,
                exclusive: false,
                fork_from: None,
            },
        )
        .await
        .expect_err("no live parent");
        assert_eq!(reason(&orphan), Some(SessionReason::ParentMissing));
    }

    #[tokio::test]
    async fn an_exclusive_start_refuses_a_held_name_and_new_names_are_for_people() {
        let db = common::store::fresh("shared-names").await;
        project(&db).await;
        let by = writer("laptop", "claude-code", None);
        let held = start(&db, "feat-x", &by).await;
        let taken = sessions::start_session(
            &db,
            &sessions::StartRequest {
                project_id: "proj",
                session_name: "feat-x",
                writer: &by,
                exclusive: true,
                fork_from: None,
            },
        )
        .await
        .expect_err("taken");
        assert_eq!(reason(&taken), Some(SessionReason::NameTaken));
        assert_eq!(
            taken.details().expect("details")["session_id"],
            serde_json::json!(held.id)
        );

        let opaque = "0f8e5c2a-1b3d-4e5f-8a9b-0c1d2e3f4a5b";
        let refused = sessions::open_for_write(&db, "proj", opaque, &by)
            .await
            .expect_err("opaque");
        assert_eq!(reason(&refused), Some(SessionReason::NameOpaque));
        // A migrated session keeps whatever name it had.
        db.connect()
            .expect("connect")
            .execute(
                &format!(
                    "INSERT INTO sessions(id, project_id, session_name, status, brain_path, created_at, last_activity)
                     VALUES ('s-old', 'proj', '{opaque}', 'active', 'sessions/proj/s-old.db', 'x', 'x')"
                ),
                (),
            )
            .await
            .expect("a migrated row");
        let (opened, joined) = sessions::open_for_write(&db, "proj", opaque, &by)
            .await
            .expect("a migrated opaque name opens");
        assert_eq!(opened.id, "s-old");
        assert!(joined.first);
    }

    #[tokio::test]
    async fn a_stale_base_is_refused_and_an_own_write_is_not() {
        let state = common::state::open("shared-stale").await;
        project(&state.db).await;
        let laptop = writer("laptop", "claude-code", None);
        let session = start(&state.db, "feat-x", &laptop).await;
        let a = writer("laptop", "claude-code", Some(&session.id));
        let b = writer("desktop", "opencode", Some(&session.id));
        let put = |who: &'static str, bytes: &'static [u8], base: Option<String>| {
            let state = &state;
            let session = session.id.clone();
            let author = if who == "a" { a.clone() } else { b.clone() };
            async move {
                let gate = state.session_gates.shared(&session).await;
                shared::write(
                    state,
                    &gate,
                    SessionWrite {
                        session_id: &session,
                        writer: &author,
                        op_name: "brain.put",
                        op: BrainOp::Put {
                            path: "/kv/plan",
                            bytes,
                        },
                        if_version: base.as_deref(),
                        event: None,
                    },
                )
                .await
            }
        };
        let first = put("a", b"one", None).await.expect("a new key");
        put("a", b"two", None)
            .await
            .expect("an own write needs no base");
        let stale = put("b", b"three", None)
            .await
            .expect_err("another participant");
        assert_eq!(reason(&stale), Some(SessionReason::StaleBase));
        let current = stale.details().expect("details")["version"].clone();
        let wrong = put("b", b"three", first.version.clone())
            .await
            .expect_err("an old base");
        assert_eq!(reason(&wrong), Some(SessionReason::StaleBase));
        put("b", b"three", current.as_str().map(str::to_string))
            .await
            .expect("the current base");

        let gate = state.session_gates.shared(&session.id).await;
        let revert = shared::write(
            &state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &a,
                op_name: "brain.revert",
                op: BrainOp::Revert {
                    path: "/kv/plan",
                    version: first.version.as_deref().expect("version"),
                },
                if_version: None,
                event: None,
            },
        )
        .await
        .expect_err("a revert over another's write is an overwrite");
        assert_eq!(reason(&revert), Some(SessionReason::StaleBase));
        let delete = shared::write(
            &state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &a,
                op_name: "brain.delete",
                op: BrainOp::Delete { path: "/kv/plan" },
                if_version: None,
                event: None,
            },
        )
        .await
        .expect_err("so is a delete");
        assert_eq!(reason(&delete), Some(SessionReason::StaleBase));
    }

    #[tokio::test]
    async fn end_waits_for_a_write_in_flight_and_later_writes_are_refused() {
        let state = common::state::open("shared-gate").await;
        project(&state.db).await;
        let by = writer("laptop", "claude-code", None);
        let session = start(&state.db, "feat-x", &by).await;
        let author = writer("laptop", "claude-code", Some(&session.id));

        let gate = state.session_gates.shared(&session.id).await;
        let ending = {
            let state = state.clone();
            let id = session.id.clone();
            tokio::spawn(async move {
                let family = sessions::family(&state.db, &id).await.expect("family");
                let gates = state.session_gates.exclusive_all(&family).await;
                sessions::end_session(&state.db, &gates, &id, &Writer::human(), None).await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !ending.is_finished(),
            "end waits for the write holding the gate"
        );
        shared::write(
            &state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &author,
                op_name: "brain.put",
                op: BrainOp::Put {
                    path: "/kv/plan",
                    bytes: b"landed",
                },
                if_version: None,
                event: None,
            },
        )
        .await
        .expect("the write in flight lands");
        drop(gate);
        ending.await.expect("join").expect("end");

        let gate = state.session_gates.shared(&session.id).await;
        let refused = shared::write(
            &state,
            &gate,
            SessionWrite {
                session_id: &session.id,
                writer: &author,
                op_name: "brain.put",
                op: BrainOp::Put {
                    path: "/kv/plan",
                    bytes: b"late",
                },
                if_version: None,
                event: None,
            },
        )
        .await
        .expect_err("ended");
        assert_eq!(reason(&refused), Some(SessionReason::Ended));
        let artifact = agent_hub::store::artifacts::publish(
            &state.db,
            state.dir(),
            agent_hub::store::artifacts::NewArtifact {
                writer: &common::seed::writer("laptop"),
                project_id: "proj",
                title: "late",
                description: "",
                label: None,
                kind: "markdown",
                content: b"# late",
                envelope: None,
                session_id: Some(&session.id),
            },
            None,
        )
        .await
        .expect_err("an artifact write re-checks the session");
        assert_eq!(reason(&artifact), Some(SessionReason::Ended));
    }

    #[tokio::test]
    async fn a_write_that_crashed_before_its_index_is_reindexed_at_the_next_open() {
        let dir = common::temp::TempDir::new("shared-reindex");
        let config = common::state::config(&dir);
        let session_id = {
            let state = agent_hub::app::AppState::open(config.clone())
                .await
                .expect("open");
            project(&state.db).await;
            let by = writer("laptop", "claude-code", None);
            let session = start(&state.db, "feat-x", &by).await;
            let brain = state.brain.open("proj", &session.id).await.expect("brain");
            brain.put("/kv/plan", b"in the file").await.expect("put");
            // The crash: the marker is set and the index never landed.
            state
                .db
                .connect()
                .expect("connect")
                .execute(
                    &format!(
                        "UPDATE sessions SET index_pending = 'w-1' WHERE id = '{}'",
                        session.id
                    ),
                    (),
                )
                .await
                .expect("mark");
            session.id
        };
        let state = agent_hub::app::AppState::open(config)
            .await
            .expect("reopen");
        let session = sessions::get(&state.db, &session_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(session.index_pending, None, "the marker is cleared");
        let hits = agent_hub::store::search::query(
            &state.db,
            &agent_hub::store::search::SearchQuery {
                text: "file".to_string(),
                project_id: None,
                kind: None,
                session_id: None,
                limit: 20,
                mode: None,
            },
        )
        .await
        .expect("search");
        assert!(
            hits.iter().any(|hit| hit.ref_id == "/kv/plan"),
            "the brain value is searchable again: {hits:?}"
        );
    }
}
