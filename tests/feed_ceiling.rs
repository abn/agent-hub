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

/// The configured ceiling bounds the agent-content writers, not only
/// `signal_append`: with the feed full, an artifact publish is refused, while a
/// knowledge base page still writes because its feed signal is best-effort.
#[tokio::test]
async fn a_full_feed_refuses_an_artifact_publish_but_not_a_knowledge_base_write() {
    use agent_hub::limits::EventCeiling;

    let state = common::state::open_with("feed-ceiling-artifact", |config| {
        config.events_per_project = EventCeiling { per_project: 1 };
    })
    .await;
    let project = agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("create project");

    // Fill the feed with the one event it may hold.
    events::append(&state.db, 1, "agent-one", None, signal("the one event"))
        .await
        .expect("the first event fills the ceiling");

    // The artifact publish is an agent-content writer, so the ceiling bounds it.
    let refused = agent_hub::store::artifacts::publish_capped(
        &state.db,
        &state.data_dir,
        1,
        agent_hub::store::artifacts::NewArtifact {
            actor: "agent-one",
            project_id: &project.id,
            title: "Report",
            description: "",
            label: None,
            kind: "markdown",
            content: b"# report",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect_err("a full feed refuses the artifact feed event");
    assert_eq!(refused.code(), ErrorCode::Conflict);

    // A knowledge base write is not refused: its signal is best-effort and is
    // dropped when the feed is full, while the page write still succeeds.
    let written = agent_hub::brain::knowledge::put(
        &state,
        &project.id,
        "agent-one",
        "/fs/notes.md",
        "# notes\n",
        None,
    )
    .await
    .expect("the page write succeeds with its signal dropped");
    assert_eq!(written.path, "/fs/notes.md");

    let stored = agent_hub::brain::knowledge::page_path("/fs/notes.md").expect("path");
    assert_eq!(
        state
            .knowledge
            .open_existing(&project.id, agent_hub::brain::KNOWLEDGE_FILE)
            .await
            .expect("open knowledge")
            .expect("the knowledge file exists")
            .get(&stored)
            .await
            .expect("read page")
            .expect("the page is stored"),
        b"# notes\n",
    );

    assert_eq!(
        events::count_for_project(&state.db, &project.id)
            .await
            .expect("count"),
        1,
        "the dropped signal left the feed at its ceiling"
    );

    // A lifecycle write is exempt by construction (`sessions.rs` passes no
    // ceiling), so a full feed can never refuse `session_start`.
    let session =
        agent_hub::store::sessions::start(&state.db, &project.id, "bootstrap", "agent-one")
            .await
            .expect("session_start works at a full ceiling");
    assert_eq!(session.status, "active");
    assert_eq!(
        events::count_for_project(&state.db, &project.id)
            .await
            .expect("count"),
        2,
        "the lifecycle event lands even though the ceiling is full"
    );
}
