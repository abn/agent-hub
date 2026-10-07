//! Live artifact versions: the fork, the in-place write, the seal, and the
//! conflict that keeps one writer on one version.

use agent_hub::error::ErrorCode;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, LiveOptions, NewArtifact, UpdateOptions};
use agent_hub::store::projects;

mod common;

use common::temp::TempDir;

async fn open(dir: &std::path::Path) -> turso::Database {
    let db = common::store::open(dir).await;
    let _ = projects::create(&db, "proj", "Default Project").await;
    db
}

fn public<'a>(title: &'a str, content: &'a [u8]) -> NewArtifact<'a> {
    NewArtifact {
        actor: "agent-one",
        project_id: "proj",
        title,
        description: "",
        label: None,
        kind: "markdown",
        content,
        envelope: None,
        session_id: None,
    }
}

fn live<'a>(session: &'a str) -> LiveOptions<'a> {
    LiveOptions {
        force: false,
        session_id: Some(session),
    }
}

/// A live session forks one version and leaves the current pointer where it was.
#[tokio::test]
async fn a_draft_forks_one_version_and_leaves_the_current_one_alone() {
    let dir = TempDir::new("artifact-live-fork");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"# Notes\n\nfirst"), None)
        .await
        .expect("publish");
    assert_eq!(artifact.version, 1);

    let drafted = artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"# Notes\n\nsecond",
        EnvelopeUpdate::Keep,
        live("sess-1"),
    )
    .await
    .expect("draft");

    assert_eq!(drafted.version, 1, "the current version does not move");
    assert_eq!(drafted.live_version, Some(2));
    assert_eq!(drafted.live_rev, 1);

    // The artifact's own address still serves the sealed first version.
    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(read.version, 1);
    assert_eq!(bytes, b"# Notes\n\nfirst");

    // The live version is its own address, and it is the second one.
    let (_, live_bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, 2)
        .await
        .expect("live version");
    assert_eq!(live_bytes, b"# Notes\n\nsecond");
}

/// Later writes rewrite the one live version rather than adding versions.
#[tokio::test]
async fn later_drafts_rewrite_the_same_version() {
    let dir = TempDir::new("artifact-live-rewrite");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"one"), None)
        .await
        .expect("publish");

    for (rev, content) in [(1, &b"two"[..]), (2, b"three"), (3, b"four")] {
        let drafted = artifacts::draft(
            &db,
            &dir,
            "agent-one",
            &artifact.id,
            content,
            EnvelopeUpdate::Keep,
            live("sess-1"),
        )
        .await
        .expect("draft");
        assert_eq!(drafted.live_version, Some(2));
        assert_eq!(drafted.live_rev, rev);
    }

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(
        versions.len(),
        2,
        "a live session adds one version, not one per write"
    );
    let (_, bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, 2)
        .await
        .expect("live version");
    assert_eq!(bytes, b"four");
}

/// Publishing seals the live version: the current pointer moves and the live
/// fields clear, so the artifact is an ordinary one again.
#[tokio::test]
async fn publishing_seals_the_live_version_in_place() {
    let dir = TempDir::new("artifact-live-seal");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"one"), None)
        .await
        .expect("publish");
    artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        live("sess-1"),
    )
    .await
    .expect("draft");

    let sealed = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"final",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            session_id: Some("sess-1"),
            ..Default::default()
        },
        None,
    )
    .await
    .expect("seal");

    assert_eq!(
        sealed.version, 2,
        "the seal keeps the number the session held"
    );
    assert_eq!(sealed.live_version, None);
    assert_eq!(sealed.live_rev, 0);

    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(read.version, 2);
    assert_eq!(read.live_version, None);
    assert_eq!(bytes, b"final");

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(
        versions.len(),
        2,
        "sealing overwrites the row rather than adding one"
    );
}

