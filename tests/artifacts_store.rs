//! Artifact tests: publish, version, protected envelope, indexing, and limits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::error::ErrorCode;
use agent_hub::limits::ARTIFACT_BYTES_MAX;
use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
use agent_hub::store::events::{FeedQuery, read_feed};
use agent_hub::store::projects;
use agent_hub::store::{migrate, open_engine};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-{tag}-{}-{nanos}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn open(dir: &std::path::Path) -> turso::Database {
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    migrate(&db).await.expect("migrate");
    db
}

fn public<'a>(title: &'a str, content: &'a [u8]) -> NewArtifact<'a> {
    NewArtifact {
        actor: "agent-one",
        project_id: "proj",
        title,
        description: "",
        favicon: "",
        label: None,
        kind: "html",
        content,
        envelope: None,
    }
}

#[tokio::test]
async fn publish_reads_back_and_lands_on_the_feed() {
    let dir = temp_dir("artifact");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"<h1>hits</h1>"), None)
        .await
        .expect("publish");
    assert_eq!(artifact.version, 1);
    assert!(!artifact.protected);

    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(read.id, artifact.id);
    assert_eq!(bytes, b"<h1>hits</h1>");

    let listed = artifacts::list(&db, "proj").await.expect("list");
    assert_eq!(listed.len(), 1);

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let published = events
        .iter()
        .find(|event| event.kind == "artifact" && event.summary.contains("published"))
        .expect("a publish event lands on the feed");
    let payload = published.payload.as_ref().expect("publish payload");
    assert_eq!(payload["artifact_id"], artifact.id);
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["protected"], false);
}

#[tokio::test]
async fn concurrent_updates_get_distinct_versions() {
    let dir = temp_dir("artifact-concurrent");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first draft"), None)
        .await
        .expect("publish");

    let (a, b) = tokio::join!(
        artifacts::update(
            &db,
            &dir,
            "agent-one",
            &artifact.id,
            b"second draft",
            EnvelopeUpdate::Keep,
            UpdateOptions::default(),
            None
        ),
        artifacts::update(
            &db,
            &dir,
            "agent-one",
            &artifact.id,
            b"third draft",
            EnvelopeUpdate::Keep,
            UpdateOptions::default(),
            None
        ),
    );

    let results = [a, b];
    assert!(
        results.iter().any(|result| result.is_ok()),
        "at least one concurrent update succeeds"
    );
    let mut versions: Vec<i64> = results
        .iter()
        .filter_map(|result| result.as_ref().ok().map(|artifact| artifact.version))
        .collect();
    versions.sort_unstable();
    // A concurrent update may lose the write lock and error; what matters is
    // that no two successful updates ever share a version.
    for (index, version) in versions.iter().enumerate() {
        assert_eq!(
            *version,
            2 + index as i64,
            "successful updates get distinct, sequential versions"
        );
    }

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let updated = events
        .iter()
        .filter(|event| event.kind == "artifact" && event.summary.contains("updated"))
        .count();
    assert_eq!(
        updated,
        versions.len(),
        "each granted version has one event"
    );
}

#[tokio::test]
async fn update_adds_a_version_and_refreshes_search() {
    let dir = temp_dir("artifact-update");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first draft"), None)
        .await
        .expect("publish");

    let updated = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second draft",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update");
    assert_eq!(updated.version, 2);

    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(bytes, b"second draft");
    assert_eq!(read.version, 2);

    assert!(search_hits(&db, "draft").await >= 1);

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert!(
        events.iter().any(|event| event.kind == "artifact"
            && event.summary.contains("updated")
            && event
                .payload
                .as_ref()
                .and_then(|payload| payload["version"].as_i64())
                == Some(2)),
        "an update event with the granted version lands on the feed"
    );
}

