//! Reproduction tests for final-path artifact blob orphaning (finding H15).

use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use agent_hub::store::projects;
use common::temp::TempDir;

mod common;

/// A failure after the blob has been promoted leaves the version path for
/// reconcile rather than removing it, because another update may already have
/// won that number.
#[tokio::test]
async fn a_failure_after_promotion_leaves_the_final_path_to_reconcile() {
    let dir = TempDir::new("blob-orphaning-update");
    let db = common::store::open(&dir).await;
    projects::create(&db, "proj", "Project")
        .await
        .expect("create project");

    let art = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Artifact Title",
            description: "",
            label: None,
            kind: "markdown",
            content: b"version 1 content",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish v1");

    // The publish already spent the project's one event, so an update under a
    // ceiling of one is refused after the blob has been promoted: the write
    // order is blob, version row, index, event.
    let res = artifacts::update_for_principal_capped(
        &db,
        &dir,
        None,
        1,
        "agent-one",
        &art.id,
        b"version 2 content that will fail commit",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await;

    assert!(res.is_err(), "the event ceiling refuses the update");

    // The promoted version path is another update's to win once the write lock
    // is released, so the failed call leaves it alone and reconcile sweeps it.
    let v2_path = dir
        .join("artifacts")
        .join("proj")
        .join(&art.id)
        .join("v2.md");
    assert!(
        v2_path.exists(),
        "the promoted final path is left for reconcile, not removed here: {:?}",
        v2_path
    );

    // Reconcile removes it, since no committed version row names it.
    let reaped = agent_hub::blob::reconcile(&db, &dir)
        .await
        .expect("reconcile");
    assert!(reaped >= 1, "reconcile reaps the unreferenced version file");
    assert!(
        !v2_path.exists(),
        "reconcile reclaims the unreferenced final path: {:?}",
        v2_path
    );
}

/// A row already sitting at the next number is reused, not collided with. A
/// live session that went quiet leaves exactly that: its pointer is cleared
/// while its version row stays, and `current_ver` never moved.
#[tokio::test]
async fn a_version_row_left_at_the_next_number_is_reused() {
    let dir = TempDir::new("blob-orphaning-reuse");
    let db = common::store::open(&dir).await;
    projects::create(&db, "proj", "Project")
        .await
        .expect("create project");

    let art = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Artifact Title",
            description: "",
            label: None,
            kind: "markdown",
            content: b"version 1 content",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish v1");

    // A row an abandoned live session left at the next number.
    let conn = db.connect().expect("connect");
    conn.execute(
        "INSERT INTO artifact_versions(artifact_id, version, title, description, kind, encrypted, size_bytes, path, created_at)
         VALUES (?1, 2, 'Abandoned', '', 'markdown', 0, 10, 'dummy', '2026-09-25T00:00:00Z')",
        [art.id.clone()],
    )
    .await
    .expect("insert the abandoned row");

    let updated = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &art.id,
        b"version 2 content",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("the update reuses the row rather than colliding");
    assert_eq!(updated.version, 2);

    let (_, bytes) = artifacts::get(&db, &dir, &art.id).await.expect("get");
    assert_eq!(bytes, b"version 2 content");

    let versions = artifacts::list_versions(&db, &art.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2, "reuse adds no version row");
}

#[tokio::test]
async fn startup_reconcile_removes_unreferenced_version_blobs() {
    let dir = TempDir::new("blob-orphaning-reconcile");
    let db = common::store::open(&dir).await;
    projects::create(&db, "proj", "Project")
        .await
        .expect("create project");

    // Publish artifact v1 legitimately
    let art = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Legit Artifact",
            description: "",
            label: None,
            kind: "markdown",
            content: b"version 1 legit",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish v1");

    let art_dir = dir.join("artifacts").join("proj").join(&art.id);
    let legit_v1 = art_dir.join("v1.md");
    assert!(legit_v1.exists(), "v1.md must exist");

    // Plant an unreferenced version file v2.md (e.g. from an update that crashed after promotion)
    let orphan_v2 = art_dir.join("v2.md");
    std::fs::write(&orphan_v2, b"orphaned version 2 content").expect("write orphan v2");

    // Plant an unreferenced pending file pending-xyz.md
    let orphan_pending = art_dir.join("pending-dummy.md");
    std::fs::write(&orphan_pending, b"orphaned pending content").expect("write orphan pending");

    // Plant a completely orphaned artifact directory with an unreferenced file
    let orphan_art_dir = dir
        .join("artifacts")
        .join("proj")
        .join("orphaned-artifact-id");
    std::fs::create_dir_all(&orphan_art_dir).expect("create orphan art dir");
    let orphan_v1 = orphan_art_dir.join("v1.md");
    std::fs::write(&orphan_v1, b"orphaned artifact v1").expect("write orphan v1");

    // Run reconcile
    let reaped = agent_hub::blob::reconcile(&db, &dir)
        .await
        .expect("reconcile");

    // In unfixed code: reconcile did not exist or reap_pending only reaped pending-*,
    // leaving orphan_v2 and orphan_v1!
    assert!(legit_v1.exists(), "committed v1.md must NOT be removed");
    assert!(
        !orphan_v2.exists(),
        "unreferenced v2.md must be removed by reconcile"
    );
    assert!(!orphan_pending.exists(), "pending file must be removed");
    assert!(
        !orphan_v1.exists(),
        "unreferenced artifact file must be removed"
    );
    assert!(
        !orphan_art_dir.exists(),
        "empty orphan artifact dir must be pruned"
    );
    assert_eq!(
        reaped, 3,
        "must have reaped 3 orphaned files (pending, orphan v2, orphan v1)"
    );
}
