//! The caller's own attention queue, drained into the notification trailer.
//!
//! An agent learns here of an answer to a question it posted or a decision on
//! an approval it posted. A server-side cursor records the newest id shown,
//! so each item is delivered once and an empty read moves nothing.

use serde_json::{Value, json};

use crate::app::AppState;
use crate::principal::Principal;
use crate::store::{inbox, notifications};

/// Resolved items the caller authored and has not yet been shown, or `None`.
///
/// A failure reads as nothing pending: the trailer must never fail the tool
/// call it rides on.
pub(crate) async fn pending(state: &AppState, principal: &Principal) -> Option<Vec<Value>> {
    let items = read_pending(state, principal).await.map_err(|err| {
        tracing::warn!(
            actor = %principal.actor,
            error = %err,
            "could not read the attention queue"
        );
    });
    let (items, next_since) = items.ok()?;
    if items.is_empty() {
        return None;
    }
    if let Some(next_since) = next_since
        && let Err(err) = notifications::advance(&state.db, &principal.actor, &next_since).await
    {
        tracing::warn!(
            actor = %principal.actor,
            error = %err,
            "could not advance the attention cursor"
        );
    }
    Some(items)
}

/// The caller's resolved items newer than its cursor, with the cursor to
/// advance to once they are delivered.
async fn read_pending(
    state: &AppState,
    principal: &Principal,
) -> crate::Result<(Vec<Value>, Option<String>)> {
    let cursor = notifications::cursor(&state.db, &principal.actor).await?;
    let visible = crate::policy::visibility(&state.db, principal).await?;
    let page = inbox::page_for_agent(
        &state.db,
        Some("resolved"),
        None,
        Some(&principal.actor),
        cursor.as_deref(),
        crate::limits::FEED_LIMIT_MAX,
        visible.as_filter(),
    )
    .await?;
    if page.items.is_empty() {
        return Ok((Vec::new(), None));
    }
    let mut items = Vec::with_capacity(page.items.len());
    for item in &page.items {
        // A resolution the hub made at a deadline arrives under the same kind
        // as a human's, so an agent handles both on one path, and says it
        // expired in its title and its own flag.
        let (kind, title, at, expired) = match item.kind.as_str() {
            "question" => {
                let Some(answer) = item.answer.as_ref() else {
                    continue;
                };
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
                let Some(decision) = item.decision.as_ref() else {
                    continue;
                };
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
            _ => continue,
        };
        let mut entry = json!({
            "source": "attention",
            "kind": kind,
            "id": item.event_id,
            "project_id": item.project_id,
            "title": title,
            "at": at,
        });
        if expired {
            entry["expired"] = Value::Bool(true);
        }
        items.push(entry);
    }
    if items.is_empty() {
        return Ok((Vec::new(), None));
    }
    Ok((items, page.next_since))
}