#[tokio::test]
async fn a_protected_artifact_is_not_searchable_by_body() {
    let dir = temp_dir("artifact-protected");
    let db = open(&dir).await;
    let envelope = serde_json::json!({"alg": "AES-GCM", "kdf": "PBKDF2-SHA256", "iterations": 600000, "salt": "c2FsdA==", "iv": "aXY="});
    let artifact = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Secret report",
            kind: "html",
            content: b"ciphertextbytes",
            envelope: Some(envelope.clone()),
            description: "",
            favicon: "",
            label: None,
        },
        None,
    )
    .await
    .expect("publish protected");

    assert!(artifact.protected);
    assert_eq!(artifact.envelope, Some(envelope));

    assert_eq!(
        search_hits(&db, "ciphertextbytes").await,
        0,
        "the ciphertext body is not indexed"
    );
    assert!(
        search_hits(&db, "Secret").await >= 1,
        "the title is indexed"
    );
}

#[tokio::test]
async fn an_over_cap_artifact_is_rejected() {
    let dir = temp_dir("artifact-cap");
    let db = open(&dir).await;
    let big = vec![0u8; ARTIFACT_BYTES_MAX + 1];
    let err = artifacts::publish(&db, &dir, public("Big", &big), None)
        .await
        .expect_err("too large");
    assert_eq!(err.code(), ErrorCode::PayloadTooLarge);
}

#[tokio::test]
async fn a_publish_replays_on_its_idempotency_key() {
    let dir = temp_dir("artifact-idem");
    let db = open(&dir).await;
    let first = artifacts::publish(&db, &dir, public("Report", b"draft"), Some("pub-key"))
        .await
        .expect("publish");
    let second = artifacts::publish(&db, &dir, public("Report", b"draft"), Some("pub-key"))
        .await
        .expect("replay");

    assert_eq!(first.id, second.id, "a retry returns the original artifact");
    assert_eq!(second.version, 1);

    let listed = artifacts::list(&db, "proj").await.expect("list");
    assert_eq!(listed.len(), 1, "a retry adds no artifact");

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let published = events
        .iter()
        .filter(|event| event.kind == "artifact" && event.summary.contains("published"))
        .count();
    assert_eq!(published, 1, "a retry appends no event");
}

#[tokio::test]
async fn an_update_replays_on_its_idempotency_key() {
    let dir = temp_dir("artifact-idem-update");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"draft"), None)
        .await
        .expect("publish");

    let first = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        Some("upd-key"),
    )
    .await
    .expect("update");
    let second = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"another",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        Some("upd-key"),
    )
    .await
    .expect("replay");

    assert_eq!(first.version, 2);
    assert_eq!(second.version, 2, "a retry returns the granted version");

    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(read.version, 2);
    assert_eq!(
        bytes, b"second",
        "a replayed update does not overwrite the blob"
    );

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    let updated = events
        .iter()
        .filter(|event| event.kind == "artifact" && event.summary.contains("updated"))
        .count();
    assert_eq!(updated, 1, "a retry appends no event");
}

async fn search_hits(db: &turso::Database, term: &str) -> usize {
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT doc_id FROM search_docs WHERE fts_match(title, ?1) OR fts_match(body, ?1)",
            vec![turso::Value::Text(term.to_string())],
        )
        .await
        .expect("search");
    let mut count = 0;
    while rows.next().await.expect("row").is_some() {
        count += 1;
    }
    count
}

#[tokio::test]
async fn publish_round_trips_display_metadata() {
    let dir = temp_dir("artifact-meta");
    let db = open(&dir).await;
    let artifact = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            description: "Quarterly numbers",
            favicon: "chart",
            label: Some("q3"),
            ..public("Report", b"<h1>hits</h1>")
        },
        None,
    )
    .await
    .expect("publish");
    assert_eq!(artifact.description, "Quarterly numbers");
    assert_eq!(artifact.favicon, "chart");
    assert_eq!(artifact.label.as_deref(), Some("q3"));

    let listed = artifacts::list(&db, "proj").await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].description, "Quarterly numbers");
    assert_eq!(listed[0].label.as_deref(), Some("q3"));

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].label.as_deref(), Some("q3"));
    assert_eq!(versions[0].description, "Quarterly numbers");
}

