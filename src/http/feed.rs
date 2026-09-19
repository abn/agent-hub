//! The project feed REST route.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::limits::FEED_LIMIT_DEFAULT;
use crate::store::events::{self, Event, FeedQuery, read_feed};

/// A page of the feed plus the cursors to continue in either direction.
#[derive(Debug, Serialize)]
pub struct FeedPage {
    /// The events in this page.
    pub events: Vec<Event>,
    /// Newest id on the page: pass as `since` to poll for newer events.
    pub next_since: Option<String>,
    /// Oldest id on the page: pass as `before` to page further back.
    pub next_before: Option<String>,
    /// The newest event the human has seen in this project, absent until the
    /// feed has been opened. An event above it is unseen.
    pub last_seen: Option<String>,
}

/// The event the human has read down to.
#[derive(Debug, Deserialize)]
pub struct SeenBody {
    /// The newest event on the page the human just read.
    pub event_id: String,
}

/// `GET /api/v1/projects/{id}/feed`
///
/// Query parameters are `since`, `before`, `limit`, and repeatable `kinds`.
/// A valid bearer token is required; the resolved principal is not recorded
/// on a read.
pub async fn read(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> std::result::Result<Json<FeedPage>, Problem> {
    let token = bearer_token(&headers);
    state
        .auth
        .require_admin(token.as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let mut query = parse_query(raw.as_deref()).map_err(|err| Problem::from_error(&err))?;
    // The human feed hides the hub's own audit events unless they are asked
    // for by kind.
    if query.kinds.is_none() {
        query.kinds = Some(events::human_kinds());
    }

    let page = read_feed(&state.db, &project_id, &query)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let last_seen = events::last_seen(&state.db, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(FeedPage {
        events: page.events,
        next_since: page.next_since,
        next_before: page.next_before,
        last_seen,
    }))
}

/// `POST /api/v1/projects/{id}/feed/seen`
///
/// A valid bearer token is required. The cursor moves up to the given event
/// and never back: an older event, an event of another project, and an id that
/// names nothing all leave it where it is, and the response says so. An
/// unknown project is a 404.
pub async fn seen(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<SeenBody>, JsonRejection>,
) -> std::result::Result<Json<events::Seen>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "the body must be JSON with an event_id field")?;

    if crate::store::projects::get(&state.db, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .is_none()
    {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "project {project_id} not found"
        ))));
    }

    let seen = events::mark_seen(&state.db, &project_id, &payload.event_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    if seen.advanced {
        state.notify();
    }
    Ok(Json(seen))
}

// Query parsing uses form_urlencoded to handle repeated keys and standard decoding.
fn parse_query(raw: Option<&str>) -> std::result::Result<FeedQuery, Error> {
    let mut since = None;
    let mut before = None;
    let mut limit = None;
    let mut kinds: Vec<String> = Vec::new();

    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "since" => since = Some(value.into_owned()),
            "before" => before = Some(value.into_owned()),
            "limit" => {
                limit = Some(value.parse::<i64>().map_err(|_| {
                    Error::InvalidArgument(format!("limit must be an integer, got '{value}'"))
                })?);
            }
            "kinds" => {
                if !events::KINDS.contains(&value.as_ref()) {
                    return Err(Error::InvalidArgument(format!(
                        "unknown event kind '{value}'"
                    )));
                }
                kinds.push(value.into_owned());
            }
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unknown query parameter '{other}'"
                )));
            }
        }
    }

    Ok(FeedQuery {
        since,
        before,
        limit: limit.unwrap_or(FEED_LIMIT_DEFAULT),
        kinds: if kinds.is_empty() { None } else { Some(kinds) },
        // The route is admin-only, and the audit screen reads the trail here.
        include_audit: true,
    })
}
