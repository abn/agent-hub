//! Project REST routes: list, create, and delete.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::store::projects::{self, Project};

/// The projects a caller can see.
#[derive(Debug, Serialize)]
pub struct ProjectList {
    /// The projects, oldest first.
    pub projects: Vec<Project>,
}

/// A project to create.
#[derive(Debug, Deserialize)]
pub struct NewProject {
    /// The immutable slug.
    pub id: String,
    /// The display name.
    pub display_name: String,
}

/// `GET /api/v1/projects`
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<ProjectList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let projects = projects::list(&state.db)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(ProjectList { projects }))
}

/// `POST /api/v1/projects`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: std::result::Result<Json<NewProject>, axum::extract::rejection::JsonRejection>,
) -> std::result::Result<Json<Project>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "project body must be JSON with id and display_name")?;

    let project = projects::create(&state.db, &payload.id, &payload.display_name)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(project))
}

/// `DELETE /api/v1/projects/{id}`
///
/// A valid bearer token is required. An unknown project is a 404, and an
/// agent's personal space is a 409.
pub async fn delete(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    projects::delete(&state.db, &state.data_dir, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(StatusCode::NO_CONTENT)
}
