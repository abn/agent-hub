//! Session REST routes: list a project's sessions, end one, and read a brain.

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

/// The prefix whose brain entries are listed.
#[derive(Debug, Deserialize)]
pub struct BrainParams {
    /// A `/kv` or `/fs` prefix, defaulting to every namespace.
    pub path: Option<String>,
}

/// A session's brain entries under a prefix.
#[derive(Debug, Serialize)]
pub struct BrainList {
    /// The entry paths, sorted.
    pub entries: Vec<String>,
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
        .require_admin(bearer_token(&headers).as_deref())
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
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    session_store::end(&state.db, &session_id, &principal.actor)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(EndResult { ok: true }))
}

/// `GET /api/v1/sessions/{id}/brain?path=`
///
/// A valid bearer token is required. An unknown session is a 404. The optional
/// `path` is a `/kv` or `/fs` prefix; omitted, both namespaces are listed.
pub async fn brain(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<BrainParams>,
) -> std::result::Result<Json<BrainList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let session = session_store::get(&state.db, &session_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("session {session_id} not found")))
        })?;

    // A read does not create a brain: a session whose file is absent simply
    // has no entries yet.
    let brain = match state
        .brain
        .open_existing(&session.project_id, &session.id)
        .await
    {
        Ok(Some(brain)) => brain,
        Ok(None) => {
            return Ok(Json(BrainList {
                entries: Vec::new(),
            }));
        }
        Err(err) => return Err(Problem::from_error(&err)),
    };

    let entries = list_entries(&brain, params.path.as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(BrainList { entries }))
}

async fn list_entries(
    brain: &crate::brain::Brain,
    path: Option<&str>,
) -> crate::error::Result<Vec<String>> {
    match path {
        Some(path) => brain.list(path).await,
        None => {
            let mut entries = brain.list("/kv").await?;
            entries.extend(brain.list("/fs").await?);
            Ok(entries)
        }
    }
}
