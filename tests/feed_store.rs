//! Feed store tests: append, read, idempotency, paging, limits, indexing.

use std::sync::Arc;

use agent_hub::error::ErrorCode;
use agent_hub::limits::FEED_LIMIT_MAX;
use agent_hub::store::events::{self, FeedQuery, NewEvent, append, read_feed};

mod common;

use common::store::TestDb;

async fn open() -> TestDb {
    let db = common::store::fresh("feed").await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Default Project").await;
    db
}

fn event(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: Some(serde_json::json!({"body": summary})),
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

#[tokio::test]
async fn append_and_read_newest_first() {
    let db = open().await;
    append(&db, "agent-one", None, event("first"))
        .await
        .expect("append first");
    append(&db, "agent-one", None, event("second needle"))
        .await
        .expect("append second");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("read feed")
        .events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].summary, "second needle");
    assert_eq!(events[0].actor, "agent-one");
    assert_eq!(events[1].summary, "first");
}

#[tokio::test]
async fn idempotency_key_returns_the_same_event_once() {
    let db = open().await;
    let first = append(&db, "agent-one", Some("k1"), event("only once"))
        .await
        .expect("append");
    let second = append(&db, "agent-one", Some("k1"), event("only once"))
        .await
        .expect("append again");
    assert_eq!(first, second);

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("read feed")
        .events;
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn since_cursor_reads_oldest_first() {
    let db = open().await;
    let first = append(&db, "a", None, event("one")).await.expect("one");
    append(&db, "a", None, event("two")).await.expect("two");
    append(&db, "a", None, event("three")).await.expect("three");

    let query = FeedQuery {
        since: Some(first),
        ..Default::default()
    };
    let events = read_feed(&db, "proj", &query).await.expect("read").events;
    assert_eq!(
        events
            .iter()
            .map(|e| e.summary.as_str())
            .collect::<Vec<_>>(),
        vec!["two", "three"]
    );
}

#[tokio::test]
async fn a_default_feed_read_hides_the_audit_trail() {
    let db = open().await;
    let mut audit = event("agent created");
    audit.kind = "system".to_string();
    append(&db, "human", None, audit)
        .await
        .expect("append audit event");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("read feed")
        .events;
    assert!(
        events.is_empty(),
        "a feed read built only with a project id must not return an audit event: {events:?}"
    );
}

#[tokio::test]
async fn payload_over_the_cap_is_rejected() {
    let db = open().await;
    let mut big = event("big");
    big.payload = Some(serde_json::Value::String("x".repeat(300 * 1024)));
    let err = append(&db, "a", None, big).await.expect_err("too large");
    assert_eq!(err.code(), ErrorCode::PayloadTooLarge);
}

#[tokio::test]
async fn unknown_kind_is_rejected() {
    let db = open().await;
    let mut bad = event("bad kind");
    bad.kind = "telepathy".to_string();
    let err = append(&db, "a", None, bad).await.expect_err("bad kind");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn appended_event_is_searchable_through_the_corpus() {
    let db = open().await;
    append(&db, "a", None, event("the engine keeps session state"))
        .await
        .expect("append");

    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT doc_id FROM search_docs WHERE fts_match(body, 'session')",
            (),
        )
        .await
        .expect("search");
    let row = rows.next().await.expect("row").expect("one row");
    let doc_id = match row.get_value(0).expect("value") {
        turso::Value::Text(text) => text,
        other => panic!("unexpected value: {other:?}"),
    };
    assert!(
        doc_id.starts_with("event:"),
        "doc id is namespaced: {doc_id}"
    );
}

#[tokio::test]
async fn empty_forward_poll_keeps_the_since_cursor() {
    let db = open().await;
    append(&db, "a", None, event("one")).await.expect("one");

    let first_page = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("page one");
    let cursor = first_page.next_since.clone().expect("a since cursor");

    let poll = read_feed(
        &db,
        "proj",
        &FeedQuery {
            since: Some(cursor.clone()),
            ..Default::default()
        },
    )
    .await
    .expect("empty poll");
    assert!(poll.events.is_empty(), "no newer events to return");
    assert_eq!(poll.next_since.as_deref(), Some(cursor.as_str()));
    assert_eq!(poll.next_before, None);
}

#[tokio::test]
async fn before_cursor_pages_backwards_with_both_cursors() {
    let db = open().await;
    append(&db, "a", None, event("one")).await.expect("one");
    append(&db, "a", None, event("two")).await.expect("two");
    let third = append(&db, "a", None, event("three")).await.expect("three");

    let first_page = read_feed(
        &db,
        "proj",
        &FeedQuery {
            limit: 2,
            ..Default::default()
        },
    )
    .await
    .expect("page one");
    assert_eq!(first_page.events.len(), 2);
    assert_eq!(first_page.events[0].summary, "three");
    assert_eq!(first_page.events[1].summary, "two");
    assert_eq!(third, first_page.events[0].id);
    assert_eq!(
        first_page.next_since.as_deref(),
        Some(first_page.events[0].id.as_str())
    );
    assert_eq!(
        first_page.next_before.as_deref(),
        Some(first_page.events[1].id.as_str())
    );

    let second_page = read_feed(
        &db,
        "proj",
        &FeedQuery {
            before: first_page.next_before.clone(),
            ..Default::default()
        },
    )
    .await
    .expect("page two");
    assert_eq!(second_page.events.len(), 1);
    assert_eq!(second_page.events[0].summary, "one");
    assert_eq!(
        second_page.next_before.as_deref(),
        Some(second_page.events[0].id.as_str())
    );
}

#[tokio::test]
async fn forward_polling_a_burst_reaches_every_event() {
    const BURST: usize = 300;

    let db = open().await;
    let mut ids = Vec::with_capacity(BURST);
    for index in 0..BURST {
        ids.push(
            append(&db, "a", None, event(&format!("burst {index}")))
                .await
                .expect("append"),
        );
    }

    for (index, pair) in ids.windows(2).enumerate() {
        assert!(
            pair[0] < pair[1],
            "event {index} sorts before the event appended after it: {pair:?}"
        );
    }

    // What a poller does: keep the last id it saw and ask for what came after
    // it. An id that sorts below its predecessor is skipped by that `>` and is
    // then lost to that client for good, so the walk has to see all of them.
    let mut cursor = ids[0].clone();
    let mut walked = Vec::with_capacity(BURST);
    loop {
        let page = read_feed(
            &db,
            "proj",
            &FeedQuery {
                since: Some(cursor.clone()),
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("poll forward");
        if page.events.is_empty() {
            break;
        }
        walked.extend(page.events.iter().map(|e| e.id.clone()));
        cursor = page.next_since.expect("a cursor to continue from");
    }
    assert_eq!(walked, ids[1..], "the walk reaches every later event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_appends_keep_ids_in_commit_order() {
    const WRITERS: usize = 4;
    const PER_WRITER: usize = 25;

    let db = Arc::new(open().await);
    let mut handles = Vec::new();
    for writer in 0..WRITERS {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            let mut ids = Vec::with_capacity(PER_WRITER);
            for index in 0..PER_WRITER {
                ids.push(
                    append(
                        &db,
                        "a",
                        None,
                        event(&format!("writer {writer} event {index}")),
                    )
                    .await
                    .expect("append"),
                );
            }
            ids
        }));
    }

    let mut appended = Vec::new();
    for handle in handles {
        appended.push(handle.await.expect("join"));
    }

    // Each writer awaits its own appends, so its events committed in that
    // order. Id order is the only order the feed exposes, so it has to agree.
    for (writer, ids) in appended.iter().enumerate() {
        for (index, pair) in ids.windows(2).enumerate() {
            assert!(
                pair[0] < pair[1],
                "writer {writer} event {index} sorts before its successor: {pair:?}"
            );
        }
    }

    let page = read_feed(
        &db,
        "proj",
        &FeedQuery {
            limit: FEED_LIMIT_MAX,
            ..Default::default()
        },
    )
    .await
    .expect("read feed");
    assert_eq!(page.events.len(), WRITERS * PER_WRITER);
    let oldest_first: Vec<&str> = page.events.iter().map(|e| e.id.as_str()).rev().collect();
    for (writer, ids) in appended.iter().enumerate() {
        let mut cursor = 0;
        for (index, id) in ids.iter().enumerate() {
            let found = oldest_first[cursor..]
                .iter()
                .position(|candidate| *candidate == id)
                .map(|offset| cursor + offset)
                .unwrap_or_else(|| {
                    panic!("writer {writer} event {index} reads after its predecessor")
                });
            cursor = found + 1;
        }
    }
}

#[tokio::test]
async fn a_sessions_events_are_counted_over_its_own_column() {
    let db = open().await;

    let mine = NewEvent {
        session_id: Some("sess-one".to_string()),
        ..event("written while the session ran")
    };
    append(&db, "agent-one", None, mine).await.expect("append");
    append(&db, "agent-one", None, event("someone else's work"))
        .await
        .expect("append");
    let theirs = NewEvent {
        session_id: Some("sess-two".to_string()),
        ..event("another session's work")
    };
    append(&db, "agent-two", None, theirs)
        .await
        .expect("append");

    assert_eq!(
        agent_hub::store::events::count_for_session(&db, "sess-one")
            .await
            .expect("count"),
        1,
        "a session counts its own events and no one else's"
    );
    assert_eq!(
        agent_hub::store::events::count_for_session(&db, "sess-three")
            .await
            .expect("count"),
        0,
        "a session that wrote nothing counts zero, not the whole feed"
    );

    let latest = agent_hub::store::events::latest_for_session(&db, "sess-one")
        .await
        .expect("latest")
        .expect("one event");
    assert_eq!(latest.summary, "written while the session ran");
    assert_eq!(latest.actor, "agent-one");
}

fn event_in(project_id: &str, summary: &str) -> NewEvent {
    NewEvent {
        project_id: project_id.to_string(),
        ..event(summary)
    }
}

#[tokio::test]
async fn the_feed_cursor_only_ever_moves_forward() {
    let db = open().await;
    let first = append(&db, "agent-one", None, event("first"))
        .await
        .expect("append");
    let second = append(&db, "agent-one", None, event("second"))
        .await
        .expect("append");
    let third = append(&db, "agent-one", None, event("third"))
        .await
        .expect("append");

    assert_eq!(
        events::last_seen(&db, "proj").await.expect("last seen"),
        None,
        "nothing is seen until the human opens the feed"
    );

    let seen = events::mark_seen(&db, "proj", &second)
        .await
        .expect("mark seen");
    assert!(seen.advanced);
    assert_eq!(seen.last_seen.as_deref(), Some(second.as_str()));

    let back = events::mark_seen(&db, "proj", &first)
        .await
        .expect("mark seen");
    assert!(!back.advanced, "an older event does not rewind the cursor");
    assert_eq!(back.last_seen.as_deref(), Some(second.as_str()));

    let forward = events::mark_seen(&db, "proj", &third)
        .await
        .expect("mark seen");
    assert!(forward.advanced);
    assert_eq!(
        events::last_seen(&db, "proj").await.expect("last seen"),
        Some(third)
    );
}

#[tokio::test]
async fn the_cursor_ignores_an_event_from_elsewhere_or_from_nowhere() {
    let db = open().await;
    let _ = agent_hub::store::projects::create(&db, "other", "other").await;
    let mine = append(&db, "agent-one", None, event("mine"))
        .await
        .expect("append");
    let theirs = append(&db, "agent-one", None, event_in("other", "theirs"))
        .await
        .expect("append");
    events::mark_seen(&db, "proj", &mine)
        .await
        .expect("mark seen");

    let foreign = events::mark_seen(&db, "proj", &theirs)
        .await
        .expect("mark seen");
    assert!(
        !foreign.advanced,
        "an event of another project says nothing about this feed"
    );
    assert_eq!(foreign.last_seen.as_deref(), Some(mine.as_str()));

    let unknown = events::mark_seen(&db, "proj", "no-such-event")
        .await
        .expect("mark seen");
    assert!(!unknown.advanced);
    assert_eq!(unknown.last_seen.as_deref(), Some(mine.as_str()));
}

#[tokio::test]
async fn the_unseen_count_follows_the_feed_and_the_cursor() {
    let db = open().await;
    // The counts are reported per project on the roll, so both exist.
    for id in ["proj", "other"] {
        let _ = agent_hub::store::projects::create(&db, id, id).await;
    }
    append(&db, "agent-one", None, event("first"))
        .await
        .expect("append");
    let second = append(&db, "agent-one", None, event("second"))
        .await
        .expect("append");
    append(&db, "agent-one", None, event_in("other", "elsewhere"))
        .await
        .expect("append");

    assert_eq!(events::unseen_count(&db, "proj").await.expect("count"), 2);

    events::mark_seen(&db, "proj", &second)
        .await
        .expect("mark seen");
    assert_eq!(events::unseen_count(&db, "proj").await.expect("count"), 0);

    append(&db, "agent-one", None, event("third"))
        .await
        .expect("append");
    assert_eq!(
        events::unseen_count(&db, "proj").await.expect("count"),
        1,
        "an event written after the cursor is unseen again"
    );

    let counts = events::unseen_counts(&db).await.expect("counts");
    let looked_up = |project: &str| {
        counts
            .iter()
            .find(|row| row.project_id == project)
            .map(|row| row.events)
            .unwrap_or_default()
    };
    assert_eq!(looked_up("proj"), 1);
    assert_eq!(looked_up("other"), 1, "a feed never opened is all unseen");
}

#[tokio::test]
async fn the_unseen_count_seeks_and_does_not_scan_the_feed() {
    let db = open().await;
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            &format!("EXPLAIN QUERY PLAN {}", events::UNSEEN_COUNT_SQL),
            vec![
                turso::Value::Text("proj".to_string()),
                turso::Value::Text("cursor".to_string()),
            ],
        )
        .await
        .expect("explain the count");
    let mut plan = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        if let turso::Value::Text(detail) = row.get_value(3).expect("plan detail") {
            plan.push(detail);
        }
    }

    let over_events = plan
        .iter()
        .find(|step| step.contains("events"))
        .unwrap_or_else(|| panic!("the plan reads the feed: {plan:?}"));
    assert!(
        over_events.starts_with("SEARCH") && over_events.contains("events_feed"),
        "the count seeks into the feed index rather than scanning: {plan:?}"
    );
    // Both bounds are in the seek, so only the events above the cursor are
    // touched. The index is descending, which is how the engine words the
    // upper bound here.
    assert!(
        over_events.contains("project_id=?")
            && (over_events.contains("id<?") || over_events.contains("id>?")),
        "the cursor is part of the seek, not a filter over the project: {plan:?}"
    );
}

