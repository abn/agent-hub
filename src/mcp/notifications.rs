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
//!
//! What a call is shown depends on the session it is bound to. A session sees
//! the items asked in it and the items asked outside any session; a call with
//! no session sees only the second kind. Reading and advancing are separate
//! steps so a peek can report what a drain would deliver and move nothing.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use serde_json::{Value, json};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::app::AppState;
use crate::principal::Principal;

use super::{attention, subscriptions};

/// One pending item and the id it is ordered by.
///
/// The item is the trailer's line. The key is what "newer" means for it: an
/// event id, and for an attention item the id of the answer or decision that
/// resolved it, since that is minted after the question and is what the
/// attention cursor counts.
pub(crate) struct Entry {
    pub item: Value,
    pub key: String,
}

/// What is waiting for one caller in one session scope, and the cursors that
/// delivering it would advance.
#[derive(Default)]
pub(crate) struct Pending {
    pub entries: Vec<Entry>,
    attention: Vec<(String, String)>,
    subscriptions: Vec<(String, String)>,
    /// Subscription cursors that may move on a read that delivers nothing:
    /// they stop short of anything the subscription could report.
    skips: Vec<(String, String)>,
}

/// Read what is waiting without advancing anything.
///
/// `floor` is a place the read starts from when it is past the stored cursor,
/// so a peek that already holds a page asks for what follows it and not for the
/// page again. A drain passes none.
///
/// A failure reads as nothing pending: the trailer must never fail the tool
/// call it rides on.
pub(crate) async fn read(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
    floor: Option<&str>,
) -> Pending {
    let mut pending = Pending::default();
    match attention::read(state, principal, session, floor).await {
        Ok(read) => {
            pending.entries.extend(read.entries);
            pending.attention = read.cursors;
        }
        Err(err) => tracing::warn!(
            actor = %principal.actor,
            error = %err,
            "could not read the attention queue"
        ),
    }
    if let Some(read) = subscriptions::read(state, principal, session, floor).await {
        pending.entries.extend(read.entries);
        pending.subscriptions = read.cursors;
        pending.skips = read.skips;
    }
    pending
}

/// Calls that read and settle the same cursors, serialised.
///
/// The key is the actor and a session, the empty session standing for items
/// asked outside any session. Without it two calls of one actor read the same
/// cursor before either moved it and each deliver the same items.
static LOCKS: LazyLock<std::sync::Mutex<Locks>> = LazyLock::new(Default::default);

type Locks = HashMap<(String, String), Arc<Mutex<()>>>;

/// Take the locks for one call's scope: its own session's and, last, the
/// session-less one every scope also reads. A fixed order cannot deadlock.
async fn serialise(actor: &str, session: Option<&str>) -> Vec<OwnedMutexGuard<()>> {
    let keys = session
        .into_iter()
        .chain(std::iter::once(""))
        .map(|session| (actor.to_string(), session.to_string()))
        .collect::<Vec<_>>();
    let locks: Vec<Arc<Mutex<()>>> = {
        let mut held = LOCKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A lock nobody holds or waits on is dropped here, so the table keeps
        // only what is in use.
        held.retain(|_, lock| Arc::strong_count(lock) > 1);
        keys.into_iter()
            .map(|key| Arc::clone(held.entry(key).or_default()))
            .collect()
    };
    let mut guards = Vec::with_capacity(locks.len());
    for lock in locks {
        guards.push(lock.lock_owned().await);
    }
    guards
}

impl Pending {
    /// Mark what was read as delivered.
    ///
    /// Cursors move past what was scanned as well as what was reported, so
    /// events the caller cannot read, and events its filters drop, are not
    /// retried on every call.
    /// A call cancelled after this and before its reply is sent loses what it read.
    async fn settle(&self, state: &AppState, principal: &Principal) {
        for (scope, newest) in &self.attention {
            if let Err(err) =
                crate::store::notifications::advance(&state.db, &principal.actor, scope, newest)
                    .await
            {
                tracing::warn!(
                    actor = %principal.actor,
                    error = %err,
                    "could not advance the attention cursor"
                );
            }
        }
        for (id, newest) in &self.subscriptions {
            if let Err(err) = crate::store::subscriptions::set_cursor(&state.db, id, newest).await {
                tracing::warn!(
                    subscription = %id,
                    error = %err,
                    "could not advance a subscription cursor"
                );
            }
        }
    }
}

impl Pending {
    /// Move the subscription cursors that stop short of anything reportable.
    ///
    /// A row a subscription can never deliver is skipped for good by a peek as
    /// much as by a drain, so repeated peeks move on through a long filtered
    /// stretch instead of rescanning it. Nothing deliverable is passed.
    pub(crate) async fn settle_skips(&self, state: &AppState) {
        for (id, newest) in &self.skips {
            if let Err(err) = crate::store::subscriptions::set_cursor(&state.db, id, newest).await {
                tracing::warn!(
                    subscription = %id,
                    error = %err,
                    "could not advance a subscription cursor"
                );
            }
        }
    }
}

/// The trailer for one successful tool result, or `None` when there is
/// nothing to report. Delivering it advances the cursors it read from.
pub(crate) async fn trailer(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
) -> Option<Value> {
    let _serialised = serialise(&principal.actor, session).await;
    let pending = read(state, principal, session, None).await;
    pending.settle(state, principal).await;
    if pending.entries.is_empty() {
        return None;
    }
    let items: Vec<Value> = pending
        .entries
        .into_iter()
        .map(|entry| entry.item)
        .collect();
    Some(json!({ "pending": items }))
}
