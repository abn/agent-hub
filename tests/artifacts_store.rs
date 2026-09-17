//! Artifact tests: publish, version, protected envelope, indexing, and limits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::limits::ARTIFACT_BYTES_MAX;
use agent_hub::store::artifacts::{self, NewArtifact, UpdateOptions};
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
            None,
            UpdateOptions::default(),
            None
        ),
        artifacts::update(
            &db,
            &dir,
            "agent-one",
            &artifact.id,
            b"third draft",
            None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        Some(rotated_envelope.clone()),
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
        None,
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
        None,
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
