//! HTTP API and static PWA surface.

use axum::extract::State;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;

pub mod artifacts;
pub mod auth;
pub mod feed;
pub mod inbox;
pub mod problem;
pub mod search;
pub mod sessions;
pub mod storage;

use problem::Problem;

/// Build the HTTP router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/home", get(inbox::home))
        .route("/api/v1/inbox", get(inbox::list))
        .route("/api/v1/questions/{id}/answer", post(inbox::answer))
        .route("/api/v1/projects/{id}/feed", get(feed::read))
        .route("/api/v1/projects/{id}/artifacts", get(artifacts::list))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}/end", post(sessions::end))
        .route("/api/v1/storage/sessions/{id}", delete(storage::prune))
        .route("/api/v1/prune/undo/{token}", post(storage::undo))
        .route("/api/v1/search", get(search::search))
        .route("/artifacts/{id}", get(artifacts::render))
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