#[tokio::test]
async fn thread_id_must_name_an_existing_event_in_the_same_project() {
    let db = open().await;
    let root_id = append(&db, "agent-one", None, event("root event"))
        .await
        .expect("append root");

    // Valid thread_id in the same project is accepted.
    let mut child = event("child event");
    child.thread_id = Some(root_id.clone());
    let child_id = append(&db, "agent-one", None, child)
        .await
        .expect("append child with valid thread_id");
    assert!(!child_id.is_empty());

    // Unknown thread_id is refused with NotFound.
    let mut unknown = event("unknown thread");
    unknown.thread_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string());
    let err = append(&db, "agent-one", None, unknown)
        .await
        .expect_err("unknown thread_id should be refused");
    assert_eq!(err.code(), ErrorCode::NotFound);

    // Thread id in another project is refused with NotFound (no existence oracle).
    let mut cross = event("cross-project thread");
    cross.project_id = "other-proj".to_string();
    cross.thread_id = Some(root_id);
    let err = append(&db, "agent-one", None, cross)
        .await
        .expect_err("cross-project thread_id should be refused");
    assert_eq!(err.code(), ErrorCode::NotFound);

    // Orphan answer is still rejected with InvalidArgument.
    let mut orphan = event("orphan answer");
    orphan.kind = "answer".to_string();
    orphan.thread_id = None;
    let err = append(&db, "agent-one", None, orphan)
        .await
        .expect_err("orphan answer should be rejected");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn a_thread_id_must_name_a_thread_root() {
    let db = open().await;
    let root_id = append(&db, "agent-one", None, event("root event"))
        .await
        .expect("append root");
    let mut child = event("child event");
    child.thread_id = Some(root_id.clone());
    let child_id = append(&db, "agent-one", None, child)
        .await
        .expect("append child");

    // A reply to the child belongs to the root's thread, not to a thread of
    // the child's own: the feed reads a thread by one id.
    let mut grandchild = event("threaded onto a child");
    grandchild.thread_id = Some(child_id.clone());
    let err = append(&db, "agent-one", None, grandchild)
        .await
        .expect_err("a child is not a thread root");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
    assert!(
        err.to_string().contains(&root_id),
        "the refusal names the thread the child belongs to: {err}"
    );

    // A question roots its own thread, so it is a root like any other.
    let mut question = event("a question");
    question.kind = "question".to_string();
    let question_id = append(&db, "agent-one", None, question)
        .await
        .expect("append question");
    let mut under_question = event("under the question");
    under_question.thread_id = Some(question_id);
    append(&db, "agent-one", None, under_question)
        .await
        .expect("a question is a thread root");

    let feed = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("read feed")
        .events;
    assert!(
        feed.iter().all(|e| e.summary != "threaded onto a child"),
        "a refused write leaves nothing behind"
    );
}

