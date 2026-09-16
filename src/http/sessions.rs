//! Session REST routes: list a project's sessions and end one.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::store::sessions as session_store;
use crate::store::sessions::Session;

/// The project filter for a session listing.
#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// The project whose sessions are listed.
    pub project: Option<String>,
}

/// A project's sessions.
#[derive(Debug, Serialize)]
pub struct SessionList {
    /// The sessions, most recently active first.
    pub sessions: Vec<Session>,
}

/// The acknowledgement returned when a session is ended.
#[derive(Debug, Serialize)]
pub struct EndResult {
    /// Always true on success.
    pub ok: bool,
}

/// `GET /api/v1/sessions?project=<id>`
///
/// A valid bearer token is required. The `project` filter is mandatory.
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> std::result::Result<Json<SessionList>, Problem> {
    state
        .auth
        .resolve_bearer(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let project = params
        .project
        .filter(|project| !project.is_empty())
        .ok_or_else(|| {
            Problem::from_error(&Error::InvalidArgument(
                "the project query parameter is required".to_string(),
            ))
        })?;

    let sessions = session_store::list(&state.db, &project)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(SessionList { sessions }))
}

/// `POST /api/v1/sessions/{id}/end`
///
/// A valid bearer token is required. An unknown session is a 404.
pub async fn end(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<EndResult>, Problem> {
    let principal = state
        .auth
        .resolve_bearer(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    session_store::end(&state.db, &session_id, &principal.actor)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(EndResult { ok: true }))
}
