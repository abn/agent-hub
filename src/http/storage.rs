//! Storage and prune REST routes: soft-delete a session, then undo it.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;

use crate::app::AppState;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath};
use crate::store::prune::{self, PruneToken};
use crate::store::storage::StorageUsage;

/// The acknowledgement returned when a prune is undone.
#[derive(Debug, Serialize)]
pub struct UndoResult {
    /// Always true on success.
    pub ok: bool,
}

/// `DELETE /api/v1/storage/sessions/{id}`
///
/// A valid bearer token is required. An unknown session is a 404. The response
/// carries the token that restores the session within the undo window.
pub async fn prune(
    State(state): State<AppState>,
    ProblemPath(session_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<PruneToken>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let token = prune::prune_session(&state.db, &session_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(token))
}

/// `POST /api/v1/prune/undo/{token}`
///
/// A valid bearer token is required. Undoing restores the session's row.
pub async fn undo(
    State(state): State<AppState>,
    ProblemPath(token): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<UndoResult>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    prune::undo(&state.db, &token)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(UndoResult { ok: true }))
}

/// `GET /api/v1/storage`
///
/// A valid bearer token is required. Reports what the data volume holds, by
/// kind and by project, what a prune would reclaim, and the volume's own
/// capacity and free space when they can be measured.
pub async fn usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<StorageUsage>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let usage = state
        .stats
        .usage(&state.db, &state.data_dir, &state.host, state.generation())
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(usage))
}
