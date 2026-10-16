//! Human idea capture: a human posts an idea into a project's feed as an open
//! item, so it lands in the inbox for triage.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::Deserialize;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::store::events::{NewEvent, append};
use crate::store::projects;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdeaBody {
    pub summary: String,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct IdeaResult {
    pub id: String,
}

pub async fn create(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<IdeaBody>, axum::extract::rejection::JsonRejection>,
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

    let id = append(
        &state.db,
        state.config.events_per_project.per_project,
        "human",
        None,
        NewEvent {
            project_id: project_id.clone(),
            kind: "idea".to_string(),
            summary: payload.summary,
            payload: payload.payload,
            needs_action: true,
            thread_id: None,
            session_id: None,
        },
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(IdeaResult { id }))
}