#[tokio::test]
async fn a_stale_base_version_conflicts_and_force_overwrites() {
    let dir = temp_dir("artifact-occ");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"v1"), None)
        .await
        .expect("publish");

    let stale = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"v2-stale",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            base_version: Some(999),
            force: false,
            label: None,
        },
        None,
    )
    .await
    .expect_err("stale base conflicts");
    assert_eq!(stale.code(), ErrorCode::Conflict);
    assert!(
        stale.to_string().contains("version 1"),
        "the conflict names the current version, got: {stale}"
    );

    let forced = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"v2-forced",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            base_version: Some(999),
            force: true,
            label: Some("forced"),
        },
        None,
    )
    .await
    .expect("force overwrites");
    assert_eq!(forced.version, 2);
    assert_eq!(forced.label.as_deref(), Some("forced"));

    let fresh = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"v3",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            base_version: Some(2),
            force: false,
            label: None,
        },
        None,
    )
    .await
    .expect("matching base applies");
    assert_eq!(fresh.version, 3);
    assert_eq!(
        fresh.label.as_deref(),
        Some("forced"),
        "no label keeps the existing one"
    );
}

#[tokio::test]
async fn a_version_read_returns_the_version_bytes_and_metadata() {
    let dir = temp_dir("artifact-version-read");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first"), None)
        .await
        .expect("publish");
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            base_version: None,
            force: false,
            label: Some("v2"),
        },
        None,
    )
    .await
    .expect("update");

    let (first, first_bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, 1)
        .await
        .expect("version 1");
    assert_eq!(first_bytes, b"first");
    assert_eq!(first.version, 1);
    assert_eq!(first.label, None);

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version, 1);
    assert_eq!(versions[1].version, 2);
    assert_eq!(versions[1].label.as_deref(), Some("v2"));
    assert_eq!(versions[1].title, "Report");

    let missing = artifacts::get_at_version(&db, &dir, &artifact.id, 99)
        .await
        .expect_err("unknown version");
    assert_eq!(missing.code(), ErrorCode::NotFound);
    let zero = artifacts::get_at_version(&db, &dir, &artifact.id, 0)
        .await
        .expect_err("version zero");
    assert_eq!(zero.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn versions_carry_their_own_envelopes() {
    let dir = temp_dir("artifact-envelopes");
    let db = open(&dir).await;
    let first_envelope = serde_json::json!({"alg": "AES-256-GCM", "kdf": "PBKDF2-HMAC-SHA256", "iterations": 600000, "salt": "c2FsdA==", "iv": "aXY="});
    let artifact = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            content: b"ciphertext-one",
            envelope: Some(first_envelope.clone()),
            ..public("Secret", b"")
        },
        None,
    )
    .await
    .expect("publish protected");
    assert!(artifact.protected);

    let rotated_envelope = serde_json::json!({"alg": "AES-256-GCM", "kdf": "PBKDF2-HMAC-SHA256", "iterations": 600000, "salt": "bmV3c2FsdA==", "iv": "bmV3aXY="});
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"ciphertext-two",
        EnvelopeUpdate::Set(rotated_envelope.clone()),
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update with rotated envelope");

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].envelope, Some(first_envelope));
    assert_eq!(versions[1].envelope, Some(rotated_envelope));
    assert!(versions.iter().all(|version| version.protected));
}

