//! Search tests: the query path over the corpus written by the other stores.

use agent_hub::error::ErrorCode;
use agent_hub::store::artifacts::{self, NewArtifact};
use agent_hub::store::events::{NewEvent, append};
use agent_hub::store::search::{self, SearchDoc, SearchQuery};

mod common;

use common::store::open;
use common::temp::TempDir;

async fn seed(db: &turso::Database, dir: &std::path::Path) {
    let event = NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: "engine groundwork".to_string(),
        payload: Some(serde_json::json!({"body": "the engine keeps session state"})),
        needs_action: false,
        thread_id: None,
        session_id: None,
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
            session_id: None,
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
        session_id: None,
        limit: 50,
    }
}

#[tokio::test]
async fn finds_feed_and_artifact_content() {
    let dir = TempDir::new("search");
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
async fn searching_deplo_finds_deploy_and_deployment() {
    let dir = TempDir::new("search-deplo");
    let db = open(&dir).await;
    plant(&db, "doc-deploy", "proj", "feed", "we deploy the server").await;
    plant(
        &db,
        "doc-deployment",
        "proj",
        "feed",
        "automated deployment pipeline",
    )
    .await;
    plant(&db, "doc-other", "proj", "feed", "nothing related here").await;

    let hits = search::query(&db, &q("deplo")).await.expect("search");
    let ids: Vec<&str> = hits.iter().map(|hit| hit.doc_id.as_str()).collect();
    assert!(
        ids.contains(&"doc-deploy"),
        "deplo finds deploy; got {ids:?}"
    );
    assert!(
        ids.contains(&"doc-deployment"),
        "deplo finds deployment; got {ids:?}"
    );
    assert_eq!(hits.len(), 2);
}

#[tokio::test]
async fn whole_word_match_ranks_above_prefix_only_match() {
    let dir = TempDir::new("search-rank-prefix");
    let db = open(&dir).await;
    plant(
        &db,
        "doc-deployment",
        "proj",
        "feed",
        "deployment pipeline running",
    )
    .await;
    plant(&db, "doc-deploy", "proj", "feed", "deploy the code now").await;

    let hits = search::query(&db, &q("deploy")).await.expect("search");
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].doc_id, "doc-deploy",
        "exact whole-word match ranks first"
    );
    assert_eq!(
        hits[1].doc_id, "doc-deployment",
        "prefix match ranks second"
    );
}

#[tokio::test]
async fn balanced_quoted_phrase_is_unchanged() {
    let dir = TempDir::new("search-phrase");
    let db = open(&dir).await;
    plant(&db, "doc-exact", "proj", "feed", "engine report summary").await;
    plant(
        &db,
        "doc-split",
        "proj",
        "feed",
        "engine is running a report",
    )
    .await;

    let hits = search::query(&db, &q("\"engine report\""))
        .await
        .expect("search");
    let ids: Vec<&str> = hits.iter().map(|hit| hit.doc_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["doc-exact"],
        "quoted phrase only matches exact phrase"
    );
}

#[tokio::test]
async fn filters_by_type_and_project() {
    let dir = TempDir::new("search-filter");
    let db = open(&dir).await;
    seed(&db, &dir).await;

    let artifacts = search::query(
        &db,
        &SearchQuery {
            text: "engine".to_string(),
            project_id: Some("proj".to_string()),
            kind: Some("artifact".to_string()),
            session_id: None,
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
            session_id: None,
            limit: 50,
        },
    )
    .await
    .expect("search");
    assert!(other_project.is_empty(), "project scope excludes the hits");
}

