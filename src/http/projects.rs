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
    /// Whether the project is confidential.
    #[serde(default)]
    pub confidential: bool,
}

/// `GET /api/v1/projects`
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<ProjectList>, Problem> {
    let principal = state
        .auth
        .resolve_agent(&state.db, bearer_token(&headers).as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let projects = projects::list_visible(&state.db, &state.config.active_since(), &principal)
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
    let _principal = state
        .auth
        .resolve_agent(&state.db, bearer_token(&headers).as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "project body must be JSON with id and display_name")?;

    let project = projects::create_with_confidential(
        &state.db,
        &payload.id,
        &payload.display_name,
        payload.confidential,
    )
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
    pub confidential: Option<bool>,
}

/// `GET /api/v1/projects/{id}`
///
/// A valid bearer token is required. An unknown project is a 404.
pub async fn get(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Project>, Problem> {
    let principal = state
        .auth
        .resolve_agent(&state.db, bearer_token(&headers).as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let project = projects::get(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let visible = match (&project, principal.is_admin) {
        (Some(_), true) => true,
        (Some(p), false) => {
            if !p.confidential {
                true
            } else if let Some(agent_id) = principal.agent_id.as_deref() {
                crate::store::identity::has_grant(&state.db, agent_id, &p.id)
                    .await
                    .map_err(|err| Problem::from_error(&err))?
            } else {
                false
            }
        }
        (None, _) => false,
    };

    if visible {
        Ok(Json(project.unwrap()))
    } else {
        Err(Problem::from_error(&crate::error::Error::NotFound(
            format!("project {id} not found"),
        )))
    }
}

/// `PATCH /api/v1/projects/{id}`
///
/// A valid bearer token is required. A field the body does not name is left
/// alone, an attempt to change the id is a 400, and an unknown project is a
/// 404. A body that names neither field is an accepted no-op, and one that
/// writes nothing wakes nobody. An agent's personal space is settable here,
/// unlike on delete.
///
/// An agent may **tighten** a project it can write: it may set `confidential`,
/// and only to true. Renaming and making a project public again are the
/// admin's, the asymmetry the operating model states.
pub async fn update(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<ProjectPatch>, axum::extract::rejection::JsonRejection>,
) -> std::result::Result<Json<Project>, Problem> {
    let principal = state
        .auth
        .resolve_agent(&state.db, bearer_token(&headers).as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "project body must be JSON with display_name")?;

    if payload.id.is_some() {
        return Err(Problem::from_error(&crate::error::Error::InvalidArgument(
            "a project id is read-only after creation; it names the project in every MCP call"
                .to_string(),
        )));
    }

    if !principal.is_admin {
        if payload.display_name.is_some() {
            return Err(Problem::from_error(&crate::error::Error::InvalidArgument(
                "only the admin may rename a project".to_string(),
            )));
        }
        if payload.confidential != Some(true) {
            return Err(Problem::from_error(&crate::error::Error::InvalidArgument(
                "an agent may make a project confidential; only the admin may make it public"
                    .to_string(),
            )));
        }
        // It must be able to write the project, and a missing one reads as a
        // denial like every other non-admin refusal.
        crate::policy::authorize(&state.db, &principal, &id, crate::policy::Access::Write)
            .await
            .map_err(|err| Problem::from_error(&err))?;
    }

    let changes = projects::ProjectChanges {
        display_name: payload.display_name.as_deref(),
        confidential: payload.confidential,
    };
    let changed = changes.display_name.is_some() || changes.confidential.is_some();

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
    let principal = state
        .auth
        .resolve_agent(&state.db, bearer_token(&headers).as_deref())
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let project = projects::get(&state.db, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let visible = match (&project, principal.is_admin) {
        (Some(_), true) => true,
        (Some(p), false) => {
            if !p.confidential {
                true
            } else if let Some(agent_id) = principal.agent_id.as_deref() {
                crate::store::identity::has_grant(&state.db, agent_id, &p.id)
                    .await
                    .map_err(|err| Problem::from_error(&err))?
            } else {
                false
            }
        }
        (None, _) => false,
    };

    if !visible {
        return Err(Problem::from_error(&crate::error::Error::NotFound(
            format!("project {id} not found"),
        )));
    }

    let stats = projects::stats(
        &state.db,
        Some(&state.data_dir),
        &id,
        &state.config.active_since(),
    )
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

    // The project's events went with it, and the storage report only ever
    // adds to what it has weighed.
    state.stats.forget_events();
    state.notify();
    Ok(StatusCode::NO_CONTENT)
}
