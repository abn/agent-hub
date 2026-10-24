//! Human idea capture: a human posts an idea into a project's feed as an open
//! item, so it lands in the inbox for triage.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use serde::Deserialize;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::store::events::{self, Event, NewEvent};
use crate::store::projects;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdeaBody {
    pub summary: String,
    pub payload: Option<serde_json::Value>,
    /// The idea this is an addendum to. Absent for the idea itself.
    #[serde(default)]
    pub thread_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct IdeaResult {
    pub id: String,
}

pub async fn create(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<IdeaBody>, JsonRejection>,
) -> std::result::Result<Json<IdeaResult>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "idea body must be JSON with a summary")?;

    if payload.summary.trim().is_empty() {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "summary must not be empty".to_string(),
        )));
    }

    // A missing project is a 404 for the admin, matching the other routes.
    projects::get(&state.db, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("project {project_id} not found")))
        })?;

    let id = events::append(
        &state.db,
        state.config.events_per_project.per_project,
        "human",
        None,
        NewEvent {
            project_id,
            kind: "idea".to_string(),
            summary: payload.summary,
            payload: payload.payload,
            needs_action: payload.thread_id.is_none(),
            thread_id: payload.thread_id,
            session_id: None,
        },
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(IdeaResult { id }))
}

/// `POST /api/v1/ideas/{id}/resolve`
///
/// A valid admin token is required. An idea that is not in the inbox, or is not
/// an idea, is a problem.
pub async fn resolve(
    State(state): State<AppState>,
    ProblemPath(idea_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<IdeaResult>, Problem> {
    mark(state, headers, idea_id, "resolved").await
}

/// `POST /api/v1/ideas/{id}/clear`
///
/// Clears an idea from the inbox without claiming it was acted on.
pub async fn clear(
    State(state): State<AppState>,
    ProblemPath(idea_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<IdeaResult>, Problem> {
    mark(state, headers, idea_id, "cleared").await
}

async fn mark(
    state: AppState,
    headers: HeaderMap,
    idea_id: String,
    status: &str,
) -> std::result::Result<Json<IdeaResult>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let idea = load_idea(&state, &idea_id).await?;
    let result = if status == "resolved" {
        crate::store::ideas::resolve(&state.db, &idea.id).await
    } else {
        crate::store::ideas::clear(&state.db, &idea.id).await
    };
    result.map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(IdeaResult { id: idea_id }))
}

/// `POST /api/v1/ideas/{id}/reply`
///
/// Logs an addendum on the idea's thread. The addendum does not enter the
/// inbox: it is a note on the idea, not a new open item.
pub async fn reply(
    State(state): State<AppState>,
    ProblemPath(idea_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<IdeaBody>, JsonRejection>,
) -> std::result::Result<Json<IdeaResult>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let idea = load_idea(&state, &idea_id).await?;
    let payload = json_body(body, "idea body must be JSON with a summary")?;
    if payload.summary.trim().is_empty() {
        return Err(Problem::from_error(&Error::InvalidArgument(
            "summary must not be empty".to_string(),
        )));
    }

    // An addendum always lands on the idea's root thread, even when the caller
    // named one of its replies.
    let root_id = idea
        .thread_id
        .clone()
        .filter(|thread| thread != &idea_id)
        .unwrap_or_else(|| idea_id.clone());
    let id = events::append(
        &state.db,
        state.config.events_per_project.per_project,
        "human",
        None,
        NewEvent {
            project_id: idea.project_id,
            kind: "idea".to_string(),
            summary: payload.summary,
            payload: payload.payload,
            needs_action: false,
            thread_id: Some(root_id),
            session_id: None,
        },
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(IdeaResult { id }))
}

async fn load_idea(state: &AppState, idea_id: &str) -> std::result::Result<Event, Problem> {
    let event = events::get(&state.db, idea_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!("idea {idea_id} not found")))
        })?;
    if event.kind != "idea" {
        return Err(Problem::from_error(&Error::InvalidArgument(format!(
            "event {idea_id} is a {} and is not an idea",
            event.kind
        ))));
    }
    Ok(event)
}
