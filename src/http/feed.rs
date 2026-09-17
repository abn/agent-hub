//! The project feed REST route.

use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use serde::Serialize;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
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
}

/// `GET /api/v1/projects/{id}/feed`
///
/// Query parameters are `since`, `before`, `limit`, and repeatable `kinds`.
/// A valid bearer token is required; the resolved principal is not recorded
/// on a read.
pub async fn read(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
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

    Ok(Json(FeedPage {
        events: page.events,
        next_since: page.next_since,
        next_before: page.next_before,
    }))
}

// Query parsing is manual because the axum query extractor cannot map a
// repeated key into a `Vec`.
fn parse_query(raw: Option<&str>) -> std::result::Result<FeedQuery, Error> {
    let mut since = None;
    let mut before = None;
    let mut limit = None;
    let mut kinds: Vec<String> = Vec::new();

    for pair in raw.unwrap_or_default().split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key);
        let value = decode(value);
        match key.as_str() {
            "since" => since = Some(value),
            "before" => before = Some(value),
            "limit" => {
                limit = Some(value.parse::<i64>().map_err(|_| {
                    Error::InvalidArgument(format!("limit must be an integer, got '{value}'"))
                })?);
            }
            "kinds" => {
                if !events::KINDS.contains(&value.as_str()) {
                    return Err(Error::InvalidArgument(format!(
                        "unknown event kind '{value}'"
                    )));
                }
                kinds.push(value);
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
    })
}

fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match hex(bytes[index + 1]).zip(hex(bytes[index + 2])) {
                    Some((high, low)) => {
                        out.push((high << 4) | low);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