#[tokio::test]
async fn delete_removes_history_blobs_and_index_then_replays_fresh() {
    let dir = temp_dir("artifact-delete");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"draft"), Some("del-key"))
        .await
        .expect("publish");
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update");

    let deleted = artifacts::delete(&db, &dir, "agent-one", &artifact.id)
        .await
        .expect("delete");
    assert_eq!(deleted.id, artifact.id);

    assert!(
        artifacts::metadata(&db, &artifact.id).await.is_err(),
        "metadata is gone"
    );
    assert!(
        artifacts::list_versions(&db, &artifact.id).await.is_err(),
        "history is gone"
    );
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT version FROM artifact_versions WHERE artifact_id = ?1",
            vec![turso::Value::Text(artifact.id.clone())],
        )
        .await
        .expect("query versions");
    assert!(
        rows.next().await.expect("row").is_none(),
        "no version rows survive the delete"
    );
    drop(rows);
    assert!(
        !dir.join(format!("artifacts/proj/{}", artifact.id)).exists(),
        "the blob tree is gone"
    );
    // The feed keeps its published/updated/deleted events, so assert on the
    // artifact's own index row rather than on the shared term.
    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT doc_id FROM search_docs WHERE doc_id = ?1",
            vec![turso::Value::Text(format!("artifact:{}", artifact.id))],
        )
        .await
        .expect("query index");
    assert!(
        rows.next().await.expect("row").is_none(),
        "the artifact index row is gone"
    );
    drop(rows);

    let events = read_feed(&db, "proj", &FeedQuery::default())
        .await
        .expect("feed")
        .events;
    assert!(
        events
            .iter()
            .any(|event| event.kind == "artifact" && event.summary.contains("deleted")),
        "a deleted event lands on the feed"
    );

    // A replay after the delete records fresh instead of resolving to the
    // missing row.
    let republished = artifacts::publish(&db, &dir, public("Report", b"draft"), Some("del-key"))
        .await
        .expect("replay after delete records fresh");
    assert_ne!(
        republished.id, artifact.id,
        "the replay mints a new artifact"
    );
}

#[tokio::test]
async fn validation_rejects_bad_metadata() {
    let dir = temp_dir("artifact-validation");
    let db = open(&dir).await;

    let empty = artifacts::publish(&db, &dir, public("   ", b"body"), None).await;
    assert_eq!(
        empty.expect_err("blank title").code(),
        ErrorCode::InvalidArgument
    );

    let long_title = "t".repeat(501);
    let err = artifacts::publish(&db, &dir, public(&long_title, b"body"), None)
        .await
        .expect_err("long title");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);

    let long_description = "d".repeat(2001);
    let err = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            description: &long_description,
            ..public("Report", b"body")
        },
        None,
    )
    .await
    .expect_err("long description");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);

    let err = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            favicon: "too-many-emoji-here",
            ..public("Report", b"body")
        },
        None,
    )
    .await
    .expect_err("long favicon");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);

    let long_label = "l".repeat(61);
    let err = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            label: Some(&long_label),
            ..public("Report", b"body")
        },
        None,
    )
    .await
    .expect_err("long label");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);

    let err = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            kind: "pdf",
            ..public("Report", b"body")
        },
        None,
    )
    .await
    .expect_err("unknown kind");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn a_blank_markdown_title_falls_back_to_the_first_heading() {
    let dir = temp_dir("artifact-title-fallback");
    let db = open(&dir).await;
    let artifact = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            title: "",
            kind: "markdown",
            content: b"intro\n\n# Real Title\n\nbody",
            description: "",
            favicon: "",
            label: None,
            actor: "agent-one",
            project_id: "proj",
            envelope: None,
        },
        None,
    )
    .await
    .expect("heading fallback");
    assert_eq!(artifact.title, "Real Title");
}

#[tokio::test]
async fn a_conflicting_update_leaves_no_blob_behind() {
    let dir = temp_dir("artifact-conflict-blob");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"v1"), None)
        .await
        .expect("publish");

    let stale = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"v2-stale",
        EnvelopeUpdate::Keep,
        UpdateOptions {
            base_version: Some(999),
            force: false,
            label: None,
        },
        None,
    )
    .await
    .expect_err("stale base conflicts");
    assert_eq!(stale.code(), ErrorCode::Conflict);

    assert_eq!(
        blob_files(&dir),
        vec![format!("proj/{}/v1.html", artifact.id)],
        "a refused update leaves nothing on the volume"
    );
}

