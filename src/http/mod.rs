//! HTTP API and static PWA surface.

use axum::extract::{DefaultBodyLimit, State};
use axum::routing::{MethodRouter, delete, get, patch, post};
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

pub mod artifacts;
pub mod auth;
pub mod backup;
pub mod devices;
pub mod enrol;
pub mod feed;
pub mod filters;
pub mod ideas;
pub mod inbox;
pub mod kb;
pub mod kb_comments;
pub mod me;
pub mod members;
pub mod metrics;
pub mod notify;
pub mod origin;
pub mod problem;
pub mod projects;
pub mod resolve;
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
    let mut router = assets;
    for (path, handlers) in routes() {
        router = router.route(path, handlers);
    }
    router
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(limits::REQUEST_BODY_BYTES_MAX))
        .with_state(state)
}

/// Every route but the embedded assets: its path and its handlers.
///
/// A table rather than a chain, so what a route is reached by can be read
/// back: a paired phone's guard decides every path here
/// ([`crate::phones::guard`]), and its test walks this list.
fn routes() -> Vec<(&'static str, MethodRouter<AppState>)> {
    vec![
        ("/healthz", get(healthz)),
        ("/readyz", get(readyz)),
        (web::MANIFEST_PATH, get(web::manifest)),
        ("/sw.js", get(web::service_worker)),
        ("/bootstrap/SKILL.md", get(skill::skill)),
        ("/metrics", get(metrics::metrics)),
        ("/api/v1/members", get(members::list).post(members::create)),
        (
            "/api/v1/members/{id}/token",
            post(members::issue).delete(members::revoke),
        ),
        (
            "/api/v1/members/{id}/grants",
            get(members::grants).post(members::grant),
        ),
        (
            "/api/v1/members/{id}/grants/{project_id}",
            delete(members::ungrant),
        ),
        ("/api/v1/devices", get(devices::list)),
        ("/api/v1/devices/me", get(devices::me)),
        (
            "/api/v1/devices/pairing",
            post(devices::pairing).delete(devices::withdraw),
        ),
        ("/api/v1/devices/{id}", delete(devices::revoke)),
        ("/api/v1/devices/{id}/confirm", post(devices::confirm)),
        ("/api/v1/devices/{id}/refuse", post(devices::refuse)),
        ("/api/v1/enrol", post(enrol::enrol)),
        ("/api/v1/enrol/status", get(enrol::status)),
        ("/api/v1/enrol/{id}/approve", post(enrol::approve)),
        ("/api/v1/enrol/{id}/refuse", post(enrol::refuse)),
        ("/api/v1/artifacts", get(artifacts::list_session)),
        (
            "/api/v1/artifacts/{id}",
            get(artifacts::content).delete(artifacts::destroy),
        ),
        (
            "/api/v1/artifacts/{id}/share",
            post(artifacts::share_create)
                .delete(artifacts::share_revoke)
                .get(artifacts::share_get),
        ),
        (
            "/api/v1/artifacts/{id}/versions",
            get(artifacts::versions).delete(artifacts::squash_versions_delete),
        ),
        (
            "/api/v1/artifacts/{id}/versions/{version}",
            delete(artifacts::squash_versions_path),
        ),
        (
            "/api/v1/artifacts/{id}/squash",
            post(artifacts::squash_versions_post),
        ),
        (
            "/api/v1/artifacts/{id}/versions/squash",
            post(artifacts::squash_versions_post),
        ),
        ("/api/v1/artifacts/{id}/live", get(artifacts::live)),
        (
            "/api/v1/artifacts/{id}/viewer-pass",
            get(artifacts::viewer_pass),
        ),
        ("/api/v1/artifacts/{id}/raw", get(artifacts::raw)),
        (
            "/api/v1/artifacts/{id}/comments",
            get(artifacts::comment_list).post(artifacts::comment_post),
        ),
        (
            "/api/v1/artifacts/{id}/comments/{comment_id}",
            patch(artifacts::comment_resolve).delete(artifacts::comment_remove),
        ),
        ("/api/v1/me/pending", get(me::pending)),
        ("/api/v1/feed", get(feed::read_global)),
        ("/api/v1/home", get(inbox::home)),
        ("/api/v1/inbox", get(inbox::list)),
        ("/api/v1/inbox/read-all", post(inbox::read_all)),
        ("/api/v1/inbox/{event_id}/read", post(inbox::read)),
        ("/api/v1/inbox/{event_id}/unread", post(inbox::unread)),
        ("/api/v1/stream", get(stream::stream)),
        ("/api/v1/questions/{id}/answer", post(inbox::answer)),
        ("/api/v1/approvals/{id}/decision", post(inbox::decide)),
        (
            "/api/v1/projects",
            get(projects::list).post(projects::create),
        ),
        (
            "/api/v1/projects/{id}",
            get(projects::get)
                .patch(projects::update)
                .delete(projects::delete),
        ),
        ("/api/v1/projects/{id}/stats", get(projects::stats)),
        ("/api/v1/projects/{id}/ideas", post(ideas::create)),
        ("/api/v1/ideas/{id}/resolve", post(ideas::resolve)),
        ("/api/v1/ideas/{id}/clear", post(ideas::clear)),
        ("/api/v1/ideas/{id}/reply", post(ideas::reply)),
        ("/api/v1/projects/{id}/feed", get(feed::read)),
        ("/api/v1/projects/{id}/feed/seen", post(feed::seen)),
        ("/api/v1/projects/{id}/feed/dismiss", post(feed::dismiss)),
        (
            "/api/v1/projects/{id}/feed/dismiss-read",
            post(feed::dismiss_read),
        ),
        ("/api/v1/projects/{id}/artifacts", get(artifacts::list)),
        ("/api/v1/projects/{id}/kb/pages", get(kb::pages)),
        (
            "/api/v1/projects/{id}/kb/pages/{*path}",
            get(kb::page_get)
                .put(kb::page_put)
                .delete(kb::page_delete)
                .post(kb::page_post),
        ),
        ("/api/v1/projects/{id}/kb/promote", post(kb::promote)),
        (
            "/api/v1/projects/{id}/kb/comments",
            get(kb_comments::list).post(kb_comments::create),
        ),
        (
            "/api/v1/projects/{id}/kb/comments/{comment_id}",
            delete(kb_comments::remove),
        ),
        (
            "/api/v1/projects/{id}/kb/comments/{comment_id}/done",
            post(kb_comments::set_done),
        ),
        ("/api/v1/projects/{id}/kb/history", get(kb::history)),
        (
            "/api/v1/projects/{id}/kb/versions",
            get(kb::versions).delete(kb::forget_versions),
        ),
        ("/api/v1/projects/{id}/kb/backlinks", get(kb::backlinks)),
        ("/api/v1/projects/{id}/kb/lint", get(kb::lint)),
        ("/api/v1/projects/{id}/kb/stats", get(kb::stats)),
        ("/api/v1/sessions", get(sessions::list)),
        ("/api/v1/sessions/{id}", get(sessions::detail)),
        ("/api/v1/sessions/{id}/end", post(sessions::end)),
        ("/api/v1/sessions/{id}/brain", get(sessions::brain)),
        ("/api/v1/sessions/{id}/replay", get(sessions::replay)),
        ("/api/v1/sessions/{id}/resume", get(sessions::resume)),
        ("/api/v1/sessions/{id}/scores", post(sessions::score)),
        (
            "/api/v1/sessions/{id}/brain/entry",
            get(sessions::brain_entry),
        ),
        ("/api/v1/storage", get(storage::usage)),
        ("/api/v1/storage/sessions", delete(storage::prune_all)),
        ("/api/v1/storage/sessions/{id}", delete(storage::prune)),
        (
            "/api/v1/storage/projects/{id}/sessions",
            delete(storage::prune_project),
        ),
        ("/api/v1/prune/undo/{token}", post(storage::undo)),
        ("/api/v1/backups", post(backup::create)),
        (
            "/api/v1/notify/targets",
            get(notify::list).post(notify::create),
        ),
        (
            "/api/v1/notify/targets/{id}",
            patch(notify::change).delete(notify::delete),
        ),
        ("/api/v1/notify/targets/{id}/test", post(notify::test)),
        ("/api/v1/search", get(search::search)),
        ("/id/{id}", get(resolve::redirect)),
        ("/api/v1/resolve/{id}", get(resolve::resolve)),
        ("/artifacts/{id}", get(artifacts::host)),
        ("/artifacts/{id}/frame", get(artifacts::frame)),
        ("/artifacts/{id}/og.svg", get(artifacts::og_svg)),
        ("/s/{token}", get(artifacts::share_host)),
        ("/s/{token}/frame", get(artifacts::share_frame)),
        ("/s/{token}/og.svg", get(artifacts::share_og_svg)),
    ]
}

/// The path of every route the router serves, assets included.
pub fn route_paths() -> Vec<&'static str> {
    web::asset_paths()
        .chain(routes().into_iter().map(|(path, _)| path))
        .collect()
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
