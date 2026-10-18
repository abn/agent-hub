//! Session anchor and score store tests: the five-field contract, source
//! resolution, the resume view, and the human review gate.

use agent_hub::principal::Principal;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::{anchors, projects, sessions};

mod common;

use common::store::TestDb;

async fn open() -> TestDb {
    let db = common::store::fresh("anchors").await;
    let _ = projects::create(&db, "proj", "Project").await;
    let _ = projects::create(&db, "other", "Other Project").await;
    db
}

fn agent() -> Principal {
    Principal {
        actor: "agent-one".to_string(),
        agent_id: Some("agent-one".to_string()),
        is_admin: false,
        is_pending: false,
    }
}

fn other_agent() -> Principal {
    Principal {
        actor: "agent-two".to_string(),
        agent_id: Some("agent-two".to_string()),
        is_admin: false,
        is_pending: false,
    }
}

fn human() -> Principal {
    Principal {
        actor: "human".to_string(),
        agent_id: None,
        is_admin: true,
        is_pending: false,
    }
}

async fn source(db: &TestDb, project: &str, session_id: &str, summary: &str) -> String {
    events::append(
        db,
        0,
        "agent-one",
        None,
        NewEvent {
            project_id: project.to_string(),
            kind: "signal".to_string(),
            summary: summary.to_string(),
            payload: Some(serde_json::json!({ "body": summary })),
            needs_action: false,
            thread_id: None,
            session_id: Some(session_id.to_string()),
        },
    )
    .await
    .expect("source event")
}

fn anchor_request(source_ids: Vec<String>) -> anchors::AnchorRequest {
    anchors::AnchorRequest {
        phase: "implement".to_string(),
        summary: "Plan done, tests green.".to_string(),
        next_steps: vec!["open review".to_string()],
        source_ids,
    }
}

#[tokio::test]
async fn an_anchor_round_trips_and_resume_reads_after_it() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let first = source(&db, "proj", &session.id, "plan written").await;
    let second = source(&db, "proj", &session.id, "tests green").await;

    let anchor = anchors::append_anchor(
        &db,
        0,
        &agent(),
        Some("anchor-1"),
        &session,
        anchor_request(vec![first.clone(), second.clone()]),
    )
    .await
    .expect("anchor");

    assert_eq!(anchor.phase, "implement");
    assert_eq!(anchor.summary, "Plan done, tests green.");
    assert_eq!(anchor.next_steps, vec!["open review"]);
    assert_eq!(anchor.owner, "agent-one");
    assert_eq!(anchor.sources.len(), 2);
    assert!(anchor.sources.iter().all(|source| source.found));

    // A retry with the same key returns the original anchor and writes nothing.
    let again = anchors::append_anchor(
        &db,
        0,
        &agent(),
        Some("anchor-1"),
        &session,
        anchor_request(vec![first, second]),
    )
    .await
    .expect("retry");
    assert_eq!(again.id, anchor.id);

    let after = source(&db, "proj", &session.id, "review opened").await;
    let resume = anchors::resume(&db, &session, 50).await.expect("resume");
    let stored = resume.anchor.expect("anchor");
    assert_eq!(stored.id, anchor.id);
    assert_eq!(resume.events.len(), 1);
    assert_eq!(resume.events[0].id, after);
    assert_eq!(resume.next_since.as_deref(), Some(after.as_str()));
    assert!(!resume.truncated);
}

#[tokio::test]
async fn an_anchor_refuses_a_source_from_another_project() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let outside = source(&db, "other", &session.id, "not here").await;

    let err = anchors::append_anchor(
        &db,
        0,
        &agent(),
        None,
        &session,
        anchor_request(vec![outside]),
    )
    .await
    .expect_err("a source outside the project is refused");

    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn another_agent_cannot_anchor_someone_elses_session() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");

    let err = anchors::append_anchor(
        &db,
        0,
        &other_agent(),
        None,
        &session,
        anchor_request(Vec::new()),
    )
    .await
    .expect_err("an anchor belongs to the session owner");

    assert_eq!(err.code(), agent_hub::error::ErrorCode::Forbidden);
}

#[tokio::test]
async fn a_human_score_cites_the_anchor_and_appears_on_resume() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let anchor =
        anchors::append_anchor(&db, 0, &agent(), None, &session, anchor_request(Vec::new()))
            .await
            .expect("anchor");

    let score = anchors::append_score(
        &db,
        0,
        &human(),
        Some("score-1"),
        &session,
        anchors::ScoreRequest {
            slice: vec![anchor.id.clone()],
            score: 4,
            rationale: " plan traceable to sources ".to_string(),
        },
    )
    .await
    .expect("score");

    assert_eq!(score.slice, vec![anchor.id.clone()]);
    assert_eq!(score.score, 4);
    assert_eq!(score.rationale, "plan traceable to sources");
    assert_eq!(score.reviewer, "human");

    let resume = anchors::resume(&db, &session, 50).await.expect("resume");
    assert_eq!(resume.scores.len(), 1);
    assert_eq!(resume.scores[0].id, score.id);
}

