//! Comment threads on knowledge base pages.
//!
//! The human surface over a page's discussion, the same shape as the artifact
//! comment routes: list, post, resolve and delete, every one behind the admin
//! gate. A page has no id, so a comment is keyed by the project and the
//! canonical page path, and the path is normalised the way the knowledge base
//! normalises it before anything is stored.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;
use crate::http::kb;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery, json_body};
use crate::store::page_comments;

/// The query for a page's comment thread.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentsQuery {
    pub path: String,
}

/// The body for posting a comment on a page.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentPostBody {
    pub path: String,
    pub body: String,
    /// Who the comment is recorded under. Absent means the human.
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub anchor: Option<Value>,
}

/// The resolution flip for a comment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentDoneBody {
    pub done: bool,
}

/// `GET /api/v1/projects/{id}/kb/comments?path=<page>`
///
/// Admin-only. A page with no thread is an empty list.
pub async fn list(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<CommentsQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    kb::check_access(&state, &headers, &project_id).await?;
    let path = kb::page_path(&query.path)?;

    let comments = page_comments::list_comments(&state.db, &project_id, &path)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(json!({ "comments": comments })))
}

/// `POST /api/v1/projects/{id}/kb/comments`
///
/// Admin-only. The author defaults to the human.
pub async fn create(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<CommentPostBody>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Value>), Problem> {
    kb::check_access(&state, &headers, &project_id).await?;
    let payload = json_body(body, "comment body must be JSON with path and body fields")?;
    let path = kb::page_path(&payload.path)?;
    let author = payload.author.as_deref().unwrap_or("human");

    let comment = page_comments::add_comment(
        &state.db,
        &project_id,
        &path,
        author,
        &payload.body,
        payload.anchor,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok((StatusCode::CREATED, Json(json!({ "comment": comment }))))
}

/// `POST /api/v1/projects/{id}/kb/comments/{comment_id}/done`
///
/// Admin-only. Flips the resolution of one comment. A comment of another
/// project, like an unknown id, is a 404.
pub async fn set_done(
    State(state): State<AppState>,
    ProblemPath((project_id, comment_id)): ProblemPath<(String, String)>,
    headers: HeaderMap,
    body: std::result::Result<Json<CommentDoneBody>, JsonRejection>,
) -> std::result::Result<Json<Value>, Problem> {
    kb::check_access(&state, &headers, &project_id).await?;
    let payload = json_body(body, "done body must be JSON with a done field")?;

    let comment = page_comments::get_comment(&state.db, &comment_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    if comment.project_id != project_id {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "comment {comment_id} not found"
        ))));
    }
    let updated = page_comments::set_comment_done(&state.db, &comment_id, payload.done)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(json!({ "comment": updated })))
}

/// `DELETE /api/v1/projects/{id}/kb/comments/{comment_id}`
///
/// Admin-only. Removes one comment. A comment of another project, like an
/// unknown id, is a 404.
pub async fn remove(
    State(state): State<AppState>,
    ProblemPath((project_id, comment_id)): ProblemPath<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    kb::check_access(&state, &headers, &project_id).await?;

    let comment = page_comments::get_comment(&state.db, &comment_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    if comment.project_id != project_id {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "comment {comment_id} not found"
        ))));
    }
    page_comments::delete_comment(&state.db, &comment_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(StatusCode::NO_CONTENT)
}
