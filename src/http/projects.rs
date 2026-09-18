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

/// The fields of a project a settings screen may change.
///
/// The id is read-only after creation, so it is named here only to refuse it:
/// a body that carries one is a change nobody can make. Every other unknown
/// field is ignored, as it is on the create route.
#[derive(Debug, Deserialize)]
pub struct ProjectPatch {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub artifact_password_policy: Option<String>,
}

/// `GET /api/v1/projects/{id}`
///
/// A valid bearer token is required. An unknown project is a 404.
pub async fn get(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Project>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    projects::get(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .map(Json)
        .ok_or_else(|| {
            Problem::from_error(&crate::error::Error::NotFound(format!(
                "project {id} not found"
            )))
        })
}

/// `PATCH /api/v1/projects/{id}`
///
/// A valid bearer token is required. A field the body does not name is left
/// alone, an attempt to change the id is a 400, and an unknown project is a
/// 404. A body that names neither field is an accepted no-op, and one that
/// writes nothing wakes nobody. An agent's personal space is settable here,
/// unlike on delete.
pub async fn update(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<ProjectPatch>, axum::extract::rejection::JsonRejection>,
) -> std::result::Result<Json<Project>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(
        body,
        "project body must be JSON with display_name or artifact_password_policy",
    )?;

    if payload.id.is_some() {
        return Err(Problem::from_error(&crate::error::Error::InvalidArgument(
            "a project id is read-only after creation; it names the project in every MCP call"
                .to_string(),
        )));
    }

    let changes = projects::ProjectChanges {
        display_name: payload.display_name.as_deref(),
        artifact_password_policy: payload.artifact_password_policy.as_deref(),
    };
    let changed = changes.display_name.is_some() || changes.artifact_password_policy.is_some();

    let project = projects::update(&state.db, &id, changes)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    if changed {
        state.notify();
    }
    Ok(Json(project))
}

/// `GET /api/v1/projects/{id}/stats`
///
/// A valid bearer token is required. An unknown project is a 404.
pub async fn stats(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<projects::ProjectStats>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    if projects::get(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .is_none()
    {
        return Err(Problem::from_error(&crate::error::Error::NotFound(
            format!("project {id} not found"),
        )));
    }

    let stats = projects::stats(&state.db, &id, &state.config.active_since())
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(stats))
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
