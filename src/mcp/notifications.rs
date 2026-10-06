//! The notification trailer attached to every MCP tool result.
//!
//! Agents have no push channel. Because individual harnesses would otherwise
//! each invent their own timer and poll, the hub delivers what needs attention
//! on the next call the agent makes: a top-level `notifications` member on a
//! successful tool result, present only when there is something to report.
//!
//! Two sources feed it: the caller's own attention queue, the answers and
//! decisions on its questions and approvals that it has not been shown, and
//! the standing subscriptions it registered with `notify_subscribe`.

use serde_json::{Value, json};

use crate::app::AppState;
use crate::principal::Principal;

use super::{attention, subscriptions};

/// The trailer for one successful tool result, or `None` when there is
/// nothing to report.
pub(crate) async fn trailer(state: &AppState, principal: &Principal) -> Option<Value> {
    let mut pending: Vec<Value> = Vec::new();
    if let Some(items) = attention::pending(state, principal).await {
        pending.extend(items);
    }
    if let Some(items) = subscriptions::drain(state, principal).await {
        pending.extend(items);
    }
    if pending.is_empty() {
        return None;
    }
    Some(json!({ "pending": pending }))
}
