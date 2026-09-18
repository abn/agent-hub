//! HTTP API and static PWA surface.

use axum::extract::State;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;

pub mod agents;
pub mod artifacts;
pub mod auth;
pub mod feed;
pub mod inbox;
pub mod problem;
pub mod projects;
pub mod search;
pub mod sessions;
pub mod skill;
pub mod storage;
pub mod stream;
pub mod web;

use problem::Problem;

/// Build the HTTP router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/", get(web::index))
        .route("/app.js", get(web::app_js))
        .route("/app.css", get(web::app_css))
        .route("/tokens.css", get(web::tokens_css))
        .route("/manifest.webmanifest", get(web::manifest))
        .route("/sw.js", get(web::service_worker))
        .route("/icon.svg", get(web::icon))
        .route("/crypto.mjs", get(web::crypto_js))
        .route("/vendor/marked.js", get(web::marked_js))
        .route("/vendor/mermaid.runtime.js", get(web::mermaid_js))
        .route("/frame-loader.js", get(web::frame_loader_js))
        .route("/artifact-viewer.mjs", get(web::viewer_js))
        .route("/SKILL.md", get(skill::skill))
        .route("/api/v1/agents", get(agents::list).post(agents::create))
        .route("/api/v1/agents/{id}", patch(agents::update))
        .route(
            "/api/v1/agents/{id}/token",
            post(agents::issue).delete(agents::revoke),
        )
        .route(
            "/api/v1/agents/{id}/grants",
            get(agents::grants).post(agents::grant),
        )
        .route(
            "/api/v1/agents/{id}/grants/{project_id}",
            delete(agents::ungrant),
        )
        .route(
            "/api/v1/artifacts/{id}",
            get(artifacts::content).delete(artifacts::destroy),
        )
        .route("/api/v1/artifacts/{id}/versions", get(artifacts::versions))
        .route("/api/v1/artifacts/{id}/raw", get(artifacts::raw))
        .route("/api/v1/home", get(inbox::home))
        .route("/api/v1/inbox", get(inbox::list))
        .route("/api/v1/stream", get(stream::stream))
        .route("/api/v1/questions/{id}/answer", post(inbox::answer))
        .route("/api/v1/approvals/{id}/decision", post(inbox::decide))
        .route(
            "/api/v1/projects",
            get(projects::list).post(projects::create),
        )
        .route("/api/v1/projects/{id}", delete(projects::delete))
        .route("/api/v1/projects/{id}/feed", get(feed::read))
        .route("/api/v1/projects/{id}/artifacts", get(artifacts::list))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}/end", post(sessions::end))
        .route("/api/v1/sessions/{id}/brain", get(sessions::brain))
        .route("/api/v1/storage", get(storage::usage))
        .route("/api/v1/storage/sessions/{id}", delete(storage::prune))
        .route("/api/v1/prune/undo/{token}", post(storage::undo))
        .route("/api/v1/search", get(search::search))
        .route("/artifacts/{id}", get(artifacts::host))
        .route("/artifacts/{id}/frame", get(artifacts::frame))
        .route("/artifacts/{id}/og.svg", get(artifacts::og_svg))
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
