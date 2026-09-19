//! The search route: auth, results, and query validation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::http::router;
use agent_hub::store::events::{NewEvent, append};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

static NEXT: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-search-http-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("addr"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps: agent_hub::limits::InboxCaps::disabled(),
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
    })
    .await
    .expect("open state")
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

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method("GET");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder.body(Body::empty()).expect("request")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn search_returns_hits_with_a_token() {
    let state = state().await;
    seed(&state).await;
    let app = router(state);

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
    let app = router(state().await);
    let response = app
        .oneshot(get("/api/v1/search?q=engine", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn search_requires_a_query() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/api/v1/search", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_rejects_an_unknown_type() {
    let app = router(state().await);
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
    let app = router(state);
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
    let app = router(state);
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
    let app = router(state);
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

    let app = router(state);
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
    let res = router(state)
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
