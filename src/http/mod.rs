//! HTTP API and static PWA surface.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::app::AppState;

/// Build the HTTP router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
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