#[tokio::test]
async fn empty_query_is_rejected() {
    let dir = TempDir::new("search-empty");
    let db = open(&dir).await;
    let err = search::query(&db, &q("   ")).await.expect_err("reject");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn unknown_type_is_rejected() {
    let dir = TempDir::new("search-type");
    let db = open(&dir).await;
    let err = search::query(
        &db,
        &SearchQuery {
            text: "engine".to_string(),
            project_id: None,
            kind: Some("nonsense".to_string()),
            session_id: None,
            limit: 50,
        },
    )
    .await
    .expect_err("reject");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn ranking_prefers_the_higher_term_frequency() {
    let dir = TempDir::new("search-rank");
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
            session_id: None,
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
            session_id: None,
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
    let dir = TempDir::new("search-scope-deep");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 600).await;
    plant(&db, "wanted", "quiet", "feed", "needle").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: Some("quiet".to_string()),
            kind: None,
            session_id: None,
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
    let dir = TempDir::new("search-type-deep");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 600).await;
    plant(&db, "wanted", "noisy", "brain", "needle").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: Some("brain".to_string()),
            session_id: None,
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
    let dir = TempDir::new("search-scope-rank");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 20).await;
    plant_rising(&db, "quiet", "feed").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: Some("quiet".to_string()),
            kind: None,
            session_id: None,
            limit: 50,
        },
    )
    .await
    .expect("scoped search");
    assert_eq!(order(&hits), RISING, "relevance orders a project scope");
}

#[tokio::test]
async fn a_type_scope_keeps_relevance_order() {
    let dir = TempDir::new("search-type-rank");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 20).await;
    plant_rising(&db, "noisy", "brain").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: Some("brain".to_string()),
            session_id: None,
            limit: 50,
        },
    )
    .await
    .expect("scoped search");
    assert_eq!(order(&hits), RISING, "relevance orders a family scope");
}

#[tokio::test]
async fn a_confined_search_keeps_relevance_order() {
    let dir = TempDir::new("search-confined-rank");
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
            session_id: None,
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
    let dir = TempDir::new("search-cap");
    let db = open(&dir).await;
    bury(&db, "noisy", "feed", 150).await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: None,
            session_id: None,
            limit: 500,
        },
    )
    .await
    .expect("search");
    assert_eq!(hits.len(), agent_hub::limits::SEARCH_LIMIT_MAX as usize);
}

#[tokio::test]
async fn the_knowledge_base_is_a_corpus_family_of_its_own() {
    let dir = TempDir::new("search-kb");
    let db = open(&dir).await;
    plant(&db, "kb:quiet:/fs/runbook.md", "quiet", "kb", "needle").await;
    plant(&db, "brain:one:/fs/note.md", "quiet", "brain", "needle").await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: Some("kb".to_string()),
            session_id: None,
            limit: 50,
        },
    )
    .await
    .expect("a knowledge base search");
    assert_eq!(
        hits.iter()
            .map(|hit| hit.doc_id.as_str())
            .collect::<Vec<_>>(),
        vec!["kb:quiet:/fs/runbook.md"],
        "the knowledge base family is searchable on its own"
    );
    assert!(
        hits[0].session_id.is_none(),
        "a knowledge base page belongs to no session"
    );
}

async fn plant_in_session(
    db: &turso::Database,
    doc_id: &str,
    project_id: &str,
    session_id: &str,
    body: &str,
) {
    let conn = db.connect().expect("connect");
    search::index_doc(
        &conn,
        SearchDoc {
            doc_id,
            project_id,
            kind: "brain",
            ref_id: doc_id,
            session_id: Some(session_id),
            title: Some("wanted"),
            body,
            updated_at: "2026-09-18T00:00:00Z",
        },
    )
    .await
    .expect("index a session entry");
}

#[tokio::test]
async fn a_session_scope_returns_only_that_session_in_relevance_order() {
    let dir = TempDir::new("search-session-scope");
    let db = open(&dir).await;
    bury(&db, "proj", "feed", 20).await;
    plant_in_session(
        &db,
        "other",
        "proj",
        "sibling",
        "needle needle needle needle needle",
    )
    .await;
    plant_in_session(&db, "third", "proj", "wanted-session", "needle").await;
    plant_in_session(&db, "second", "proj", "wanted-session", "needle needle").await;
    plant_in_session(
        &db,
        "first",
        "proj",
        "wanted-session",
        "needle needle needle needle",
    )
    .await;

    let hits = search::query(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: None,
            session_id: Some("wanted-session".to_string()),
            limit: 50,
        },
    )
    .await
    .expect("session scoped search");
    assert_eq!(
        order(&hits),
        RISING,
        "a session scope returns that session's entries in relevance order"
    );
}

