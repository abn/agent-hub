//! The home summary, the inbox listing, and answering a question.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery, json_body};
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
    /// Maximum entries to return, clamped by the store to the feed page cap.
    pub limit: Option<i64>,
    /// Keep only what has not been read, which is the Inbox header's filter.
    pub unread_only: Option<bool>,
}

/// The inbox entries matching the filters.
#[derive(Debug, Serialize)]
pub struct InboxList {
    /// The entries, most recently updated first.
    pub items: Vec<InboxItem>,
}

/// The optional confinement for marking everything read.
#[derive(Debug, Deserialize, Default)]
pub struct ReadAllBody {
    /// Restrict to one project; absent means every project.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// How many entries a bulk read moved.
#[derive(Debug, Serialize)]
pub struct ReadAllResult {
    pub marked: i64,
}

/// `POST /api/v1/inbox/{event_id}/read`
///
/// A valid bearer token is required. Idempotent: an entry already read is
/// answered with `changed` false, and so is one that waits on the human or has
/// been resolved, since neither carries read state. An event with no inbox
/// entry is a 404.
pub async fn read(
    State(state): State<AppState>,
    ProblemPath(event_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<inbox_store::ReadState>, Problem> {
    mark(state, &headers, &event_id, true).await
}

/// `POST /api/v1/inbox/{event_id}/unread`
///
/// The other direction of the swipe, on the same terms as [`read`].
pub async fn unread(
    State(state): State<AppState>,
    ProblemPath(event_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<inbox_store::ReadState>, Problem> {
    mark(state, &headers, &event_id, false).await
}

async fn mark(
    state: AppState,
    headers: &HeaderMap,
    event_id: &str,
    read: bool,
) -> std::result::Result<Json<inbox_store::ReadState>, Problem> {
    state
        .auth
        .require_admin(bearer_token(headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let marked = if read {
        inbox_store::mark_read(&state.db, event_id).await
    } else {
        inbox_store::mark_unread(&state.db, event_id).await
    }
    .map_err(|err| Problem::from_error(&err))?;

    if marked.changed {
        state.notify();
    }
    Ok(Json(marked))
}

/// `POST /api/v1/inbox/read-all`
///
/// A valid bearer token is required. The optional `project_id` confines it to
/// one project. Only unread entries move: what waits on the human stays where
/// it is.
pub async fn read_all(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: std::result::Result<Json<ReadAllBody>, JsonRejection>,
) -> std::result::Result<Json<ReadAllResult>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    // An empty body is the whole inbox, which is what the header control does.
    let payload = match body {
        Ok(Json(payload)) => payload,
        Err(_) => ReadAllBody::default(),
    };
    let project = payload
        .project_id
        .as_deref()
        .filter(|project| !project.is_empty());

    let marked = inbox_store::mark_all_read(&state.db, project)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    if marked > 0 {
        state.notify();
    }
    Ok(Json(ReadAllResult { marked }))
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

/// The acknowledgement returned when a question is answered or an approval is
/// decided.
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

    // The file and volume numbers come from the memo, so Home's one request
    // does not restat the data directory on every poll.
    let usage = state
        .stats
        .usage(&state.db, &state.data_dir, &state.host, state.generation())
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let summary = home_store::home(
        &state.db,
        HOME_RECENT_LIMIT,
        &state.config.active_since(),
        usage,
    )
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
    ProblemQuery(params): ProblemQuery<InboxParams>,
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

    // The two ways of naming a status must not disagree: a listing that
    // silently honoured one of them would answer a question nobody asked.
    let status = match (params.unread_only.unwrap_or(false), status) {
        (true, Some(status)) if status != "unread" => {
            return Err(Problem::from_error(&Error::InvalidArgument(format!(
                "unread_only asks for unread entries and status asks for '{status}'"
            ))));
        }
        (true, _) => Some("unread"),
        (false, status) => status,
    };

    let limit = params.limit.unwrap_or(crate::limits::FEED_LIMIT_DEFAULT);
    let items = inbox_store::list(&state.db, status, project, limit)
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
    ProblemPath(question_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<AnswerBody>, JsonRejection>,
) -> std::result::Result<Json<AnswerResult>, Problem> {
    let principal = state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "answer body must be JSON with a body field")?;

    let event_id = question_store::answer(
        &state.db,
        &principal.actor,
        &question_id,
        &payload.body,
        payload.idempotency_key.as_deref(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(AnswerResult { event_id }))
}

/// The body for a decision on an approval.
#[derive(Debug, Deserialize)]
pub struct DecisionBody {
    /// The decision: `approve` or `decline`.
    pub decision: String,
    /// Optional note recorded with the decision.
    #[serde(default)]
    pub note: Option<String>,
    /// Optional idempotency key, so a retried decision returns the original.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// `POST /api/v1/approvals/{id}/decision`
///
/// A valid bearer token is required. The decision is recorded on the feed and
/// resolves the waiting item. An unknown id is a 404, a non-approval id a 400.
pub async fn decide(
    State(state): State<AppState>,
    ProblemPath(approval_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<DecisionBody>, JsonRejection>,
) -> std::result::Result<Json<AnswerResult>, Problem> {
    let principal = state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "decision body must be JSON with a decision field")?;

    let approved = match payload.decision.as_str() {
        "approve" => true,
        "decline" => false,
        other => {
            return Err(Problem::from_error(&Error::InvalidArgument(format!(
                "decision must be approve or decline, got '{other}'"
            ))));
        }
    };

    let event_id = question_store::decide(
        &state.db,
        &principal.actor,
        &approval_id,
        approved,
        payload.note.as_deref(),
        payload.idempotency_key.as_deref(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(AnswerResult { event_id }))
}
