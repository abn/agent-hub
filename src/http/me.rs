//! The calling member's own REST surface.
//!
//! Routes here answer for the member behind the token and never for the human
//! admin: they read what is waiting for that member's agent, and nothing they
//! do changes what it is shown next.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::credential_error;
use crate::http::problem::Problem;
use crate::mcp::notifications::{self, Entry, Pending};
use crate::mcp::session_arg::{self, Mode, SessionArg, SessionRef};

/// How many items one peek returns when the caller does not say.
const LIMIT_DEFAULT: usize = 20;
/// The most items one peek returns.
const LIMIT_MAX: usize = 100;
/// The longest a peek holds the request open, in seconds.
const WAIT_MAX: u64 = 60;
/// How often a held peek reads again with no write to wake it, so a missed
/// wake costs seconds and not the whole wait.
const RECHECK: Duration = Duration::from_secs(10);

/// What a peek was asked for.
#[derive(Debug, Default)]
struct PendingQuery {
    /// A session id, or with `name` the same session by name.
    session: Option<String>,
    name: Option<String>,
    project_id: Option<String>,
    limit: usize,
    wait: u64,
    after: Option<String>,
}

/// `GET /api/v1/me/pending`
///
/// What the notification trailer would deliver to the caller now, in the
/// trailer's item shape, without delivering it: no cursor moves, so a later
/// `notify_wait` or tool result still carries every item. The read is keyed by
/// the caller's member, its agent (`X-Agent-Id`, else `cli`) and a session:
/// `session` is a session id, or `name` with `project_id` names it. Without
/// either only the items asked outside any session are read.
///
/// With `wait` (seconds, 60 at most) the request is held open until an item
/// newer than `after` is waiting, or the time is up. Without `after`, any
/// waiting item ends the wait. `count` is how many items are pending, `items`
/// the first `limit` of those newer than `after`, oldest first, and `cursor`
/// the id of the last of them, which is what `after` takes on the next peek, so
/// no item past `limit` is stepped over.
///
/// The token must be an active member's. A pending or missing token is refused
/// with its reason, and so is the admin token: the admin has no attention queue
/// of its own.
pub async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> std::result::Result<Json<Value>, Problem> {
    let token = crate::http::auth::bearer_token(&headers);
    let principal = crate::mcp::resolve_caller(&state, &headers)
        .await
        .map_err(|err| Problem::from_error(&credential_error(err, token.as_deref())))?;
    principal
        .require_active()
        .map_err(|err| Problem::from_error(&err))?;
    if principal.member().is_none() || principal.is_admin() {
        return Err(Problem::from_error(&Error::Forbidden(
            "pending items are read with a member token; the admin token has none".to_string(),
        )));
    }
    let query = parse_query(raw.as_deref()).map_err(|err| Problem::from_error(&err))?;
    let session = match (&query.session, &query.name, &query.project_id) {
        (None, None, None) => None,
        (session_id, name, project_id) => {
            let arg = SessionArg::Ref(SessionRef {
                session_id: session_id.clone(),
                name: name.clone(),
                project_id: project_id.clone(),
            });
            let resolved = session_arg::resolve(
                &state,
                &principal,
                Some(&arg),
                query.project_id.as_deref(),
                Mode::Read,
            )
            .await
            .map_err(|err| Problem::from_error(&err))?;
            // A peek in a session that has ended says so, so a hook sharing it
            // with another instance knows to start it again.
            if let Some(session) = resolved
                .session
                .as_ref()
                .filter(|session| session.status != "active")
            {
                return Err(Problem::from_error(&Error::Session(
                    crate::error::SessionReason::Ended,
                    format!(
                        "session '{}' has ended; call session_start to reopen it",
                        session.session_name
                    ),
                    None,
                )));
            }
            resolved.session.map(|session| session.id)
        }
    };
    // Subscribe before the first read so a write cannot land between the read
    // and listening.
    let mut ticker = state.ticker.subscribe();
    let deadline = Instant::now() + Duration::from_secs(query.wait);
    let pending = loop {
        let pending = notifications::read(
            &state,
            &principal,
            session.as_deref(),
            query.after.as_deref(),
        )
        .await;
        // A peek delivers nothing, but a subscription's cursor may pass rows it
        // could never report, so a long filtered stretch is scanned once.
        pending.settle_skips(&state).await;
        let now = Instant::now();
        if query.wait == 0 || now >= deadline || has_fresh(&pending, query.after.as_deref()) {
            break pending;
        }
        let remaining = (deadline - now).min(RECHECK);
        if let Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) =
            tokio::time::timeout(remaining, ticker.recv()).await
        {
            tokio::time::sleep(remaining).await;
        }
    };

    let (count, returned) = cut(pending.entries, query.after.as_deref(), query.limit);
    let cursor = returned
        .last()
        .map(|entry| entry.key.clone())
        .or(query.after)
        .unwrap_or_default();
    let items: Vec<Value> = returned.into_iter().map(|entry| entry.item).collect();
    Ok(Json(
        json!({ "count": count, "items": items, "cursor": cursor }),
    ))
}

