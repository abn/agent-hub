//! Search tests: the query path over the corpus written by the other stores.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::events::{NewEvent, append};
use agent_hub::store::search::{self, SearchQuery};
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

async fn seed(db: &turso::Database, dir: &std::path::Path) {
    let mut event = NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: "engine groundwork".to_string(),
        payload: Some(serde_json::json!({"body": "the engine keeps session state"})),
        needs_action: false,
        thread_id: None,
    };
    event.summary = "engine groundwork".to_string();
    append(db, "agent-one", None, event).await.expect("append");

    artifacts::publish(
        db,
        dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "Engine report",
            kind: "markdown",
            content: b"# engine notes\nstate and search",
            envelope: None,
        },
    )
    .await
    .expect("publish");
}

fn q(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_string(),
        project_id: None,
        kind: None,
        limit: 50,
    }
}

#[tokio::test]
async fn finds_feed_and_artifact_content() {
    let dir = temp_dir("search");
    let db = open(&dir).await;
    seed(&db, &dir).await;

    let hits = search::query(&db, &q("engine")).await.expect("search");
    let kinds: Vec<&str> = hits.iter().map(|hit| hit.kind.as_str()).collect();
    assert!(kinds.contains(&"feed"), "a feed event matches");
    assert!(kinds.contains(&"artifact"), "an artifact matches");
    assert!(
        hits.iter().any(|hit| !hit.snippet.is_empty()),
        "hits carry a snippet"
    );
}

#[tokio::test]
async fn filters_by_type_and_project() {
    let dir = temp_dir("search-filter");
    let db = open(&dir).await;
    seed(&db, &dir).await;

    let artifacts = search::query(
        &db,
        &SearchQuery {
            text: "engine".to_string(),
            project_id: Some("proj".to_string()),
            kind: Some("artifact".to_string()),
            limit: 50,
        },
    )
    .await
    .expect("search");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, "artifact");

    let other_project = search::query(
        &db,
        &SearchQuery {
            text: "engine".to_string(),
            project_id: Some("elsewhere".to_string()),
            kind: None,
            limit: 50,
        },
    )
    .await
    .expect("search");
    assert!(other_project.is_empty(), "project scope excludes the hits");
}

#[tokio::test]
async fn empty_query_is_rejected() {
    let dir = temp_dir("search-empty");
    let db = open(&dir).await;
    let err = search::query(&db, &q("   ")).await.expect_err("reject");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn unknown_type_is_rejected() {
    let dir = temp_dir("search-type");
    let db = open(&dir).await;
    let err = search::query(
        &db,
        &SearchQuery {
            text: "engine".to_string(),
            project_id: None,
            kind: Some("nonsense".to_string()),
            limit: 50,
        },
    )
    .await
    .expect_err("reject");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}
