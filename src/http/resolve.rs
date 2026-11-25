//! Short resource URLs: `/id/{id}` and the lookup behind it.
//!
//! Every id-bearing resource has one address, `<hub>/id/<id>`. The redirect
//! looks nothing up, so it reveals nothing. The PWA calls the resolver, which
//! answers where the id lives under the caller's read access.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::{Error, ErrorCode};
use crate::http::auth::agent_principal;
use crate::http::origin::encode;
use crate::http::problem::{Problem, ProblemPath};
use crate::policy::{self, Access};
use crate::store::{artifacts, comments, events, page_comments, sessions};

const ID_CHARS_MAX: usize = 64;

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_CHARS_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `GET /id/{id}`
///
/// No credential and no lookup. The location is relative so a proxy that
/// strips a path prefix keeps it.
pub async fn redirect(ProblemPath(id): ProblemPath<String>) -> Response {
    if !valid_id(&id) {
        return Problem::from_error(&Error::NotFound("not found".to_string())).into_response();
    }
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, format!("../#/id/{id}")),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

/// `GET /api/v1/resolve/{id}`
///
/// A member token or the admin. A missing id and one the caller may not read
/// answer the same 404.
pub async fn resolve(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, Problem> {
    let principal = agent_principal(&state, &headers).await?;
    let missing = || Problem::from_error(&Error::NotFound(format!("{id} not found")));
    if !valid_id(&id) {
        return Err(missing());
    }
    let found = lookup(&state, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .ok_or_else(missing)?;
    match policy::authorize(&state.db, &principal, found.project_id(), Access::Read).await {
        Ok(()) => Ok(Json(found.body())),
        Err(err) if matches!(err.code(), ErrorCode::Forbidden | ErrorCode::NotFound) => {
            Err(missing())
        }
        Err(err) => Err(Problem::from_error(&err)),
    }
}

enum Found {
    Artifact {
        id: String,
        project_id: String,
    },
    Event {
        id: String,
        project_id: String,
    },
    Session {
        id: String,
        project_id: String,
    },
    ArtifactComment {
        id: String,
        project_id: String,
        artifact_id: String,
    },
    PageComment {
        id: String,
        project_id: String,
        page: String,
    },
}

impl Found {
    fn project_id(&self) -> &str {
        match self {
            Self::Artifact { project_id, .. }
            | Self::Event { project_id, .. }
            | Self::Session { project_id, .. }
            | Self::ArtifactComment { project_id, .. }
            | Self::PageComment { project_id, .. } => project_id,
        }
    }

    fn body(&self) -> Value {
        match self {
            Self::Artifact { id, project_id } => json!({
                "kind": "artifact",
                "id": id,
                "project_id": project_id,
                "route": format!("#/artifacts/{}", encode(id)),
            }),
            Self::Event { id, project_id } => json!({
                "kind": "event",
                "id": id,
                "project_id": project_id,
                "route": format!(
                    "#/projects/{}/feed?event={}",
                    encode(project_id),
                    encode(id)
                ),
            }),
            Self::Session { id, project_id } => json!({
                "kind": "session",
                "id": id,
                "project_id": project_id,
                "route": format!(
                    "#/projects/{}/sessions?session={}",
                    encode(project_id),
                    encode(id)
                ),
            }),
            Self::ArtifactComment {
                id,
                project_id,
                artifact_id,
            } => json!({
                "kind": "comment",
                "id": id,
                "project_id": project_id,
                "artifact_id": artifact_id,
                "route": format!(
                    "#/artifacts/{}?comment={}",
                    encode(artifact_id),
                    encode(id)
                ),
            }),
            Self::PageComment {
                id,
                project_id,
                page,
            } => json!({
                "kind": "comment",
                "id": id,
                "project_id": project_id,
                "page": page,
                "route": format!(
                    "#/projects/{}/wiki?page={}&comment={}",
                    encode(project_id),
                    encode(page),
                    encode(id)
                ),
            }),
        }
    }
}

/// The resource an id names, in the order artifact, feed event, session,
/// comment. Ids are ULIDs, so at most one kind matches.
async fn lookup(state: &AppState, id: &str) -> crate::error::Result<Option<Found>> {
    match artifacts::project_id(&state.db, id).await {
        Ok(project_id) => {
            return Ok(Some(Found::Artifact {
                id: id.to_string(),
                project_id,
            }));
        }
        Err(err) if err.code() == ErrorCode::NotFound => {}
        Err(err) => return Err(err),
    }
    if let Some(event) = events::get(&state.db, id).await? {
        return Ok(Some(Found::Event {
            id: event.id,
            project_id: event.project_id,
        }));
    }
    if let Some(session) = sessions::lookup_id(&state.db, id)
        .await?
        .filter(|session| session.deleted_at.is_none())
    {
        return Ok(Some(Found::Session {
            id: session.id,
            project_id: session.project_id,
        }));
    }
    match comments::get_comment(&state.db, id).await {
        Ok(comment) => {
            let project_id = artifacts::project_id(&state.db, &comment.artifact_id).await?;
            return Ok(Some(Found::ArtifactComment {
                id: comment.id,
                project_id,
                artifact_id: comment.artifact_id,
            }));
        }
        Err(err) if err.code() == ErrorCode::NotFound => {}
        Err(err) => return Err(err),
    }
    match page_comments::get_comment(&state.db, id).await {
        Ok(comment) => Ok(Some(Found::PageComment {
            id: comment.id,
            project_id: comment.project_id,
            page: comment.path,
        })),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}
