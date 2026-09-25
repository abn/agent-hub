//! The search route: auth, results, and query validation.

use agent_hub::app::AppState;
use agent_hub::http::router;
use agent_hub::store::events::{NewEvent, append};
use axum::http::StatusCode;
use tower::ServiceExt;

mod common;

use common::http::{get, json_body};
use common::state::TestState;

async fn state() -> TestState {
    let state = common::state::open("search-http").await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Engine Room").await;
    state
}

async fn seed(state: &AppState) {
    append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "signal".to_string(),
            summary: "engine groundwork".to_string(),
            payload: Some(serde_json::json!({"body": "the engine keeps session state"})),
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
}

#[tokio::test]
async fn search_returns_hits_with_a_token() {
    let state = state().await;
    seed(&state).await;
    let app = router(state.clone());

    let response = app
        .oneshot(get("/api/v1/search?q=engine", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let groups = body["groups"].as_array().expect("groups");
    assert!(
        groups.iter().any(|group| group["kind"] == "feed"),
        "the seeded signal is found"
    );
}

#[tokio::test]
async fn search_requires_a_token() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/search?q=engine", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn search_requires_a_query() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/search", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_rejects_an_unknown_type() {
    let state = state().await;
    let app = router(state.clone());
    let response = app
        .oneshot(get(
            "/api/v1/search?q=engine&type=nonsense",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn results_carry_the_hit_count_and_how_long_the_query_took() {
    let state = state().await;
    // Three hits in two families, so the total is not any one group's count.
    for summary in ["needle one", "needle two"] {
        append(
            &state.db,
            "agent-one",
            None,
            NewEvent {
                project_id: "proj".to_string(),
                kind: "signal".to_string(),
                summary: summary.to_string(),
                payload: None,
                needs_action: false,
                thread_id: None,
                session_id: None,
            },
        )
        .await
        .expect("append");
    }
    let conn = state.db.connect().expect("connect");
    conn.execute(
        "INSERT INTO search_docs(doc_id, project_id, type, ref_id, session_id, title, body, updated_at) \
         VALUES ('brain:one', 'proj', 'brain', '/kv/note', 'sess', 'a note', 'needle three', '2026-09-16T00:00:00Z')",
        (),
    )
    .await
    .expect("index a brain entry");

    let app = router(state.clone());
    let body = json_body(
        app.oneshot(get("/api/v1/search?q=needle", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;

    let groups = body["groups"].as_array().expect("groups");
    let grouped: i64 = groups
        .iter()
        .map(|group| group["hits"].as_array().expect("hits").len() as i64)
        .sum();
    assert_eq!(body["count"], 3, "every hit is counted: {body}");
    assert_eq!(
        body["count"].as_i64().expect("count"),
        grouped,
        "the total is the hits across the groups"
    );
    for group in groups {
        assert_eq!(
            group["count"].as_i64().expect("group count"),
            group["hits"].as_array().expect("hits").len() as i64,
            "a group header counts its own hits"
        );
    }
    assert!(
        body["took_ms"].as_u64().is_some(),
        "the results line says how long the query took: {body}"
    );

    // A warm query answers the same way, timing included.
    let app = router(state.clone());
    let again = json_body(
        app.oneshot(get("/api/v1/search?q=needle", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;
    assert_eq!(again["count"], 3);
    assert!(again["took_ms"].as_u64().is_some());
}

#[tokio::test]
async fn a_search_that_finds_nothing_counts_nothing() {
    let state = state().await;
    let app = router(state.clone());
    let body = json_body(
        app.oneshot(get("/api/v1/search?q=nothing", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;
    assert_eq!(body["count"], 0);
    assert_eq!(body["groups"].as_array().expect("groups").len(), 0);
    assert!(body["took_ms"].as_u64().is_some());
}

#[tokio::test]
async fn a_capped_result_page_says_it_was_capped() {
    let state = state().await;
    let conn = state.db.connect().expect("connect");
    for index in 0..6 {
        conn.execute(
            "INSERT INTO search_docs(doc_id, project_id, type, ref_id, session_id, title, body, updated_at) \
             VALUES (?1, 'proj', 'brain', '/kv/note', 'sess', 'a note', 'needle here', '2026-09-16T00:00:00Z')",
            [format!("brain:{index}")],
        )
        .await
        .expect("index a brain entry");
    }

    // Under the limit: the count is every hit there is, and nothing was cut.
    let app = router(state.clone());
    let whole = json_body(
        app.oneshot(get("/api/v1/search?q=needle&limit=6", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;
    assert_eq!(whole["count"], 6);
    assert_eq!(
        whole["truncated"], false,
        "a page that holds everything is not capped: {whole}"
    );

    // At the boundary: six hits, five asked for. The count is what came back,
    // and the flag is what stops it reading as a total.
    let app = router(state.clone());
    let capped = json_body(
        app.oneshot(get("/api/v1/search?q=needle&limit=5", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await;
    assert_eq!(capped["count"], 5, "the count is the page: {capped}");
    assert_eq!(
        capped["groups"][0]["hits"].as_array().expect("hits").len(),
        5,
        "the limit is still honoured"
    );
    assert_eq!(
        capped["truncated"], true,
        "a capped page says so rather than reading as a total: {capped}"
    );
}

#[tokio::test]
async fn search_query_decoding_handles_percent_encoding_consistently() {
    let state = state().await;
    seed(&state).await;

    let app = router(state.clone());
    let res = app
        .oneshot(get(
            "/api/v1/search?q=engine%20groundwork",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["count"], 1);
}

#[tokio::test]
async fn search_tolerates_hostile_syntax() {
    let state = state().await;
    seed(&state).await;

    let hostile_queries = [
        "engine AND (OR NOT",
        "NEAR/3",
        "\"",
        "\"engine",
        "\"*\"",
        "foo:bar",
        // Punctuation the term scanner keeps: a term of nothing but these is
        // not a word the index can look up.
        "- -",
        "__ ___",
        "-",
        "_",
    ];
    // Quoting adds bytes to every term, so a query that was under the engine's
    // own limit must still be under it once it has been made safe.
    let long = "ab ".repeat(3400);
    let hostile_queries: Vec<&str> = hostile_queries
        .iter()
        .copied()
        .chain([long.as_str()])
        .collect();

    for query in hostile_queries {
        let app = router(state.clone());
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let uri = format!("/api/v1/search?q={encoded}");
        let res = app
            .oneshot(get(&uri, Some("Bearer token")))
            .await
            .expect("request");
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "hostile query {query:?} should return 200 OK"
        );
    }
}

#[tokio::test]
async fn a_percent_that_is_no_escape_is_read_as_typed() {
    let state = state().await;
    seed(&state).await;

    // `%+e` is not an escape: `+` is a space, so this is "% engine". A decoder
    // that reads `+e` as a signed hex number turns it into one control byte
    // and the rest of a word, and the search finds nothing.
    let res = router(state.clone())
        .oneshot(get("/api/v1/search?q=%+engine", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(
        body["count"], 1,
        "the word after the stray percent is searched: {body}"
    );
}

/// Every hit in a response, whatever group it sits in.
fn hits(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|group| group["hits"].as_array().expect("hits").clone())
        .collect()
}

#[tokio::test]
async fn a_hit_names_its_project_as_the_projects_list_does() {
    let state = state().await;
    let _ = agent_hub::store::projects::create(&state.db, "proj", "Engine Room").await;
    seed(&state).await;

    let response = router(state.clone())
        .oneshot(get("/api/v1/search?q=engine", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let hits = hits(&body);
    assert!(!hits.is_empty(), "the seeded signal is found: {body}");
    for hit in &hits {
        assert_eq!(hit["project_id"], "proj");
        assert_eq!(
            hit["project_display_name"], "Engine Room",
            "a hit carries the project's display name: {hit}"
        );
    }
}

async fn plant_brain_entry(state: &AppState, project_id: &str, session_id: &str, body: &str) {
    let conn = state.db.connect().expect("connect");
    agent_hub::store::search::index_doc(
        &conn,
        agent_hub::store::search::SearchDoc {
            doc_id: &format!("brain:{session_id}:/fs/plan.md"),
            project_id,
            kind: "brain",
            ref_id: "/fs/plan.md",
            session_id: Some(session_id),
            title: Some("/fs/plan.md"),
            body,
            updated_at: "2026-09-18T00:00:00Z",
        },
    )
    .await
    .expect("index a brain entry");
}

#[tokio::test]
async fn a_hit_carries_what_its_row_shows_for_its_family() {
    use agent_hub::store::artifacts::{self, EnvelopeUpdate, NewArtifact, UpdateOptions};
    use agent_hub::store::sessions;

    let state = state().await;
    append(
        &state.db,
        "agent-one",
        None,
        NewEvent {
            project_id: "proj".to_string(),
            kind: "finished".to_string(),
            summary: "needle report done".to_string(),
            payload: None,
            needs_action: false,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .expect("append");
    let artifact = artifacts::publish(
        &state.db,
        &state.data_dir,
        NewArtifact {
            actor: "agent-one",
            project_id: "proj",
            title: "needle chart",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"first",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish");
    artifacts::update(
        &state.db,
        &state.data_dir,
        "agent-one",
        &artifact.id,
        b"needle, second cut",
        EnvelopeUpdate::Keep,
        UpdateOptions::default(),
        None,
    )
    .await
    .expect("update");
    let session = sessions::start(&state.db, "proj", "nightly-run", "agent-two")
        .await
        .expect("start");
    plant_brain_entry(&state, "proj", &session.id, "needle notes").await;
    sessions::end(&state.db, &session.id, "agent-two", None)
        .await
        .expect("end");

    let response = router(state.clone())
        .oneshot(get("/api/v1/search?q=needle", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let hits = hits(&body);
    let of = |kind: &str| {
        hits.iter()
            .find(|hit| hit["kind"] == kind)
            .unwrap_or_else(|| panic!("a {kind} hit: {body}"))
    };

    // Publishing an artifact lands on the feed too, so the report is picked
    // by its title.
    let feed = hits
        .iter()
        .find(|hit| hit["title"] == "needle report done")
        .unwrap_or_else(|| panic!("the report: {body}"));
    assert_eq!(feed["kind"], "feed");
    assert_eq!(feed["event_kind"], "finished", "{feed}");
    assert_eq!(feed["actor"], "agent-one", "{feed}");

    let chart = of("artifact");
    assert_eq!(chart["version"], 2, "the current version: {chart}");
    assert_eq!(
        chart["size_bytes"],
        b"needle, second cut".len(),
        "the current version's size: {chart}"
    );

    let brain = of("brain");
    assert_eq!(brain["session_name"], "nightly-run", "{brain}");
    assert_eq!(brain["session_status"], "ended", "{brain}");

    // A field belongs to one family and is left off the others.
    for field in ["version", "size_bytes", "session_name", "session_status"] {
        assert!(feed.get(field).is_none(), "{field} on a feed hit: {feed}");
    }
    for field in ["event_kind", "actor"] {
        assert!(
            chart.get(field).is_none(),
            "{field} on an artifact: {chart}"
        );
        assert!(
            brain.get(field).is_none(),
            "{field} on a brain hit: {brain}"
        );
    }
}