/// A second session may not take a version another is writing without saying so.
#[tokio::test]
async fn a_second_session_conflicts_unless_forced() {
    let dir = TempDir::new("artifact-live-conflict");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"one"), None)
        .await
        .expect("publish");
    artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        live("sess-1"),
    )
    .await
    .expect("draft");

    let refused = artifacts::draft(
        &db,
        &dir,
        "agent-two",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        live("sess-2"),
    )
    .await
    .expect_err("a second session is refused");
    assert_eq!(refused.code(), ErrorCode::Conflict);

    let taken = artifacts::draft(
        &db,
        &dir,
        "agent-two",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        LiveOptions {
            force: true,
            session_id: Some("sess-2"),
        },
    )
    .await
    .expect("a forced takeover is allowed");
    assert_eq!(taken.live_session.as_deref(), Some("sess-2"));

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(
        versions.len(),
        2,
        "a takeover does not fork a version of its own"
    );
}

/// A pointer whose session has gone is not live, and the sweeper clears it.
#[tokio::test]
async fn an_idle_pointer_is_cleared_by_the_sweep() {
    let dir = TempDir::new("artifact-live-idle");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"one"), None)
        .await
        .expect("publish");
    artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        live("sess-1"),
    )
    .await
    .expect("draft");

    // A window no session can be inside, which is what a session that stopped
    // being touched looks like to the sweep.
    artifacts::clear_idle_live(&db, "9999-01-01T00:00:00Z")
        .await
        .expect("sweep");

    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(read.live_version, None);
    assert_eq!(read.live_rev, 0);
    assert_eq!(
        read.version, 1,
        "the sealed version is still what the artifact shows"
    );
    assert_eq!(bytes, b"one");

    // The version row stays, so nothing the session wrote is destroyed.
    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
}

/// A version abandoned by the sweep is reusable, not a trap: the next live
/// write reuses its number rather than colliding on the version key, and the
/// artifact stays writable.
#[tokio::test]
async fn a_version_can_be_written_again_after_the_sweep_clears_it() {
    let dir = TempDir::new("artifact-live-reuse");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"one"), None)
        .await
        .expect("publish");
    artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        live("sess-1"),
    )
    .await
    .expect("first draft");
    artifacts::clear_idle_live(&db, "9999-01-01T00:00:00Z")
        .await
        .expect("sweep");

    // The abandoned version row is still at version 2, so the next fork has to
    // reuse it rather than insert a duplicate primary key.
    let again = artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"three",
        EnvelopeUpdate::Keep,
        live("sess-2"),
    )
    .await
    .expect("a second live session");
    assert_eq!(again.live_version, Some(2));
    assert_eq!(again.live_rev, 1, "a reused version starts its own count");

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2, "reuse adds no version");

    // And publishing over the reused row works too.
    let sealed = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"final",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            session_id: Some("sess-2"),
            ..Default::default()
        },
        None,
    )
    .await
    .expect("seal over a reused version");
    assert_eq!(sealed.version, 2);
    let (_, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(bytes, b"final");
}

/// Sealing another session's live version would discard its work, so it is
/// refused the same way a draft is.
#[tokio::test]
async fn sealing_another_sessions_live_version_conflicts() {
    let dir = TempDir::new("artifact-live-seal-conflict");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Notes", b"one"), None)
        .await
        .expect("publish");
    artifacts::draft(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        live("sess-1"),
    )
    .await
    .expect("draft");

    let refused = artifacts::update(
        &db,
        &dir,
        "agent-two",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            session_id: Some("sess-2"),
            ..Default::default()
        },
        None,
    )
    .await
    .expect_err("another session may not seal it");
    assert_eq!(refused.code(), ErrorCode::Conflict);

    let forced = artifacts::update(
        &db,
        &dir,
        "agent-two",
        &artifact.id,
        b"two",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            session_id: Some("sess-2"),
            force: true,
            ..Default::default()
        },
        None,
    )
    .await
    .expect("a forced seal is allowed");
    assert_eq!(forced.version, 2);
    assert_eq!(forced.live_version, None);
}