#[tokio::test]
async fn a_replayed_write_leaves_no_extra_blob() {
    let dir = temp_dir("artifact-replay-blob");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"draft"), Some("pub-key"))
        .await
        .expect("publish");
    artifacts::publish(&db, &dir, public("Report", b"draft"), Some("pub-key"))
        .await
        .expect("publish replay");
    assert_eq!(
        blob_files(&dir),
        vec![format!("proj/{}/v1.html", artifact.id)],
        "a replayed publish writes no second blob"
    );

    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        Some("upd-key"),
    )
    .await
    .expect("update");
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"another",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        Some("upd-key"),
    )
    .await
    .expect("update replay");
    assert_eq!(
        blob_files(&dir),
        vec![
            format!("proj/{}/v1.html", artifact.id),
            format!("proj/{}/v2.html", artifact.id),
        ],
        "a replayed update writes no third blob"
    );
}

#[tokio::test]
async fn sequential_updates_keep_every_version_blob() {
    let dir = temp_dir("artifact-version-blobs");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first"), None)
        .await
        .expect("publish");
    let second = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("second");
    let third = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"third",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("third");
    assert_eq!(second.version, 2);
    assert_eq!(third.version, 3);

    for (version, expected) in [(1, &b"first"[..]), (2, b"second"), (3, b"third")] {
        let (read, bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, version)
            .await
            .expect("version read");
        assert_eq!(read.version, version);
        assert_eq!(bytes, expected, "version {version} keeps its own bytes");
    }
    assert_eq!(
        blob_files(&dir),
        vec![
            format!("proj/{}/v1.html", artifact.id),
            format!("proj/{}/v2.html", artifact.id),
            format!("proj/{}/v3.html", artifact.id),
        ],
        "each version has one blob and nothing else is left over"
    );
}

#[tokio::test]
async fn publish_and_update_return_the_metadata_they_committed() {
    let dir = temp_dir("artifact-committed-metadata");
    let db = open(&dir).await;
    let published = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            description: "Quarterly numbers",
            favicon: "chart",
            label: Some("q3"),
            ..public("Report", b"<h1>hits</h1>")
        },
        None,
    )
    .await
    .expect("publish");
    let stored = artifacts::metadata(&db, &published.id)
        .await
        .expect("stored metadata");
    assert_eq!(
        serde_json::to_value(&published).expect("published json"),
        serde_json::to_value(&stored).expect("stored json"),
        "publish returns the metadata it committed"
    );
    assert_eq!(published.path, stored.path);
    assert_eq!(
        published.path,
        format!("artifacts/proj/{}/v1.html", stored.id)
    );

    let updated = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &published.id,
        b"<h1>more hits</h1>",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update");
    let stored = artifacts::metadata(&db, &published.id)
        .await
        .expect("stored metadata");
    assert_eq!(
        serde_json::to_value(&updated).expect("updated json"),
        serde_json::to_value(&stored).expect("stored json"),
        "update returns the metadata it committed"
    );
    assert_eq!(updated.path, stored.path);
    assert_eq!(updated.size_bytes, b"<h1>more hits</h1>".len() as i64);
    assert_eq!(updated.created_at, published.created_at);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_writes_its_blob_before_taking_the_write_lock() {
    let dir = temp_dir("artifact-lock-free-write");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first"), None)
        .await
        .expect("publish");

    let mut holder = db.connect().expect("connect");
    let lock = holder
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .expect("take the write lock");

    let updating = tokio::spawn({
        let db = db.clone();
        let dir = dir.clone();
        let id = artifact.id.clone();
        async move {
            artifacts::update(
                &db,
                &dir,
                "agent-one",
                &id,
                b"second",
                EnvelopeUpdate::Keep,
                UpdateOptions::default(),
                None,
            )
            .await
        }
    });

    // Well inside the five second lock wait: the content is on the volume
    // while another writer still holds the lock, so the transfer never
    // blocks the store.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let written: Vec<String> = blob_files(&dir)
        .into_iter()
        .filter(|name| !name.ends_with("v1.html"))
        .collect();
    lock.rollback().await.expect("release the write lock");
    assert_eq!(
        written.len(),
        1,
        "the update writes its blob before it waits for the lock, found {written:?}"
    );

    let updated = updating
        .await
        .expect("join")
        .expect("update once the lock is free");
    assert_eq!(updated.version, 2);
    assert_eq!(
        blob_files(&dir),
        vec![
            format!("proj/{}/v1.html", artifact.id),
            format!("proj/{}/v2.html", artifact.id),
        ],
        "the pending blob is renamed onto its version path"
    );
}

