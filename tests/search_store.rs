//! Search tests: the query path over the corpus written by the other stores.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::error::ErrorCode;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::events::{NewEvent, append};
use agent_hub::store::search::{self, SearchDoc, SearchQuery};
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
    let event = NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: "engine groundwork".to_string(),
        payload: Some(serde_json::json!({"body": "the engine keeps session state"})),
        needs_action: false,
        thread_id: None,
    };
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
            description: "",
            favicon: "",
            label: None,
        },
        None,
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

#[tokio::test]
async fn ranking_prefers_the_higher_term_frequency() {
    let dir = temp_dir("search-rank");
    let db = open(&dir).await;

    // Older, but mentions the term three times.
    append(
        &db,
        "a",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "older".to_string(),
            payload: Some(serde_json::json!({"body": "engine engine engine alpha"})),
            needs_action: false,
            thread_id: None,
        },
    )
    .await
    .expect("older append");

    // Newer, but mentions the term once.
    append(
        &db,
        "a",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "newer".to_string(),
            payload: Some(serde_json::json!({"body": "engine beta"})),
            needs_action: false,
            thread_id: None,
        },
    )
    .await
    .expect("newer append");

    let hits = search::query(&db, &q("engine")).await.expect("search");
    assert_eq!(hits.len(), 2);
    assert!(
        hits[0].snippet.contains("alpha"),
        "the higher-scoring document ranks first, got {:?}",
        hits[0].snippet
    );
}

/// Fill the corpus with documents that outrank everything the test cares
/// about, so the wanted document sits far below any ranked prefix.
async fn bury(db: &turso::Database, project_id: &str, kind: &str, count: usize) {
    let conn = db.connect().expect("connect");
    for index in 0..count {
        let doc_id = format!("noise:{project_id}:{kind}:{index}");
        search::index_doc(
            &conn,
            SearchDoc {
                doc_id: &doc_id,
                project_id,
                kind,
                ref_id: &doc_id,
                session_id: None,
                title: Some("noise"),
                body: "needle needle needle needle needle",
                updated_at: "2026-09-18T00:00:00Z",
            },
        )
        .await
        .expect("index noise");
    }
}

async fn plant(db: &turso::Database, doc_id: &str, project_id: &str, kind: &str, body: &str) {
    let conn = db.connect().expect("connect");
    search::index_doc(
        &conn,
        SearchDoc {
            doc_id,
            project_id,
            kind,
            ref_id: doc_id,
            session_id: None,
            title: Some("wanted"),
            body,
            updated_at: "2026-09-18T00:00:00Z",
        },
    )
    .await
    .expect("index wanted");
}

#[tokio::test]
async fn a_project_scope_reaches_below_the_ranked_prefix() {
    let dir = temp_dir("search-scope-deep");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 600).await;
    plant(&db, "wanted", "quiet", "feed", "needle").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: Some("quiet".to_string()),
            kind: None,
            limit: 50,
        },
    )
    .await
    .expect("scoped search");
    assert_eq!(hits.len(), 1, "the scoped project's only match is returned");
    assert_eq!(hits[0].doc_id, "wanted");
}

#[tokio::test]
async fn a_type_scope_reaches_below_the_ranked_prefix() {
    let dir = temp_dir("search-type-deep");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 600).await;
    plant(&db, "wanted", "noisy", "brain", "needle").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: Some("brain".to_string()),
            limit: 50,
        },
    )
    .await
    .expect("scoped search");
    assert_eq!(hits.len(), 1, "the scoped family's only match is returned");
    assert_eq!(hits[0].doc_id, "wanted");
}

/// Index three documents of rising relevance in rising order, so the wanted
/// ranking is the exact reverse of the write order. A ranking that has
/// silently collapsed to a constant score keeps the write order and fails.
async fn plant_rising(db: &turso::Database, project_id: &str, kind: &str) {
    plant(db, "third", project_id, kind, "needle").await;
    plant(db, "second", project_id, kind, "needle needle").await;
    plant(db, "first", project_id, kind, "needle needle needle needle").await;
}

const RISING: [&str; 3] = ["first", "second", "third"];

fn order(hits: &[search::SearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.doc_id.as_str()).collect()
}

#[tokio::test]
async fn a_project_scope_keeps_relevance_order() {
    let dir = temp_dir("search-scope-rank");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 20).await;
    plant_rising(&db, "quiet", "feed").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: Some("quiet".to_string()),
            kind: None,
            limit: 50,
        },
    )
    .await
    .expect("scoped search");
    assert_eq!(order(&hits), RISING, "relevance orders a project scope");
}

#[tokio::test]
async fn a_type_scope_keeps_relevance_order() {
    let dir = temp_dir("search-type-rank");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 20).await;
    plant_rising(&db, "noisy", "brain").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: Some("brain".to_string()),
            limit: 50,
        },
    )
    .await
    .expect("scoped search");
    assert_eq!(order(&hits), RISING, "relevance orders a family scope");
}

#[tokio::test]
async fn a_confined_search_keeps_relevance_order() {
    let dir = temp_dir("search-confined-rank");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 20).await;
    plant_rising(&db, "quiet", "feed").await;

    let visible = vec!["quiet".to_string()];
    let hits = search::query_visible(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: None,
            limit: 50,
        },
        Some(&visible),
    )
    .await
    .expect("confined search");
    assert_eq!(order(&hits), RISING, "relevance orders a confined page");
}

#[tokio::test]
async fn the_page_is_capped_at_the_search_limit() {
    let dir = temp_dir("search-cap");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 150).await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: None,
            limit: 500,
        },
    )
    .await
    .expect("search");
    assert_eq!(hits.len(), agent_hub::limits::SEARCH_LIMIT_MAX as usize);
}
