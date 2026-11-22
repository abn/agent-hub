//! The caller's own attention queue, drained into the notification trailer.
//!
//! An agent learns here of an answer to a question it posted or a decision on
//! an approval it posted. A server-side cursor records the newest id shown,
//! so each item is delivered once and an empty read moves nothing.
//!
//! An item belongs to the session it was asked in, and the cursor is kept per
//! session: a call bound to a session is shown that session's items and the
//! items asked outside any session, each against its own cursor. A call bound
//! to no session is shown only the second kind.

use serde_json::{Value, json};

use super::notifications::Entry;
use crate::app::AppState;
use crate::principal::Principal;
use crate::store::inbox::{self, AskedIn, InboxItem};

/// How many resolved items one read takes from one scope.
///
/// A trailer rides along with an ordinary tool result, so a backlog is
/// delivered a page at a time, oldest first, and the rest waits above the
/// cursor for the next call.
const PAGE: i64 = 50;

/// What one read of the attention queue found, and where each cursor stands
/// once it is delivered.
pub(crate) struct Read {
    pub entries: Vec<Entry>,
    /// The session scope and the largest key among the items returned in it.
    /// The empty scope is the items asked outside any session.
    pub cursors: Vec<(String, String)>,
}

/// The caller's resolved items newer than its cursors, without moving them.
///
/// Each scope is read on its own: the query holds only that scope's items,
/// oldest first from its cursor, so what another scope holds can neither crowd
/// a page nor stand between the cursor and an item. A cursor is offered only as
/// far as the last item the page returned, never past one it did not.
///
/// A `floor` above a scope's cursor replaces it as the start of the range, so
/// the page is cut from where the caller stands and not from where the hub
/// last delivered.
pub(crate) async fn read(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
    floor: Option<&str>,
) -> crate::Result<Read> {
    let visible = crate::policy::visibility(&state.db, principal).await?;
    let mut read = Read {
        entries: Vec::new(),
        cursors: Vec::new(),
    };
    let scopes = session.into_iter().map(Some).chain(std::iter::once(None));
    for scope in scopes {
        let key = scope.unwrap_or_default();
        let stored = crate::store::notifications::cursor(&state.db, &principal.actor, key).await?;
        let cursor = match (stored, floor) {
            (Some(stored), Some(floor)) if floor > stored.as_str() => Some(floor.to_string()),
            (None, Some(floor)) => Some(floor.to_string()),
            (stored, _) => stored,
        };
        let asked = match scope {
            Some(session_id) => AskedIn::Session(session_id),
            None => AskedIn::Outside,
        };
        let page = inbox::resolved_in_scope(
            &state.db,
            &principal.actor,
            asked,
            cursor.as_deref(),
            PAGE,
            visible.as_filter(),
        )
        .await?;
        let Some(last) = page.items.last() else {
            continue;
        };
        read.cursors
            .push((key.to_string(), inbox::item_effective_id(last).to_string()));
        read.entries.extend(page.items.iter().filter_map(entry));
    }
    Ok(read)
}

/// The trailer line for one resolved item, when it is one an agent hears of.
fn entry(item: &InboxItem) -> Option<Entry> {
    // A resolution the hub made at a deadline arrives under the same kind
    // as a human's, so an agent handles both on one path, and says it
    // expired in its title and its own flag.
    let (kind, title, at, expired) = match item.kind.as_str() {
        "question" => {
            let answer = item.answer.as_ref()?;
            let title = if answer.expired {
                format!("expired, closed with no answer: {}", item.summary)
            } else {
                format!("answered: {}", item.summary)
            };
            (
                "question_answered",
                title,
                answer.answered_at.clone(),
                answer.expired,
            )
        }
        "approval" => {
            let decision = item.decision.as_ref()?;
            let verb = if decision.decision == "approved" {
                "approved"
            } else {
                "declined"
            };
            let title = if decision.expired {
                format!("expired, {verb}: {}", item.summary)
            } else {
                format!("{verb}: {}", item.summary)
            };
            (
                "approval_decided",
                title,
                decision.decided_at.clone(),
                decision.expired,
            )
        }
        _ => return None,
    };
    let mut line = json!({
        "source": "attention",
        "kind": kind,
        "id": item.event_id,
        "project_id": item.project_id,
        "title": title,
        "at": at,
        "direct": true,
    });
    if expired {
        line["expired"] = Value::Bool(true);
    }
    Some(Entry {
        item: line,
        key: inbox::item_effective_id(item).to_string(),
    })
}