/// Every artifact blob on the volume, relative to the artifacts root, sorted.
fn blob_files(dir: &std::path::Path) -> Vec<String> {
    let root = dir.join("artifacts");
    let mut found = Vec::new();
    collect_files(&root, &root, &mut found);
    found.sort();
    found
}

fn collect_files(root: &std::path::Path, at: &std::path::Path, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(at) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, found);
        } else if let Ok(rel) = path.strip_prefix(root) {
            found.push(rel.to_string_lossy().into_owned());
        }
    }
}

#[tokio::test]
async fn project_delete_drops_version_rows() {
    let dir = temp_dir("artifact-project-delete");
    let db = open(&dir).await;
    projects::create(&db, "doomed", "Doomed")
        .await
        .expect("create");
    let artifact = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            project_id: "doomed",
            ..public("Report", b"v1")
        },
        None,
    )
    .await
    .expect("publish");
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"v2",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update");

    projects::delete(&db, &dir, "doomed").await.expect("delete");

    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT version FROM artifact_versions WHERE artifact_id = ?1",
            vec![turso::Value::Text(artifact.id.clone())],
        )
        .await
        .expect("query versions");
    assert!(
        rows.next().await.expect("row").is_none(),
        "no version rows survive the project"
    );
}

#[tokio::test]
async fn opening_the_hub_clears_content_left_by_an_interrupted_update() {
    let dir = temp_dir("artifact-pending");
    let db = open(&dir).await;
    let report = artifacts::publish(&db, &dir, public("Report", b"first draft"), None)
        .await
        .expect("publish");
    artifacts::update(
        &db,
        &dir,
        "agent-one",
        &report.id,
        b"second draft",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update");
    let notes = artifacts::publish(&db, &dir, public("Notes", b"notes"), None)
        .await
        .expect("publish notes");

    // Content an update wrote and never renamed onto a version path, in two
    // artifacts, plus two names the sweep must not touch.
    let root = dir.join("artifacts").join("proj");
    let stale = root.join(&report.id).join("pending-01J0STALE.html");
    let other_stale = root.join(&notes.id).join("pending-01J0OTHER.html");
    let named_pending = root.join(&report.id).join("v1-pending-review.html");
    let outside = dir.join("artifacts").join("pending-01J0LOOSE.html");
    for path in [&stale, &other_stale, &named_pending, &outside] {
        std::fs::write(path, b"interrupted").expect("write file");
    }
    drop(db);

    let state = AppState::open(Config {
        data_dir: dir.clone(),
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
    })
    .await
    .expect("open state");

    assert!(!stale.exists(), "content with no version left on disk");
    assert!(
        !other_stale.exists(),
        "content with no version left in a second artifact"
    );
    assert!(
        named_pending.exists(),
        "a file that only carries the word in its name was removed"
    );
    assert!(
        outside.exists(),
        "a file outside an artifact directory was removed"
    );

    let versions = root.join(&report.id);
    assert_eq!(
        std::fs::read(versions.join("v1.html")).expect("read v1"),
        b"first draft"
    );
    assert_eq!(
        std::fs::read(versions.join("v2.html")).expect("read v2"),
        b"second draft"
    );
    let (current, bytes) = artifacts::get(&state.db, &state.data_dir, &report.id)
        .await
        .expect("get");
    assert_eq!(current.version, 2);
    assert_eq!(bytes, b"second draft");
    let (_, bytes) = artifacts::get(&state.db, &state.data_dir, &notes.id)
        .await
        .expect("get notes");
    assert_eq!(bytes, b"notes");
}

fn envelope() -> serde_json::Value {
    serde_json::json!({
        "alg": "AES-256-GCM",
        "kdf": "PBKDF2-HMAC-SHA256",
        "iterations": 600000,
        "salt": "c2FsdA==",
        "iv": "aXY=",
    })
}

fn protected<'a>(title: &'a str, content: &'a [u8]) -> NewArtifact<'a> {
    NewArtifact {
        envelope: Some(envelope()),
        ..public(title, content)
    }
}

/// A project at one artifact password policy.
async fn project_at(db: &turso::Database, policy: &str) {
    projects::create(db, "proj", "Proj")
        .await
        .expect("create project");
    set_policy(db, policy).await;
}

async fn set_policy(db: &turso::Database, policy: &str) {
    projects::update(
        db,
        "proj",
        projects::ProjectChanges {
            artifact_password_policy: Some(policy),
            ..Default::default()
        },
    )
    .await
    .expect("set the policy");
}

#[tokio::test]
async fn a_project_that_requires_protection_refuses_plain_content() {
    let dir = temp_dir("artifact-required");
    let db = open(&dir).await;
    project_at(&db, "required").await;

    let refused = artifacts::publish(&db, &dir, public("Report", b"in the clear"), None)
        .await
        .expect_err("a plain publish is refused");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);
    assert!(
        refused.to_string().contains("envelope"),
        "the refusal says what to send instead: {refused}"
    );
    assert!(
        artifacts::list(&db, "proj").await.expect("list").is_empty(),
        "a refused publish writes nothing"
    );

    artifacts::publish(&db, &dir, protected("Report", b"ciphertext"), None)
        .await
        .expect("a protected publish is accepted");
}

