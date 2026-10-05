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

/// Free space the data volume must hold before the hub calls itself ready.
///
/// A store with no room left cannot checkpoint its write-ahead log or apply a
/// migration, so a probe that ignored the volume stayed ready through the one
/// failure an operator is about to hit.
const READY_MIN_FREE_BYTES: i64 = 64 * 1024 * 1024;

pub mod agents;
pub mod artifacts;
pub mod auth;
pub mod enrol;
pub mod feed;
pub mod inbox;
pub mod kb;
pub mod kb_comments;
pub mod metrics;
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
    // The embedded assets are a table in `web`, so the shell, the worker's
    // precache list and the routes cannot drift apart as modules are added.
    let mut assets: Router<AppState> = Router::new();
    for path in web::asset_paths() {
        // The manifest is served from the table's bytes but with the node's
        // name folded in, so it needs the state the generic handler has no
        // use for. Registering it here too would be a duplicate route.
        if path == web::MANIFEST_PATH {
            continue;
        }
        assets = assets.route(path, get(web::asset));
    }
    assets
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route(web::MANIFEST_PATH, get(web::manifest))
        .route("/sw.js", get(web::service_worker))
        .route("/SKILL.md", get(skill::skill))
        .route("/metrics", get(metrics::metrics))
        .route("/api/v1/agents", get(agents::list).post(agents::create))
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
        .route("/api/v1/enrol", post(enrol::enrol))
        .route("/api/v1/enrol/status", get(enrol::status))
        .route("/api/v1/enrol/{id}/approve", post(enrol::approve))
        .route("/api/v1/enrol/{id}/refuse", post(enrol::refuse))
        .route("/api/v1/artifacts", get(artifacts::list_session))
        .route(
            "/api/v1/artifacts/{id}",
            get(artifacts::content).delete(artifacts::destroy),
        )
        .route(
            "/api/v1/artifacts/{id}/share",
            post(artifacts::share_create)
                .delete(artifacts::share_revoke)
                .get(artifacts::share_get),
        )
        .route("/api/v1/artifacts/{id}/versions", get(artifacts::versions))
        .route(
            "/api/v1/artifacts/{id}/viewer-pass",
            get(artifacts::viewer_pass),
        )
        .route("/api/v1/artifacts/{id}/raw", get(artifacts::raw))
        .route(
            "/api/v1/artifacts/{id}/comments",
            get(artifacts::comment_list).post(artifacts::comment_post),
        )
        .route(
            "/api/v1/artifacts/{id}/comments/{comment_id}",
            patch(artifacts::comment_resolve).delete(artifacts::comment_remove),
        )
        .route("/api/v1/feed", get(feed::read_global))
        .route("/api/v1/home", get(inbox::home))
        .route("/api/v1/inbox", get(inbox::list))
        .route("/api/v1/inbox/read-all", post(inbox::read_all))
        .route("/api/v1/inbox/{event_id}/read", post(inbox::read))
        .route("/api/v1/inbox/{event_id}/unread", post(inbox::unread))
        .route("/api/v1/stream", get(stream::stream))
        .route("/api/v1/questions/{id}/answer", post(inbox::answer))
        .route("/api/v1/approvals/{id}/decision", post(inbox::decide))
        .route(
            "/api/v1/projects",
            get(projects::list).post(projects::create),
        )
        .route(
            "/api/v1/projects/{id}",
            get(projects::get)
                .patch(projects::update)
                .delete(projects::delete),
        )
        .route("/api/v1/projects/{id}/stats", get(projects::stats))
        .route("/api/v1/projects/{id}/feed", get(feed::read))
        .route("/api/v1/projects/{id}/feed/seen", post(feed::seen))
        .route("/api/v1/projects/{id}/artifacts", get(artifacts::list))
        .route("/api/v1/projects/{id}/kb/pages", get(kb::pages))
        .route(
            "/api/v1/projects/{id}/kb/pages/{*path}",
            get(kb::page_get)
                .put(kb::page_put)
                .delete(kb::page_delete)
                .post(kb::page_post),
        )
        .route("/api/v1/projects/{id}/kb/promote", post(kb::promote))
        .route(
            "/api/v1/projects/{id}/kb/comments",
            get(kb_comments::list).post(kb_comments::create),
        )
        .route(
            "/api/v1/projects/{id}/kb/comments/{comment_id}",
            delete(kb_comments::remove),
        )
        .route(
            "/api/v1/projects/{id}/kb/comments/{comment_id}/done",
            post(kb_comments::set_done),
        )
        .route("/api/v1/projects/{id}/kb/history", get(kb::history))
        .route("/api/v1/projects/{id}/kb/backlinks", get(kb::backlinks))
        .route("/api/v1/projects/{id}/kb/lint", get(kb::lint))
        .route("/api/v1/projects/{id}/kb/stats", get(kb::stats))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}", get(sessions::detail))
        .route("/api/v1/sessions/{id}/end", post(sessions::end))
        .route("/api/v1/sessions/{id}/reassign", post(sessions::reassign))
        .route("/api/v1/sessions/{id}/brain", get(sessions::brain))
        .route(
            "/api/v1/sessions/{id}/brain/entry",
            get(sessions::brain_entry),
        )
        .route("/api/v1/storage", get(storage::usage))
        .route("/api/v1/storage/sessions", delete(storage::prune_all))
        .route("/api/v1/storage/sessions/{id}", delete(storage::prune))
        .route(
            "/api/v1/storage/projects/{id}/sessions",
            delete(storage::prune_project),
        )
        .route("/api/v1/prune/undo/{token}", post(storage::undo))
        .route("/api/v1/search", get(search::search))
        .route("/artifacts/{id}", get(artifacts::host))
        .route("/artifacts/{id}/frame", get(artifacts::frame))
        .route("/artifacts/{id}/og.svg", get(artifacts::og_svg))
        .route("/s/{token}", get(artifacts::share_host))
        .route("/s/{token}/frame", get(artifacts::share_frame))
        .route("/s/{token}/og.svg", get(artifacts::share_og_svg))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(limits::REQUEST_BODY_BYTES_MAX))
        .with_state(state)
}