#[tokio::test]
async fn a_session_scope_is_confined_like_every_other() {
    let dir = TempDir::new("search-session-confined");
    let db = open(&dir).await;
    // One session name, two projects: the reachable one answers and the other
    // does not, so the scope never widens what a caller may see.
    plant_in_session(&db, "hidden", "closed", "wanted-session", "needle").await;
    plant_in_session(&db, "shown", "open", "wanted-session", "needle").await;

    let visible = vec!["open".to_string()];
    let hits = search::query_visible(
        &db,
        &SearchQuery {
            text: "needle".to_string(),
            project_id: None,
            kind: None,
            session_id: Some("wanted-session".to_string()),
            limit: 50,
        },
        Some(&visible),
    )
    .await
    .expect("confined session search");
    assert_eq!(
        order(&hits),
        vec!["shown"],
        "a session in an unreachable project stays out of the results"
    );
}

#[tokio::test]
async fn hostile_search_queries_do_not_error() {
    let dir = TempDir::new("search-hostile");
    let db = open(&dir).await;
    seed(&db, &dir).await;

    for hostile in [
        "\"",
        "\"unclosed",
        "foo (bar",
        "<script>alert(1)</script>",
        "()",
        "AND OR NOT",
        ":*",
        "engine*",
        "\"engine report\"",
    ] {
        let results = search::query(
            &db,
            &SearchQuery {
                text: hostile.to_string(),
                project_id: None,
                kind: None,
                session_id: None,
                limit: 10,
            },
        )
        .await
        .expect("hostile search query should never return engine error");

        if hostile == "()" || hostile == "\"" || hostile == ":*" {
            assert!(
                results.is_empty(),
                "query with no terms returns empty for {hostile}"
            );
        }
    }
}

#[test]
fn a_made_safe_query_keeps_words_and_phrases_and_nothing_else() {
    use agent_hub::store::search::sanitize_fts;

    assert_eq!(
        sanitize_fts("engine state"),
        "(\"engine\"^2 OR title:[engine TO enginf} OR body:[engine TO enginf}) (\"state\"^2 OR title:[state TO statf} OR body:[state TO statf})"
    );
    assert_eq!(
        sanitize_fts("\"engine state\" notes"),
        "\"engine state\" (\"notes\"^2 OR title:[notes TO notet} OR body:[notes TO notet})"
    );
    // An unbalanced quote is no phrase; its words are still searched.
    assert_eq!(
        sanitize_fts("\"engine state"),
        "(\"engine\"^2 OR title:[engine TO enginf} OR body:[engine TO enginf}) (\"state\"^2 OR title:[state TO statf} OR body:[state TO statf})"
    );
    // The bare operators are not words the reader is looking for.
    assert_eq!(
        sanitize_fts("engine AND state OR NOT x"),
        "(\"engine\"^2 OR title:[engine TO enginf} OR body:[engine TO enginf}) (\"state\"^2 OR title:[state TO statf} OR body:[state TO statf}) (\"x\"^2 OR title:[x TO y} OR body:[x TO y})"
    );
    // Lower case they are ordinary words.
    assert_eq!(
        sanitize_fts("salt and pepper"),
        "(\"salt\"^2 OR title:[salt TO salu} OR body:[salt TO salu}) (\"and\"^2 OR title:[and TO ane} OR body:[and TO ane}) (\"pepper\"^2 OR title:[pepper TO peppes} OR body:[pepper TO peppes})"
    );
    // Punctuation alone is nothing to look up, inside a phrase or out of one.
    for nothing in ["- -", "__ ___", "\"-\"", "()*:", "   ", ""] {
        assert_eq!(sanitize_fts(nothing), "", "{nothing:?} is searchable");
    }
    // A word may carry them.
    assert_eq!(
        sanitize_fts("last-run snake_case"),
        "(\"last-run\"^2 OR \"last-run\"*) (\"snake_case\"^2 OR \"snake_case\"*)"
    );
    // However long the input, what reaches the engine stays under its limit.
    let long = sanitize_fts(&"ab ".repeat(10_000));
    assert!(long.len() <= agent_hub::limits::SEARCH_QUERY_BYTES_MAX);
    assert!(long.starts_with("(\"ab\"^2"));
}