#[tokio::test]
async fn a_project_with_protection_off_refuses_an_envelope() {
    let dir = temp_dir("artifact-off");
    let db = open(&dir).await;
    project_at(&db, "off").await;

    let refused = artifacts::publish(&db, &dir, protected("Secret", b"ciphertext"), None)
        .await
        .expect_err("a protected publish is refused");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);
    assert!(
        refused.to_string().contains("envelope"),
        "the refusal names the envelope: {refused}"
    );

    artifacts::publish(&db, &dir, public("Report", b"in the clear"), None)
        .await
        .expect("a plain publish is accepted");
}

#[tokio::test]
async fn an_optional_policy_takes_either_kind() {
    let dir = temp_dir("artifact-optional");
    let db = open(&dir).await;
    project_at(&db, "optional").await;

    artifacts::publish(&db, &dir, public("Report", b"in the clear"), None)
        .await
        .expect("a plain publish is accepted");
    artifacts::publish(&db, &dir, protected("Secret", b"ciphertext"), None)
        .await
        .expect("a protected publish is accepted");
}

#[tokio::test]
async fn a_policy_change_applies_to_the_next_version_only() {
    let dir = temp_dir("artifact-policy-change");
    let db = open(&dir).await;
    project_at(&db, "optional").await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first draft"), None)
        .await
        .expect("publish");

    set_policy(&db, "required").await;

    // What was published stays published and stays readable.
    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(bytes, b"first draft");
    assert!(!read.protected);

    let refused = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"second draft",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect_err("a plain new version is refused");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);

    let updated = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"ciphertext",
        EnvelopeUpdate::Set(envelope()),
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("a protected new version is accepted");
    assert_eq!(updated.version, 2);
    assert!(updated.protected);

    // The first version is still what it was, in the clear.
    let (first, bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, 1)
        .await
        .expect("read version one");
    assert!(!first.protected);
    assert_eq!(bytes, b"first draft");
}