/// The page a peek returns: how many items are pending, and the first `limit`
/// of them, oldest first.
///
/// An item two subscriptions both report is one item. A run of items that share
/// a key is never split by `limit`: the cursor is a key, so a cut inside the run
/// would leave the rest of it behind a cursor that has already passed it.
fn cut(mut entries: Vec<Entry>, after: Option<&str>, limit: usize) -> (usize, Vec<Entry>) {
    entries.retain(|entry| after.is_none_or(|after| entry.key.as_str() > after));
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    let mut seen = HashSet::new();
    entries.retain(|entry| {
        seen.insert((
            entry.item["source"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            entry.item["id"].as_str().unwrap_or_default().to_string(),
        ))
    });
    let count = entries.len();
    let mut end = limit.min(count);
    while end > 0 && end < count && entries[end].key == entries[end - 1].key {
        end += 1;
    }
    entries.truncate(end);
    (count, entries)
}

/// Whether an item newer than `after` is waiting, or any item when none is
/// given.
fn has_fresh(pending: &Pending, after: Option<&str>) -> bool {
    match after {
        Some(after) => pending
            .entries
            .iter()
            .any(|entry| entry.key.as_str() > after),
        None => !pending.entries.is_empty(),
    }
}

fn parse_query(raw: Option<&str>) -> std::result::Result<PendingQuery, Error> {
    let mut query = PendingQuery {
        limit: LIMIT_DEFAULT,
        ..PendingQuery::default()
    };
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "session" => query.session = Some(value.into_owned()).filter(|id| !id.is_empty()),
            "name" => query.name = Some(value.into_owned()).filter(|name| !name.is_empty()),
            "project_id" => {
                query.project_id = Some(value.into_owned()).filter(|id| !id.is_empty());
            }
            "after" => query.after = Some(value.into_owned()).filter(|id| !id.is_empty()),
            "limit" => {
                let limit = value.parse::<usize>().map_err(|_| {
                    Error::InvalidArgument(format!("limit must be an integer, got '{value}'"))
                })?;
                query.limit = limit.clamp(1, LIMIT_MAX);
            }
            "wait" => {
                let wait = value.parse::<u64>().map_err(|_| {
                    Error::InvalidArgument(format!(
                        "wait must be a whole number of seconds, got '{value}'"
                    ))
                })?;
                query.wait = wait.min(WAIT_MAX);
            }
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unknown query parameter '{other}'"
                )));
            }
        }
    }
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str, id: &str, key: &str) -> Entry {
        Entry {
            item: json!({ "source": source, "id": id }),
            key: key.to_string(),
        }
    }

    fn ids(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.item["id"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn a_limit_inside_a_run_of_one_key_takes_the_whole_run() {
        let entries = vec![
            entry("attention", "q1", "b"),
            entry("subscription", "e1", "a"),
            entry("subscription", "e2", "b"),
            entry("subscription", "e3", "c"),
        ];
        let (count, page) = cut(entries, None, 2);
        assert_eq!(count, 4);
        assert_eq!(ids(&page), ["e1", "q1", "e2"], "the key b run stays whole");
    }

    #[test]
    fn an_item_reported_twice_is_one_item() {
        let entries = vec![
            entry("subscription", "e1", "a"),
            entry("subscription", "e1", "a"),
            entry("subscription", "e2", "b"),
        ];
        let (count, page) = cut(entries, None, 10);
        assert_eq!(count, 2);
        assert_eq!(ids(&page), ["e1", "e2"]);
    }

    #[test]
    fn only_items_after_the_cursor_are_counted_and_returned() {
        let entries = vec![
            entry("subscription", "e1", "a"),
            entry("subscription", "e2", "b"),
            entry("subscription", "e3", "c"),
        ];
        let (count, page) = cut(entries, Some("a"), 1);
        assert_eq!(count, 2);
        assert_eq!(ids(&page), ["e2"]);
    }
}
