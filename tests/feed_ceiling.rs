//! The per-project event ceiling: the writer refuses the event that would
//! cross it, a replay is never refused, zero disables the check, and the hub's
//! own audit trail neither counts nor is refused.

use agent_hub::error::ErrorCode;
use agent_hub::store::events::{self, NewEvent};

mod common;

use common::store::TestDb;

async fn open() -> TestDb {
    let db = common::store::fresh("feed-ceiling").await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Default Project").await;
    db
}

fn signal(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

#[tokio::test]
async fn the_event_that_crosses_the_ceiling_is_refused_and_names_the_cap() {
    let db = open().await;
    for summary in ["one", "two", "three"] {
        events::append(&db, 3, "agent-one", None, signal(summary))
            .await
            .expect("under the ceiling");
    }

    let err = events::append(&db, 3, "agent-one", None, signal("four"))
        .await
        .expect_err("the fourth event crosses the ceiling");

    assert_eq!(err.code(), ErrorCode::Conflict);
    let message = err.to_string();
    assert!(message.contains('3'), "names the cap: {message}");
    assert!(
        message.contains("HUB_EVENTS_PER_PROJECT"),
        "names the setting: {message}"
    );
    assert_eq!(
        events::count_for_project(&db, "proj").await.expect("count"),
        3,
        "the refused event left nothing behind"
    );
}

#[tokio::test]
async fn a_replayed_write_is_not_refused_at_the_ceiling() {
    let db = open().await;
    for summary in ["one", "two"] {
        events::append(&db, 3, "agent-one", None, signal(summary))
            .await
            .expect("under the ceiling");
    }
    let third = events::append(&db, 3, "agent-one", Some("third-key"), signal("three"))
        .await
        .expect("the third fills the ceiling");
    let replay = events::append(&db, 3, "agent-one", Some("third-key"), signal("three"))
        .await
        .expect("a replay of an accepted write is returned, not refused");

    assert_eq!(third, replay);
    assert_eq!(
        events::count_for_project(&db, "proj").await.expect("count"),
        3
    );
}

#[tokio::test]
async fn zero_disables_the_ceiling() {
    let db = open().await;
    for i in 0..5 {
        events::append(&db, 0, "agent-one", None, signal(&format!("signal {i}")))
            .await
            .expect("zero never refuses");
    }
    assert_eq!(
        events::count_for_project(&db, "proj").await.expect("count"),
        5
    );
}

#[tokio::test]
async fn system_audit_events_neither_count_nor_are_refused() {
    let db = open().await;
    let mut audit = signal("who changed what");
    audit.kind = "system".to_string();

    for _ in 0..4 {
        events::append(&db, 2, "human", None, audit.clone())
            .await
            .expect("an audit event is exempt");
    }

    assert_eq!(
        events::count_for_project(&db, "proj").await.expect("count"),
        0,
        "the audit trail is excluded from the count"
    );
}
