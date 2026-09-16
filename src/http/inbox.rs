//! The home summary, the inbox listing, and answering a question.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::store::home::{self as home_store, Home};
use crate::store::inbox::{self as inbox_store, InboxItem};
use crate::store::questions as question_store;

/// How many recent events the home summary carries.
const HOME_RECENT_LIMIT: i64 = 10;

/// The optional filters for an inbox listing.
#[derive(Debug, Deserialize)]
pub struct InboxParams {
    /// Restrict to one status.
    pub status: Option<String>,
    /// Restrict to one project.
    pub project: Option<String>,
}

/// The inbox entries matching the filters.
#[derive(Debug, Serialize)]
pub struct InboxList {
    /// The entries, most recently updated first.
    pub items: Vec<InboxItem>,
}

/// The answer to post against a question.
#[derive(Debug, Deserialize)]
pub struct AnswerBody {
    /// The answer text.
    pub body: String,
    /// Optional idempotency key, so a retried answer does not duplicate.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// The acknowledgement returned when a question is answered.
#[derive(Debug, Serialize)]
pub struct AnswerResult {
    /// The id of the answer event that was appended.
    pub event_id: String,
}

/// `GET /api/v1/home`
///
/// A valid bearer token is required.
pub async fn home(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<Home>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let summary = home_store::home(&state.db, HOME_RECENT_LIMIT)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(summary))
}

/// `GET /api/v1/inbox?status=&project=`
///
/// A valid bearer token is required. A status outside the store's set is a
/// 400 problem; an absent or empty filter matches everything.
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<InboxParams>,
) -> std::result::Result<Json<InboxList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let status = params.status.as_deref().filter(|status| !status.is_empty());
    let project = params
        .project
        .as_deref()
        .filter(|project| !project.is_empty());

    let items = inbox_store::list(
        &state.db,
        status,
        project,
        crate::limits::FEED_LIMIT_DEFAULT,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(InboxList { items }))
}

/// `POST /api/v1/questions/{id}/answer`
///
/// A valid bearer token is required. The answer is recorded under the token's
/// principal. An unknown id is a 404, a non-question id a 400.
pub async fn answer(
    State(state): State<AppState>,
    Path(question_id): Path<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<AnswerBody>, JsonRejection>,
) -> std::result::Result<Json<AnswerResult>, Problem> {
    let principal = state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let Json(payload) = body.map_err(|rejection| {
        Problem::from_error(&Error::InvalidArgument(format!(
            "the answer body must be JSON with a body field: {rejection}"
        )))
    })?;

    let event_id = question_store::answer(
        &state.db,
        &principal.actor,
        &question_id,
        &payload.body,
        payload.idempotency_key.as_deref(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(AnswerResult { event_id }))
}