/// Liveness: the process is running and serving. It touches nothing else.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness: the engine answers, at a schema this binary knows, over the
/// store it opened, on a volume with room left.
///
/// Three legs, because a probe that trusted only the version cached at startup
/// stayed ready through an unmounted volume or a replaced store. The first is
/// the live schema version against what this binary supports; the second the
/// device and inode of the data directory and `hub.db` against what was
/// captured at open; the third a cheap content read, so a store that is not
/// this hub's is caught even while its descriptor still answers.
async fn readyz(State(state): State<AppState>) -> std::result::Result<Json<Value>, Problem> {
    let probe = tokio::time::timeout(READY_PROBE_WAIT, store::schema_version(&state.db)).await;
    let version = match probe {
        Ok(Ok(version)) => version,
        Ok(Err(err)) => {
            return Err(unavailable(format!("the store did not answer: {err}")));
        }
        Err(_) => {
            return Err(unavailable(
                "the store did not answer the readiness query in time".to_string(),
            ));
        }
    };

    if version > store::schema::SUPPORTED_MAX {
        return Err(unavailable(format!(
            "the store is at schema version {version}, newer than this binary supports ({})",
            store::schema::SUPPORTED_MAX
        )));
    }
    if version != state.schema_version {
        return Err(unavailable(format!(
            "the store is at schema version {version}, the process opened {}",
            state.schema_version
        )));
    }

    store::probe_content(&state.db)
        .await
        .map_err(|err| unavailable(format!("the store's content did not read back: {err}")))?;

    if let Err(detail) = state
        .store_identity
        .check(&state.data_dir, &state.config.hub_db_path())
    {
        return Err(unavailable(detail));
    }

    let free = store::free_space_bytes(&state.data_dir);
    if let Some(free) = free
        && free < READY_MIN_FREE_BYTES
    {
        return Err(unavailable(format!(
            "the data directory has {free} bytes free, under the {READY_MIN_FREE_BYTES}-byte margin"
        )));
    }

    Ok(Json(json!({
        "status": "ready",
        "schema_version": version,
        "supported_max": store::schema::SUPPORTED_MAX,
        "free_bytes": free,
        "free_space_margin_bytes": READY_MIN_FREE_BYTES,
    })))
}

/// A readiness failure, as problem details with the unavailable code.
fn unavailable(detail: String) -> Problem {
    Problem::from_error(&Error::Unavailable(detail))
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
