//! Comment tests: posting, anchors, resolution, deletion, and cleanup.

use agent_hub::error::ErrorCode;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use agent_hub::store::comments::{self, AnchorInput};
use agent_hub::store::projects;

mod common;

use common::store::open;
use common::temp::TempDir;

async fn publish(db: &turso::Database, dir: &std::path::Path) -> String {
    let _ = projects::create(db, "proj", "Default Project").await;
    artifacts::publish(
        db,
        dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Report",
            description: "",
            label: None,
            kind: "html",
            content: b"<h1>body</h1>",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish")
    .id
}

async fn post(db: &turso::Database, artifact_id: &str) -> agent_hub::store::comments::Comment {
    comments::add_comment(
        db,
        artifact_id,
        "agent-one",
        "Looks good.",
        None,
        None,
        None,
        None,
    )
    .await
    .expect("post")
    .0
}

#[tokio::test]
async fn post_list_resolve_and_delete_round_trip() {
    let dir = TempDir::new("comment-round-trip");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    let (comment, replayed) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Looks good.",
        Some(AnchorInput::Point { x: 10.0, y: 20.0 }),
        None,
        Some("hash"),
        None,
    )
    .await
    .expect("post");
    assert!(!replayed);
    assert!(!comment.done);
    assert_eq!(comment.anchor_version, Some(1));
    assert_eq!(
        comment
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.get("mode")),
        Some(&serde_json::Value::String("point".to_string()))
    );

    let listed = comments::list_comments(&db, &artifact_id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, comment.id);

    let resolved = comments::set_comment_done(&db, &comment.id, true)
        .await
        .expect("resolve");
    assert!(resolved.done);

    comments::delete_comment(&db, &comment.id)
        .await
        .expect("delete");
    assert!(
        comments::get_comment(&db, &comment.id)
            .await
            .expect_err("deleted comment")
            .code()
            == ErrorCode::NotFound
    );
    assert!(
        comments::list_comments(&db, &artifact_id)
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn a_post_replays_on_its_idempotency_key_without_a_second_token() {
    let dir = TempDir::new("comment-idem");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    let (first, replayed) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Same note twice.",
        None,
        None,
        Some("hash"),
        Some("comment-key"),
    )
    .await
    .expect("post");
    assert!(!replayed);

    let (second, replayed) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Same note twice.",
        None,
        None,
        Some("other-hash"),
        Some("comment-key"),
    )
    .await
    .expect("replay");
    assert!(replayed);
    assert_eq!(first.id, second.id, "a retry returns the recorded comment");
    assert_eq!(
        second.delete_token_hash, first.delete_token_hash,
        "a replay keeps the recorded hash; the token is not re-issued"
    );

    let listed = comments::list_comments(&db, &artifact_id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1, "a retry posts no second comment");

    let other_id = publish(&db, &dir).await;
    let clash = comments::add_comment(
        &db,
        &other_id,
        "agent-one",
        "Same note twice.",
        None,
        None,
        None,
        Some("comment-key"),
    )
    .await;
    assert_eq!(
        clash.expect_err("key reuse across artifacts").code(),
        ErrorCode::InvalidArgument
    );
}

#[tokio::test]
async fn a_key_records_fresh_after_its_comment_is_deleted() {
    let dir = TempDir::new("comment-key-delete");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    let (first, replayed) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "First note.",
        None,
        None,
        None,
        Some("retried-key"),
    )
    .await
    .expect("post");
    assert!(!replayed);

    comments::delete_comment(&db, &first.id)
        .await
        .expect("delete");

    // Retrying the same key after the comment is gone must record a fresh
    // comment, never resolve to a dead id as an internal, retryable fault.
    let (second, replayed) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Second note.",
        None,
        None,
        None,
        Some("retried-key"),
    )
    .await
    .expect("a retry after delete records fresh");
    assert!(!replayed, "the key was cleared with the comment");
    assert_ne!(first.id, second.id);

    let listed = comments::list_comments(&db, &artifact_id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, second.id);
}

#[tokio::test]
async fn missing_artifacts_and_comments_are_not_found() {
    let dir = TempDir::new("comment-missing");
    let db = open(&dir).await;

    let err = comments::add_comment(&db, "ghost", "agent-one", "Hi.", None, None, None, None)
        .await
        .expect_err("missing artifact");
    assert_eq!(err.code(), ErrorCode::NotFound);

    let err = comments::list_comments(&db, "ghost")
        .await
        .expect_err("missing artifact");
    assert_eq!(err.code(), ErrorCode::NotFound);

    assert!(
        comments::get_comment(&db, "ghost")
            .await
            .expect_err("missing comment")
            .code()
            == ErrorCode::NotFound
    );

    let err = comments::set_comment_done(&db, "ghost", true)
        .await
        .expect_err("missing comment");
    assert_eq!(err.code(), ErrorCode::NotFound);

    let err = comments::delete_comment(&db, "ghost")
        .await
        .expect_err("missing comment");
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn validation_rejects_bad_comments() {
    let dir = TempDir::new("comment-validation");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    let blank = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "   ",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        blank.expect_err("blank body").code(),
        ErrorCode::InvalidArgument
    );

    let long = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        &"b".repeat(2001),
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        long.expect_err("long body").code(),
        ErrorCode::InvalidArgument
    );

    let no_author =
        comments::add_comment(&db, &artifact_id, "  ", "Hi.", None, None, None, None).await;
    assert_eq!(
        no_author.expect_err("blank author").code(),
        ErrorCode::InvalidArgument
    );

    let long_author = comments::add_comment(
        &db,
        &artifact_id,
        &"a".repeat(201),
        "Hi.",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        long_author.expect_err("long author").code(),
        ErrorCode::InvalidArgument
    );

    let nan = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Hi.",
        Some(AnchorInput::Point {
            x: f64::NAN,
            y: 0.0,
        }),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        nan.expect_err("non-finite anchor").code(),
        ErrorCode::InvalidArgument
    );

    let blank_quote = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Hi.",
        Some(AnchorInput::Text {
            quote: "  ".to_string(),
        }),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        blank_quote.expect_err("blank quote").code(),
        ErrorCode::InvalidArgument
    );

    let long_quote = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Hi.",
        Some(AnchorInput::Text {
            quote: "q".repeat(2001),
        }),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        long_quote.expect_err("long quote").code(),
        ErrorCode::InvalidArgument
    );
}

