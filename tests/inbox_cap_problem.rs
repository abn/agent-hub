//! The inbox cap's refusal as an HTTP problem.
//!
//! No REST route writes an action item: agents reach the cap through the MCP
//! tools, which is covered where those are tested. What this file pins is the
//! other half, that the store's `RateLimited` refusal becomes a 429 problem
//! with the `rate_limited` code when a handler maps it, that `0` disables a
//! cap, and that a refused write leaves no partial row. The route it drives is
//! its own, a handler of the same shape as the hub's, because the hub has none.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower::ServiceExt;

use agent_hub::app::AppState;
use agent_hub::config::{Config, TrustDefault};
use agent_hub::error::{Error, ErrorCode};
use agent_hub::http::problem::Problem;
use agent_hub::limits::InboxCaps;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::projects;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

async fn state_with_caps(inbox_caps: InboxCaps) -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-hub-http-inbox-cap-{}-{nanos}-{unique}",
        std::process::id()
    ));

    let state = AppState::open(Config {
        data_dir: dir,
        bind: "127.0.0.1:0".parse().expect("socket address"),
        public_url: None,
        admin_token: Some("token".to_string()),
        trust_default: TrustDefault::Trusted,
        inbox_caps,
        active_window: std::time::Duration::from_secs(900),
        node_name: None,
    })
    .await
    .expect("open state");

    projects::create(&state.db, "proj", "Test Project")
        .await
        .expect("create project");

    state
}

#[derive(Debug, Deserialize, Serialize)]
struct ActionRequest {
    project_id: String,
    actor: String,
    summary: String,
}

#[derive(Debug, Serialize)]
struct ActionResponse {
    event_id: String,
}

async fn post_action(
    State(state): State<AppState>,
    Json(payload): Json<ActionRequest>,
) -> Result<Json<ActionResponse>, Problem> {
    let event_id = events::append_action(
        &state.db,
        &state.config.inbox_caps,
        &payload.actor,
        None,
        NewEvent {
            project_id: payload.project_id,
            kind: "approval".to_string(),
            summary: payload.summary,
            payload: None,
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(ActionResponse { event_id }))
}

fn app_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/api/v1/test/actions", post(post_action))
        .with_state(state)
}

fn request(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

#[tokio::test]
async fn problem_mapping_for_rate_limited_returns_429() {
    let err = Error::RateLimited(
        "agent-1 already has 5 open items in proj; the per-agent cap is 5".to_string(),
    );
    assert_eq!(err.code(), ErrorCode::RateLimited);

    let problem = Problem::from_error(&err);
    assert_eq!(problem.status, 429);
    assert_eq!(problem.code, "rate_limited");
    assert_eq!(problem.title, "Too Many Requests");
    assert!(problem.detail.contains("the per-agent cap is 5"));

    let response = problem.into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );

    let body = json_body(response).await;
    assert_eq!(body["status"], 429);
    assert_eq!(body["code"], "rate_limited");
    assert_eq!(body["title"], "Too Many Requests");
    assert!(
        body["detail"]
            .as_str()
            .expect("detail")
            .contains("the per-agent cap is 5")
    );
}

async fn count_events(state: &AppState, project_id: &str) -> i64 {
    let conn = state.db.connect().expect("connect");
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM events WHERE project_id = ?1",
            vec![turso::Value::Text(project_id.to_string())],
        )
        .await
        .expect("count events");
    let row = rows.next().await.expect("row").expect("count row");
    row.get::<i64>(0).expect("count")
}

