//! HTTP API and static PWA surface.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;

pub mod auth;
pub mod feed;
pub mod problem;
pub mod sessions;

use problem::Problem;

/// Build the HTTP router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/projects/{id}/feed", get(feed::read))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}/end", post(sessions::end))
        .fallback(not_found)
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ready",
        "schema_version": state.schema_version,
    }))
}

async fn not_found() -> Problem {
    Problem::from_error(&Error::NotFound("no route matches this path".to_string()))
}
