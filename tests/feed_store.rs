//! Feed store tests: append, read, idempotency, paging, limits, indexing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::limits::FEED_LIMIT_MAX;
use agent_hub::store::events::{FeedQuery, NewEvent, append, read_feed};
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
    let path = temp_db("feed");
    let db = open_engine(&path).await.expect("open engine");
    migrate(&db).await.expect("migrate");
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
