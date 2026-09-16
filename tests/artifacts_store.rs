//! Artifact tests: publish, version, protected envelope, indexing, and limits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::limits::ARTIFACT_BYTES_MAX;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::events::{FeedQuery, read_feed};
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
        kind: "html",
        content,
        envelope: None,
    }
}

#[tokio::test]
async fn publish_reads_back_and_lands_on_the_feed() {
    let dir = temp_dir("artifact");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"<h1>hits</h1>"))
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
    assert!(
        events
            .iter()
            .any(|event| event.kind == "artifact" && event.summary.contains("published")),
        "a publish event lands on the feed"
    );
}

#[tokio::test]
async fn update_adds_a_version_and_refreshes_search() {
    let dir = temp_dir("artifact-update");
    let db = open(&dir).await;
    let artifact = artifacts::publish(&db, &dir, public("Report", b"first draft"))
        .await
        .expect("publish");

    let updated = artifacts::update(&db, &dir, "agent-one", &artifact.id, b"second draft", None)
        .await
        .expect("update");
    assert_eq!(updated.version, 2);

    let (read, bytes) = artifacts::get(&db, &dir, &artifact.id).await.expect("get");
    assert_eq!(bytes, b"second draft");
    assert_eq!(read.version, 2);

    assert!(search_hits(&db, "draft").await >= 1);
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
        },
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
    let err = artifacts::publish(&db, &dir, public("Big", &big))
        .await
        .expect_err("too large");
    assert_eq!(err.code(), ErrorCode::PayloadTooLarge);
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
