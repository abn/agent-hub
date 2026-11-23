//! The caller's own attention queue, drained into the notification trailer.
//!
//! A participant learns here of an answer to a question it or another
//! participant of its session posted, a decision on an approval, and a comment
//! or page edit that names its session or itself. A server-side cursor per
//! participant and session scope records the newest id shown, so each item is
//! delivered once and an empty read moves nothing.
//!
//! An item belongs to the session it was asked in, and the cursor is kept per
//! session: a call bound to a session is shown that session's items and the
//! items asked outside any session, each against its own cursor. A call bound
//! to no session is shown only the second kind.

use serde_json::{Value, json};

use super::notifications::Entry;
use crate::app::AppState;
use crate::principal::Principal;
use crate::store::events::Event;
use crate::store::inbox::{self, InboxItem};
use crate::store::notifications::{self as cursors, Settle};

/// How many resolved items one read takes from one scope.
///
/// A trailer rides along with an ordinary tool result, so a backlog is
/// delivered a page at a time, oldest first, and the rest waits above the
/// cursor for the next call.
const PAGE: i64 = 50;

/// What one read of the attention queue found: the entries of each scope's
/// page with the compare-and-set that delivering the page makes.
pub(crate) struct Read {
    pub pages: Vec<(Settle, Vec<Entry>)>,
}

/// The caller's attention pages, one per scope, oldest first.
///
/// Each scope is read on its own, so what another scope holds can neither
/// crowd a page nor stand between the cursor and an item. With `consume` the
/// read is [`cursors::attention`], whose settle moves the cursor; without it
/// the read is a peek, which moves nothing.
pub(crate) async fn read(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
    after: Option<&str>,
    consume: bool,
) -> crate::Result<Read> {
    let mut read = Read { pages: Vec::new() };
    let Some(key) = principal.participant() else {
        return Ok(read);
    };
    let visible = crate::policy::visibility(&state.db, principal).await?;
    let scopes = session.into_iter().map(Some).chain(std::iter::once(None));
    for scope in scopes {
        let scope_id = scope.unwrap_or_default();
        let page = if consume {
            cursors::attention(&state.db, &key, scope_id, PAGE, visible.as_filter()).await?
        } else {
            cursors::peek(&state.db, &key, scope_id, after, PAGE, visible.as_filter()).await?
        };
        let mut entries: Vec<Entry> = page.items.iter().filter_map(entry).collect();
        entries.extend(page.events.iter().map(event_entry));
        read.pages.push((page.settle, entries));
    }
    Ok(read)
}

/// The trailer line for a comment or page event whose audience names the
/// reader's session or the reader.
fn event_entry(event: &Event) -> Entry {
    Entry {
        item: json!({
            "source": "attention",
            "kind": event.kind,
            "id": event.id,
            "project_id": event.project_id,
            "title": event.summary,
            "at": event.created_at,
            "direct": true,
        }),
        key: event.id.clone(),
    }
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
