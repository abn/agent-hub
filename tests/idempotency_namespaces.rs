//! Adversarial tests for idempotency namespaces, cross-operation isolation,
//! target binding, and resolved question immutability.

use agent_hub::limits::InboxCaps;
use agent_hub::store::{
    events::{self, NewEvent},
    questions::{self, NewQuestion},
};
use common::temp::TempDir;

mod common;

const CAPS: InboxCaps = InboxCaps {
    per_actor: 10,
    per_project: 50,
};

#[tokio::test]
async fn signal_cannot_replay_question_id() {
    let dir = TempDir::new("idem-signal-question");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    // Post a question with idempotency key "shared-key"
    let q_id = questions::post(
        &db,
        &CAPS,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject: "Should we proceed?",
            body: None,
            context: None,
            to: None,
            idempotency_key: Some("shared-key"),
            session_id: None,
        },
    )
    .await
    .expect("post question");

    // Post a signal with the same idempotency key "shared-key"
    let s_id = events::append(
        &db,
        "agent-one",
        Some("shared-key"),
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "Work in progress".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append signal");

    // In unfixed code, signal looked up "event" namespace and returned q_id!
    assert_ne!(
        s_id, q_id,
        "signal write must NOT replay a question id when sharing an idempotency key"
    );
}

#[tokio::test]
async fn resolved_question_refuses_second_answer() {
    let dir = TempDir::new("idem-resolved-question");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    let q_id = questions::post(
        &db,
        &CAPS,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject: "Need guidance",
            body: None,
            context: None,
            to: None,
            idempotency_key: None,
            session_id: None,
        },
    )
    .await
    .expect("post question");

    // First answer resolves the question
    let _ans1 = questions::answer(&db, "human", &q_id, "Proceed with option A", None)
        .await
        .expect("first answer");

    // Second distinct answer must be refused
    let err = questions::answer(&db, "human", &q_id, "Actually do option B", None)
        .await
        .expect_err("second answer on resolved question must be rejected");

    assert_eq!(
        err.code(),
        agent_hub::error::ErrorCode::Conflict,
        "second answer must fail with Conflict, got: {err}"
    );
}

#[tokio::test]
async fn decision_key_cannot_replay_across_different_approvals() {
    let dir = TempDir::new("idem-decision-target");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    let app1 = events::append_action(
        &db,
        &CAPS,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "approval".to_string(),
            summary: "Approval 1".to_string(),
            payload: None,
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("post approval 1");

    let app2 = events::append_action(
        &db,
        &CAPS,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "approval".to_string(),
            summary: "Approval 2".to_string(),
            payload: None,
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("post approval 2");

    // Decide approval 1 with key "dec-key"
    let d1 = questions::decide(&db, "human", &app1, true, Some("ok 1"), Some("dec-key"))
        .await
        .expect("decide approval 1");

    // Attempt to decide approval 2 with the same key "dec-key"
    let res = questions::decide(&db, "human", &app2, true, Some("ok 2"), Some("dec-key")).await;

    // In unfixed code, it looked up "decision" + "dec-key" without checking approval_id
    // and returned d1 (the answer from approval 1), leaving approval 2 untouched!
    if let Ok(d2) = res {
        assert_ne!(
            d2, d1,
            "decision on approval 2 must NOT replay approval 1's decision event id"
        );
    }
}

#[tokio::test]
async fn artifact_publish_key_does_not_suppress_update() {
    let dir = TempDir::new("idem-artifact-publish-update");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    // Publish artifact v1 with key "art-key"
    let art1 = agent_hub::store::artifacts::publish(
        &db,
        &dir,
        agent_hub::store::artifacts::NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Artifact Title",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"version 1",
            envelope: None,
            session_id: None,
        },
        Some("art-key"),
    )
    .await
    .expect("publish v1");
    assert_eq!(art1.version, 1);

    // Attempt to update artifact with new content using the same key "art-key"
    let res = agent_hub::store::artifacts::update(
        &db,
        &dir,
        "agent-one",
        &art1.id,
        b"version 2 content",
        agent_hub::store::artifacts::EnvelopeUpdate::Keep,
        agent_hub::store::artifacts::UpdateOptions::default(),
        Some("art-key"),
    )
    .await;

    // In unfixed code, update looks up "artifact" + "art-key", finds v1, and returns v1!
    // It must NOT replay v1! It should either update to v2 under "artifact:update"
    // or reject cross-operation key reuse.
    if let Ok(updated) = res {
        assert_eq!(
            updated.version, 2,
            "update using publish key must not be suppressed to v1"
        );
    }
}

#[tokio::test]
async fn signal_key_cannot_resolve_question_without_answer() {
    let dir = TempDir::new("idem-signal-answer");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    // Append a signal with key "sig-key"
    let sig_id = events::append(
        &db,
        "agent-one",
        Some("sig-key"),
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "Generic signal".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append signal");

    // Post a question
    let q_id = questions::post(
        &db,
        &CAPS,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject: "A question",
            body: None,
            context: None,
            to: None,
            idempotency_key: None,
            session_id: None,
        },
    )
    .await
    .expect("post question");

    // Attempt to answer the question using the same idempotency key "sig-key"
    let ans_id = questions::answer(&db, "human", &q_id, "The answer", Some("sig-key")).await;

    // In unfixed code: questions::answer replayed the signal id and marked the question resolved!
    // The answer ID must NOT equal the signal ID!
    if let Ok(id) = ans_id {
        assert_ne!(
            id, sig_id,
            "answer must not replay a signal event ID and resolve without creating an answer"
        );
    }
}

/// The answer path binds a key to the question it answered. Without the binding
/// the same key replays the first question's answer id for a second question.
#[tokio::test]
async fn answer_key_cannot_replay_across_different_questions() {
    let dir = TempDir::new("idem-answer-binding");
    let db = common::store::open(&dir).await;
    let _ = agent_hub::store::projects::create(&db, "proj", "Project").await;

    let q1 = questions::post(
        &db,
        &CAPS,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject: "One",
            body: None,
            context: None,
            to: None,
            idempotency_key: None,
            session_id: None,
        },
    )
    .await
    .expect("post q1");
    let q2 = questions::post(
        &db,
        &CAPS,
        NewQuestion {
            actor: "agent-one",
            project_id: "proj",
            subject: "Two",
            body: None,
            context: None,
            to: None,
            idempotency_key: None,
            session_id: None,
        },
    )
    .await
    .expect("post q2");

    let _ = questions::answer(&db, "human", &q1, "Answer one", Some("shared-answer-key"))
        .await
        .expect("answer q1");

    let err = questions::answer(&db, "human", &q2, "Answer two", Some("shared-answer-key"))
        .await
        .expect_err("a key must not answer a different question");
    assert_eq!(
        err.code(),
        agent_hub::error::ErrorCode::InvalidArgument,
        "got: {err}"
    );
}