#[tokio::test]
async fn turning_protection_off_holds_for_a_new_version_of_a_protected_artifact() {
    let dir = temp_dir("artifact-policy-off-change");
    let db = open(&dir).await;
    project_at(&db, "optional").await;
    let artifact = artifacts::publish(&db, &dir, protected("Secret", b"ciphertext"), None)
        .await
        .expect("publish");

    set_policy(&db, "off").await;

    // An update carries the envelope forward unless it is given a new one, so
    // the version it would write is still protected, and the project no longer
    // takes one.
    let refused = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"more ciphertext",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect_err("a protected new version is refused");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);

    // The artifact itself is untouched: what was published stays published.
    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert!(read.protected);
    assert_eq!(bytes, b"ciphertext");
}

#[tokio::test]
async fn an_update_publishes_a_version_in_the_clear_when_it_says_so() {
    let dir = temp_dir("artifact-clear");
    let db = open(&dir).await;
    project_at(&db, "optional").await;
    let artifact = artifacts::publish(&db, &dir, protected("Secret", b"ciphertext"), None)
        .await
        .expect("publish");

    let inherited = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"more ciphertext",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("an update carries the envelope forward");
    assert!(inherited.protected, "saying nothing keeps the protection");

    let cleared = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"in the clear",
        EnvelopeUpdate::Clear,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("an update can say the new version is not protected");
    assert_eq!(cleared.version, 3);
    assert!(!cleared.protected);
    assert!(cleared.envelope.is_none());

    // The history stays coherent: each version is what it was published as.
    let (first, bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, 1)
        .await
        .expect("version one");
    assert!(first.protected, "the first version is still ciphertext");
    assert_eq!(bytes, b"ciphertext");
    let (third, bytes) = artifacts::get_at_version(&db, &dir, &artifact.id, 3)
        .await
        .expect("version three");
    assert!(!third.protected);
    assert_eq!(bytes, b"in the clear");

    let versions = artifacts::list_versions(&db, &artifact.id)
        .await
        .expect("versions");
    assert_eq!(
        versions
            .iter()
            .map(|version| version.protected)
            .collect::<Vec<_>>(),
        vec![true, true, false]
    );
}

#[tokio::test]
async fn protection_off_names_the_way_to_publish_in_the_clear() {
    let dir = temp_dir("artifact-off-remedy");
    let db = open(&dir).await;
    project_at(&db, "optional").await;
    let artifact = artifacts::publish(&db, &dir, protected("Secret", b"ciphertext"), None)
        .await
        .expect("publish");
    set_policy(&db, "off").await;

    let refused = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"more ciphertext",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect_err("the version it would write is still protected");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);
    assert!(
        refused.to_string().contains("envelope: null"),
        "the refusal names a request the agent can actually make: {refused}"
    );

    let cleared = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"in the clear",
        EnvelopeUpdate::Clear,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("and that request is accepted");
    assert!(!cleared.protected);
    assert_eq!(cleared.version, 2);
}

#[tokio::test]
async fn required_protection_refuses_an_update_that_clears_it() {
    let dir = temp_dir("artifact-required-clear");
    let db = open(&dir).await;
    project_at(&db, "required").await;
    let artifact = artifacts::publish(&db, &dir, protected("Secret", b"ciphertext"), None)
        .await
        .expect("publish");

    let refused = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"in the clear",
        EnvelopeUpdate::Clear,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect_err("the project requires protection");
    assert_eq!(refused.code(), ErrorCode::InvalidArgument);
    assert!(refused.to_string().contains("envelope"));

    let kept = artifacts::update(
        &db,
        &dir,
        "agent-one",
        &artifact.id,
        b"more ciphertext",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("carrying the envelope forward is accepted");
    assert!(kept.protected);
}