#[tokio::test]
async fn a_text_anchor_is_refused_on_a_protected_version() {
    let dir = TempDir::new("comment-encryption");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    // Protection is sticky: updating with an envelope protects the new
    // version while the first stays public.
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact_id,
        b"ciphertext",
        EnvelopeUpdate::Set(serde_json::json!({"alg": "AES-GCM"})),
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("protect in a new version");

    // A quote of the still-public first version is allowed.
    let quoted = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "About this line.",
        Some(AnchorInput::Text {
            quote: "a public line".to_string(),
        }),
        Some(1),
        None,
        None,
    )
    .await
    .expect("quote of the public version");
    assert_eq!(quoted.0.anchor_version, Some(1));

    // The current version is protected, so a quote defaults to refused.
    let refused = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "About this line.",
        Some(AnchorInput::Text {
            quote: "a secret line".to_string(),
        }),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        refused.expect_err("protected quote").code(),
        ErrorCode::InvalidArgument
    );

    // Point anchors never carry plaintext, so they stay allowed.
    let point = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Pinned here.",
        Some(AnchorInput::Point { x: 1.0, y: 2.0 }),
        None,
        None,
        None,
    )
    .await
    .expect("point anchors stay allowed");
    assert!(point.0.anchor.is_some());
}

#[tokio::test]
async fn a_forged_anchor_version_clamps_to_current() {
    let dir = TempDir::new("comment-clamp");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    let (comment, _) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "From the future.",
        Some(AnchorInput::Point { x: 0.0, y: 0.0 }),
        Some(999),
        None,
        None,
    )
    .await
    .expect("post");
    assert_eq!(
        comment.anchor_version,
        Some(1),
        "a future version lands on current"
    );
}

#[tokio::test]
async fn a_key_replays_fresh_after_its_artifact_is_deleted() {
    let dir = TempDir::new("comment-key-orphan");
    let db = open(&dir).await;
    let artifact_id = publish(&db, &dir).await;

    let (first, _) = comments::add_comment(
        &db,
        &artifact_id,
        "agent-one",
        "Doomed note.",
        None,
        None,
        None,
        Some("orphan-key"),
    )
    .await
    .expect("post");

    artifacts::delete(&db, &dir, "agent-one", &artifact_id)
        .await
        .expect("delete artifact");

    let second_id = publish(&db, &dir).await;
    let (second, replayed) = comments::add_comment(
        &db,
        &second_id,
        "agent-one",
        "Doomed note.",
        None,
        None,
        None,
        Some("orphan-key"),
    )
    .await
    .expect("replay after delete records fresh");
    assert!(!replayed);
    assert_ne!(first.id, second.id);
}

#[tokio::test]
async fn deleting_an_artifact_or_project_drops_its_comments() {
    let dir = TempDir::new("comment-cascade");
    let db = open(&dir).await;

    let first = publish(&db, &dir).await;
    post(&db, &first).await;

    projects::create(&db, "doomed", "Doomed")
        .await
        .expect("create");
    let second = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "doomed",
            title: "Report",
            description: "",
            label: None,
            kind: "html",
            content: b"<h1>body</h1>",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish")
    .id;
    post(&db, &second).await;

    artifacts::delete(&db, &dir, "agent-one", &first)
        .await
        .expect("delete artifact");
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT id FROM comments WHERE artifact_id = ?1",
            vec![turso::Value::Text(first.clone())],
        )
        .await
        .expect("query comments");
    assert!(
        rows.next().await.expect("row").is_none(),
        "no comments survive the artifact"
    );
    drop(rows);

    projects::delete(&db, &dir, "doomed").await.expect("delete");
    let mut rows = conn
        .query(
            "SELECT id FROM comments WHERE artifact_id = ?1",
            vec![turso::Value::Text(second.clone())],
        )
        .await
        .expect("query comments");
    assert!(
        rows.next().await.expect("row").is_none(),
        "no comments survive the project"
    );
}

#[tokio::test]
async fn delete_tokens_round_trip_through_their_hash() {
    let (plaintext, hash) = comments::generate_delete_token();
    assert_eq!(plaintext.len(), 64);
    assert_eq!(hash, agent_hub::store::identity::hash_token(&plaintext));
    let (other, _) = comments::generate_delete_token();
    assert_ne!(plaintext, other, "tokens are random");
}