async fn append_with(db: &turso::Database, summary: &str, payload: Option<serde_json::Value>) {
    append(
        db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: summary.to_string(),
            payload,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
}

async fn snippet_of(db: &turso::Database, text: &str) -> String {
    let hits = search::query(db, &q(text)).await.expect("search");
    assert_eq!(hits.len(), 1, "one event matches {text}: {hits:?}");
    hits[0].snippet.clone()
}

#[tokio::test]
async fn a_feed_snippet_is_the_words_an_agent_wrote_never_serialized_json() {
    let dir = TempDir::new("search-snippet");
    let db = open(&dir).await;

    append_with(
        &db,
        "kettle report",
        Some(serde_json::json!({"body": "the kettle is on the stove", "step": 3})),
    )
    .await;
    assert_eq!(
        snippet_of(&db, "kettle").await,
        "the kettle is on the stove",
        "a string body is the snippet"
    );

    // Found by a word that is only in the payload, which still matches; with
    // no string body to show, the snippet is the summary.
    append_with(
        &db,
        "lantern lit",
        Some(serde_json::json!({"context": "the harbour wall", "count": 2})),
    )
    .await;
    assert_eq!(snippet_of(&db, "harbour").await, "lantern lit");

    append_with(
        &db,
        "anchor weighed",
        Some(serde_json::json!({"body": {"depth": "twelve fathoms"}})),
    )
    .await;
    assert_eq!(
        snippet_of(&db, "fathoms").await,
        "anchor weighed",
        "a body that is not a string is not shown"
    );

    append_with(&db, "sails mended", None).await;
    assert_eq!(snippet_of(&db, "mended").await, "sails mended");

    let long = "rope ".repeat(80);
    append_with(&db, "rigging", Some(serde_json::json!({"body": long}))).await;
    let cut = snippet_of(&db, "rigging").await;
    assert!(cut.starts_with("rope rope"), "{cut}");
    assert_eq!(
        cut.chars().count(),
        201,
        "two hundred characters and a mark"
    );
}

/// A store written before the snippet changed holds the same corpus rows, so
/// it needs no rebuild: the row still carries the serialized payload, which is
/// what keeps every payload word searchable, and the snippet no longer reads
/// from it.
#[tokio::test]
async fn a_corpus_row_written_the_old_way_reads_as_a_clean_snippet() {
    let dir = TempDir::new("search-snippet-old");
    let db = open(&dir).await;
    append_with(
        &db,
        "kettle report",
        Some(serde_json::json!({"body": "the kettle is on the stove", "step": 3})),
    )
    .await;

    let conn = db.connect().expect("connect");
    let mut rows = conn
        .query("SELECT body FROM search_docs WHERE type = 'feed'", ())
        .await
        .expect("read the corpus");
    let row = rows.next().await.expect("row").expect("one row");
    let stored: String = row.get(0).expect("body");
    assert!(
        stored.starts_with('{') && stored.contains("\"step\":3"),
        "the corpus row is the serialized payload, as it always was: {stored}"
    );
    drop(rows);

    let snippet = snippet_of(&db, "stove").await;
    assert_eq!(snippet, "the kettle is on the stove");
    assert!(!snippet.contains('{'), "no JSON in a snippet: {snippet}");
}

#[tokio::test]
async fn an_artifact_snippet_is_still_the_opening_of_its_content() {
    let dir = TempDir::new("search-snippet-artifact");
    let db = open(&dir).await;
    seed(&db, &dir).await;
    let hits = search::query(
        &db,
        &SearchQuery {
            kind: Some("artifact".to_string()),
            ..q("engine")
        },
    )
    .await
    .expect("search");
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].snippet.starts_with("# engine notes"),
        "{}",
        hits[0].snippet
    );
}
