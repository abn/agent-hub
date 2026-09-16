//! Storage and prune REST routes: soft-delete a session, then undo it.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Serialize;

use crate::app::AppState;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::store::prune::{self, PruneToken};

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
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<PruneToken>, Problem> {
    state
        .auth
        .resolve_bearer(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let token = prune::prune_session(&state.db, &session_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(token))
}

/// `POST /api/v1/prune/undo/{token}`
///
/// A valid bearer token is required. Undoing restores the session's row.
pub async fn undo(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<UndoResult>, Problem> {
    state
        .auth
        .resolve_bearer(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    prune::undo(&state.db, &token)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(UndoResult { ok: true }))
}