#[tokio::test]
async fn a_score_refuses_an_anchor_from_another_session() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let other = sessions::start(&db, "proj", "other", "agent-one")
        .await
        .expect("other session");
    let anchor = anchors::append_anchor(&db, 0, &agent(), None, &other, anchor_request(Vec::new()))
        .await
        .expect("other anchor");

    let err = anchors::append_score(
        &db,
        0,
        &human(),
        None,
        &session,
        anchors::ScoreRequest {
            slice: vec![anchor.id],
            score: 4,
            rationale: "wrong session".to_string(),
        },
    )
    .await
    .expect_err("a score only grades its own session");

    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn an_agent_cannot_write_a_score() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let anchor =
        anchors::append_anchor(&db, 0, &agent(), None, &session, anchor_request(Vec::new()))
            .await
            .expect("anchor");

    let err = anchors::append_score(
        &db,
        0,
        &agent(),
        None,
        &session,
        anchors::ScoreRequest {
            slice: vec![anchor.id],
            score: 4,
            rationale: "self review".to_string(),
        },
    )
    .await
    .expect_err("a score is the human's");

    assert_eq!(err.code(), agent_hub::error::ErrorCode::Forbidden);
}

#[tokio::test]
async fn an_anchor_replay_survives_the_session_ending() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let first = anchors::append_anchor(
        &db,
        0,
        &agent(),
        Some("same-key"),
        &session,
        anchor_request(Vec::new()),
    )
    .await
    .expect("anchor");
    sessions::end(&db, &session.id, "agent-one", Some("agent-one"), None)
        .await
        .expect("end");

    let again = anchors::append_anchor(
        &db,
        0,
        &agent(),
        Some("same-key"),
        &session,
        anchor_request(Vec::new()),
    )
    .await
    .expect("a retry is answered, not refused");
    assert_eq!(again.id, first.id);
}

#[tokio::test]
async fn one_key_in_two_sessions_writes_two_anchors() {
    let db = open().await;
    let one = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("first");
    let two = sessions::start(&db, "proj", "other", "agent-one")
        .await
        .expect("second");

    let first = anchors::append_anchor(
        &db,
        0,
        &agent(),
        Some("shared"),
        &one,
        anchor_request(Vec::new()),
    )
    .await
    .expect("first anchor");
    let second = anchors::append_anchor(
        &db,
        0,
        &agent(),
        Some("shared"),
        &two,
        anchor_request(Vec::new()),
    )
    .await
    .expect("second anchor");

    assert_ne!(first.id, second.id, "a key is scoped to its session");
}

#[tokio::test]
async fn a_score_replay_writes_one_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let anchor =
        anchors::append_anchor(&db, 0, &agent(), None, &session, anchor_request(Vec::new()))
            .await
            .expect("anchor");
    let request = || anchors::ScoreRequest {
        slice: vec![anchor.id.clone()],
        score: 4,
        rationale: "traceable".to_string(),
    };

    let first = anchors::append_score(&db, 0, &human(), Some("once"), &session, request())
        .await
        .expect("score");
    let again = anchors::append_score(&db, 0, &human(), Some("once"), &session, request())
        .await
        .expect("replay");
    assert_eq!(first.id, again.id);

    let scores = events::read_feed(
        &db,
        "proj",
        &agent_hub::store::events::FeedQuery {
            kinds: Some(vec!["score".to_string()]),
            ..Default::default()
        },
    )
    .await
    .expect("feed")
    .events;
    assert_eq!(scores.len(), 1);
}

#[tokio::test]
async fn an_anchor_cannot_cite_an_audit_event() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    let audit = events::append(
        &db,
        0,
        "hub",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "system".to_string(),
            summary: "token issued".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("audit event");

    let err = anchors::append_anchor(
        &db,
        0,
        &agent(),
        None,
        &session,
        anchor_request(vec![audit]),
    )
    .await
    .expect_err("an audit event is not a citable source");

    assert_eq!(err.code(), agent_hub::error::ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn resume_reports_a_full_page_as_complete() {
    let db = open().await;
    let session = sessions::start(&db, "proj", "nightly", "agent-one")
        .await
        .expect("session");
    anchors::append_anchor(&db, 0, &agent(), None, &session, anchor_request(Vec::new()))
        .await
        .expect("anchor");
    let first = source(&db, "proj", &session.id, "one").await;
    source(&db, "proj", &session.id, "two").await;

    let one = anchors::resume(&db, &session, 1).await.expect("one");
    assert!(one.truncated);
    assert_eq!(one.events.len(), 1);
    assert_eq!(one.next_since.as_deref(), Some(first.as_str()));

    let two = anchors::resume(&db, &session, 2).await.expect("two");
    assert!(!two.truncated, "a page that is exactly full is complete");
    assert_eq!(two.events.len(), 2);
}