#[tokio::test]
async fn per_actor_cap_refusal_is_a_429_problem() {
    let state = state_with_caps(InboxCaps {
        per_actor: 2,
        per_project: 10,
    })
    .await;
    let app = app_router(state.clone());

    // Write 1: admitted
    let resp1 = app
        .clone()
        .oneshot(request(
            "/api/v1/test/actions",
            json!({ "project_id": "proj", "actor": "agent-a", "summary": "action 1" }),
        ))
        .await
        .expect("request 1");
    assert_eq!(resp1.status(), StatusCode::OK);

    // Write 2: admitted
    let resp2 = app
        .clone()
        .oneshot(request(
            "/api/v1/test/actions",
            json!({ "project_id": "proj", "actor": "agent-a", "summary": "action 2" }),
        ))
        .await
        .expect("request 2");
    assert_eq!(resp2.status(), StatusCode::OK);

    // Write 3: rejected at cap
    let resp3 = app
        .clone()
        .oneshot(request(
            "/api/v1/test/actions",
            json!({ "project_id": "proj", "actor": "agent-a", "summary": "action 3" }),
        ))
        .await
        .expect("request 3");
    assert_eq!(resp3.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp3
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );

    let problem = json_body(resp3).await;
    assert_eq!(problem["status"], 429);
    assert_eq!(problem["code"], "rate_limited");
    assert_eq!(problem["title"], "Too Many Requests");
    assert!(
        problem["detail"]
            .as_str()
            .expect("detail")
            .contains("agent-a already has 2 open items in proj; the per-agent cap is 2")
    );

    // Verify transactional atomicity: no partial rows written
    let count = count_events(&state, "proj").await;
    assert_eq!(count, 2, "only the admitted events were written");
}

#[tokio::test]
async fn per_project_cap_refusal_is_a_429_problem() {
    let state = state_with_caps(InboxCaps {
        per_actor: 10,
        per_project: 2,
    })
    .await;
    let app = app_router(state.clone());

    // Write 1 from agent-a: admitted
    let resp1 = app
        .clone()
        .oneshot(request(
            "/api/v1/test/actions",
            json!({ "project_id": "proj", "actor": "agent-a", "summary": "action 1" }),
        ))
        .await
        .expect("request 1");
    assert_eq!(resp1.status(), StatusCode::OK);

    // Write 2 from agent-b: admitted
    let resp2 = app
        .clone()
        .oneshot(request(
            "/api/v1/test/actions",
            json!({ "project_id": "proj", "actor": "agent-b", "summary": "action 2" }),
        ))
        .await
        .expect("request 2");
    assert_eq!(resp2.status(), StatusCode::OK);

    // Write 3 from agent-c: rejected at per-project cap
    let resp3 = app
        .clone()
        .oneshot(request(
            "/api/v1/test/actions",
            json!({ "project_id": "proj", "actor": "agent-c", "summary": "action 3" }),
        ))
        .await
        .expect("request 3");
    assert_eq!(resp3.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp3
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );

    let problem = json_body(resp3).await;
    assert_eq!(problem["status"], 429);
    assert_eq!(problem["code"], "rate_limited");
    assert_eq!(problem["title"], "Too Many Requests");
    assert!(
        problem["detail"]
            .as_str()
            .expect("detail")
            .contains("proj already has 2 open items; the per-project cap is 2")
    );

    // Verify transactional atomicity: exactly 2 events in store
    let count = count_events(&state, "proj").await;
    assert_eq!(count, 2, "third write was refused without partial rows");
}

#[tokio::test]
async fn zero_disables_a_cap() {
    let caps = InboxCaps::parse(Some("0"), Some("0")).expect("parse zero caps");
    assert_eq!(caps.per_actor, 0);
    assert_eq!(caps.per_project, 0);
    assert!(!caps.enabled(), "zero values disable both caps");

    let state = state_with_caps(caps).await;
    let app = app_router(state.clone());

    // Write 5 open items from the same agent; all must succeed when caps are 0
    for i in 1..=5 {
        let resp = app
            .clone()
            .oneshot(request(
                "/api/v1/test/actions",
                json!({ "project_id": "proj", "actor": "agent-a", "summary": format!("action {i}") }),
            ))
            .await
            .expect("request");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "item {i} must succeed when caps are disabled"
        );
    }

    let count = count_events(&state, "proj").await;
    assert_eq!(count, 5, "all 5 events were recorded");
}