#[tokio::test]
async fn a_decision_still_lands_on_an_approval_that_is_itself_threaded() {
    let db = open().await;
    let root_id = append(&db, "agent-one", None, event("root event"))
        .await
        .expect("append root");
    let mut approval = event("may I deploy");
    approval.kind = "approval".to_string();
    approval.thread_id = Some(root_id);
    let approval_id = append(&db, "agent-one", None, approval)
        .await
        .expect("append threaded approval");

    agent_hub::store::questions::decide(&db, "human", &approval_id, true, None, None)
        .await
        .expect("the decision names the approval it replies to, root or not");
}

#[tokio::test]
async fn a_replayed_write_is_returned_before_its_thread_is_checked() {
    let db = open().await;
    let root_id = append(&db, "agent-one", None, event("root event"))
        .await
        .expect("append root");
    let mut child = event("child event");
    child.thread_id = Some(root_id.clone());
    let first = append(&db, "agent-one", Some("k-thread"), child)
        .await
        .expect("append child");

    // The retry carries a thread that no longer resolves. The write was
    // accepted once, so the retry is answered with what the first call got.
    let mut retry = event("child event");
    retry.thread_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string());
    let second = append(&db, "agent-one", Some("k-thread"), retry)
        .await
        .expect("a replay is not validated again");
    assert_eq!(first, second);

    let mut answer = event("reply event");
    answer.kind = "answer".to_string();
    answer.thread_id = Some(root_id);
    let first_ans = append(&db, "agent-one", Some("k-answer"), answer)
        .await
        .expect("append answer");

    let mut orphan = event("reply event");
    orphan.kind = "answer".to_string();
    orphan.thread_id = None;
    let third = append(&db, "agent-one", Some("k-answer"), orphan)
        .await
        .expect("a replay is not validated again");
    assert_eq!(first_ans, third);
}
