//! HTTP API and static PWA surface.

use axum::extract::{DefaultBodyLimit, State};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;
use crate::{limits, store};

/// How long the readiness probe waits for the engine before calling it
/// unavailable. Shorter than the store's lock wait, so the probe answers
/// instead of queueing behind a writer.
const READY_PROBE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub mod agents;
pub mod artifacts;
pub mod auth;
pub mod feed;
pub mod inbox;
pub mod origin;
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
///
/// The request body limit is the hub's own, set here rather than left to the
/// framework default, so the documented limit is the one callers meet.
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
        .route(
            "/api/v1/artifacts/{id}/comments",
            get(artifacts::comment_list).post(artifacts::comment_post),
        )
        .route(
            "/api/v1/artifacts/{id}/comments/{comment_id}",
            patch(artifacts::comment_resolve).delete(artifacts::comment_remove),
        )
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
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(limits::REQUEST_BODY_BYTES_MAX))
        .with_state(state)
}

/// Liveness: the process is running and serving. It touches nothing else.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness: the engine answers and carries the schema the process opened.
///
/// A probe that reported only the version cached at startup stayed ready
/// through an unmounted volume or a replaced store, which is the failure a
/// healthcheck exists to catch.
async fn readyz(State(state): State<AppState>) -> std::result::Result<Json<Value>, Problem> {
    let probe = tokio::time::timeout(READY_PROBE_WAIT, store::applied_version(&state.db)).await;
    let version = match probe {
        Ok(Ok(version)) => version,
        Ok(Err(err)) => {
            return Err(Problem::from_error(&Error::Unavailable(format!(
                "the store did not answer: {err}"
            ))));
        }
        Err(_) => {
            return Err(Problem::from_error(&Error::Unavailable(
                "the store did not answer the readiness query in time".to_string(),
            )));
        }
    };
    if version != state.schema_version {
        return Err(Problem::from_error(&Error::Unavailable(format!(
            "the store is at schema version {version}, the process opened {}",
            state.schema_version
        ))));
    }

    Ok(Json(json!({
        "status": "ready",
        "schema_version": version,
    })))
}

async fn not_found() -> Problem {
    Problem::from_error(&Error::NotFound("no route matches this path".to_string()))
}

/// The path exists but not for this method. The framework still attaches the
/// `Allow` header, so the response keeps naming the methods that do work.
async fn method_not_allowed() -> Problem {
    Problem::with_status(
        &Error::InvalidArgument("this path does not serve that method".to_string()),
        axum::http::StatusCode::METHOD_NOT_ALLOWED,
    )
}
