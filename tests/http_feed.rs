//! HTTP feed route: auth, ordering, and problem responses.

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

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state() -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-{}-{nanos}-{unique}",
        std::process::id()
    ));
    AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
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

fn event(summary: &str) -> NewEvent {
    NewEvent {
        project_id: "proj".to_string(),
        kind: "signal".to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method("GET");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    builder.body(Body::empty()).expect("build request")
}

async fn problem_body(response: axum::response::Response) -> Value {
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
    );
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("problem is JSON")
}

#[tokio::test]
async fn the_human_feed_hides_system_events() {
    let state = state().await;
    append(&state.db, "agent", None, event("a signal"))
        .await
        .expect("append signal");
    let mut audit = event("agent created");
    audit.kind = "system".to_string();
    append(&state.db, "human", None, audit)
        .await
        .expect("append system");

    let kinds = |page: &Value| -> Vec<String> {
        page["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| event["kind"].as_str().expect("kind").to_string())
            .collect()
    };

    let app = router(state.clone());
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
        .await
        .expect("request");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    assert_eq!(
        kinds(&page),
        vec!["signal".to_string()],
        "the human feed hides system events"
    );

    // An explicit kind filter still reaches the audit trail.
    let app = router(state);
    let response = app
        .oneshot(get(
            "/api/v1/projects/proj/feed?kinds=system",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    assert_eq!(
        kinds(&page),
        vec!["system".to_string()],
        "an explicit kind filter is honoured"
    );
}

#[tokio::test]
async fn missing_token_is_a_problem() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "unauthenticated");
    assert_eq!(problem["status"], 401);
    assert!(problem["title"].is_string());
}

#[tokio::test]
async fn bearer_token_reads_events_newest_first() {
    let state = state().await;
    append(&state.db, "agent", None, event("first"))
        .await
        .expect("append first");
    append(&state.db, "agent", None, event("second"))
        .await
        .expect("append second");

    let app = router(state);
    let response = app
        .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let page: Value = serde_json::from_slice(&bytes).expect("page is JSON");
    let events = page["events"].as_array().expect("events array");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["summary"], "second");
    assert_eq!(events[1]["summary"], "first");
    assert_eq!(page["next_since"], events[0]["id"]);
    assert_eq!(page["next_before"], events[1]["id"]);
}

#[tokio::test]
async fn invalid_limit_is_rejected() {
    let app = router(state().await);
    let response = app
        .oneshot(get(
            "/api/v1/projects/proj/feed?limit=soon",
            Some("Bearer token"),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "invalid_argument");
}

#[tokio::test]
async fn unknown_route_is_not_found() {
    let app = router(state().await);
    let response = app
        .oneshot(get("/api/v1/no-such-route", None))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let problem = problem_body(response).await;
    assert_eq!(problem["code"], "not_found");
}

fn post(uri: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri).method("POST");
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, token);
    }
    match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .expect("build request"),
        None => builder.body(Body::empty()).expect("build request"),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

async fn read_feed_page(state: &AppState) -> Value {
    json_body(
        router(state.clone())
            .oneshot(get("/api/v1/projects/proj/feed", Some("Bearer token")))
            .await
            .expect("request"),
    )
    .await
}

#[tokio::test]
async fn the_feed_says_how_far_the_human_has_read_it() {
    let state = state().await;
    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    let first = append(&state.db, "agent", None, event("first"))
        .await
        .expect("append first");
    let second = append(&state.db, "agent", None, event("second"))
        .await
        .expect("append second");

    let page = read_feed_page(&state).await;
    assert!(
        page["last_seen"].is_null(),
        "nothing is seen until the feed is opened"
    );

    let response = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/proj/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_id": second })),
        ))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["last_seen"], second);
    assert_eq!(body["advanced"], true);
    assert_eq!(read_feed_page(&state).await["last_seen"], second);

    // An older event, an event of another project, and an id that names
    // nothing all leave the cursor where it is.
    let elsewhere = append(
        &state.db,
        "agent",
        None,
        NewEvent {
            project_id: "other".to_string(),
            ..event("elsewhere")
        },
    )
    .await
    .expect("append elsewhere");
    for event_id in [first, elsewhere, "no-such-event".to_string()] {
        let response = router(state.clone())
            .oneshot(post(
                "/api/v1/projects/proj/feed/seen",
                Some("Bearer token"),
                Some(serde_json::json!({ "event_id": event_id })),
            ))
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["advanced"], false, "{event_id} does not move it");
        assert_eq!(body["last_seen"], second);
    }
}

#[tokio::test]
async fn marking_a_feed_seen_needs_a_token_and_a_project_that_exists() {
    let state = state().await;
    let denied = router(state.clone())
        .oneshot(post("/api/v1/projects/proj/feed/seen", None, None))
        .await
        .expect("request");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let missing = router(state.clone())
        .oneshot(post(
            "/api/v1/projects/no-such-project/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({ "event_id": "any" })),
        ))
        .await
        .expect("request");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(missing).await["code"], "not_found");

    agent_hub::store::projects::create(&state.db, "proj", "Proj")
        .await
        .expect("create project");
    let nameless = router(state)
        .oneshot(post(
            "/api/v1/projects/proj/feed/seen",
            Some("Bearer token"),
            Some(serde_json::json!({})),
        ))
        .await
        .expect("request");
    assert_eq!(nameless.status(), StatusCode::BAD_REQUEST);
    assert_eq!(problem_body(nameless).await["code"], "invalid_argument");
}
