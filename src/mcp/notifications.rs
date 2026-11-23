//! The notification trailer attached to every MCP tool result.
//!
//! Agents have no push channel. Because individual harnesses would otherwise
//! each invent their own timer and poll, the hub delivers what needs attention
//! on the next call the agent makes: a top-level `notifications` member on a
//! successful tool result, present only when there is something to report.
//!
//! Two sources feed it: the participant's attention queue, the answers and
//! decisions on questions and approvals asked in its session or by it, and the
//! comments and page edits that name its session or itself; and the standing
//! subscriptions it registered with `notify_subscribe`.
//!
//! What a call is shown depends on the session it names. A session sees the
//! items asked in it and the items asked outside any session; a call with no
//! session sees only the second kind. Reading and advancing are separate steps
//! so a peek can report what a drain would deliver and move nothing, and every
//! advance is a compare-and-set: of two calls that read the same page, one
//! delivers it and the other delivers nothing.

use serde_json::{Value, json};

use crate::app::AppState;
use crate::principal::Principal;
use crate::store::notifications::Settle;

use super::{attention, subscriptions};

/// One pending item and the id it is ordered by.
///
/// The item is the trailer's line. The key is what "newer" means for it: an
/// event id, and for an attention item the id of the answer or decision that
/// resolved it, since that is minted after the question and is what the
/// attention cursor counts.
#[derive(Clone)]
pub(crate) struct Entry {
    pub item: Value,
    pub key: String,
}

/// A subscription cursor and where one read would move it.
#[derive(Debug, Clone)]
pub(crate) struct Advance {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// What is waiting for one caller in one session scope, and the cursor moves
/// that delivering it would make.
#[derive(Default)]
pub(crate) struct Pending {
    /// Everything read, for a peek.
    pub entries: Vec<Entry>,
    attention: Vec<(Settle, Vec<Entry>)>,
    subscriptions: Vec<(Advance, Vec<Entry>)>,
    /// Subscription cursors that may move on a read that delivers nothing:
    /// they stop short of anything the subscription could report.
    skips: Vec<Advance>,
}

/// Read what is waiting without advancing anything: a peek.
///
/// `floor` is a place a subscription scan starts from when it is past the
/// stored cursor, so a peek that already holds a page asks for what follows it
/// and not for the page again.
///
/// A failure reads as nothing pending: the trailer must never fail the tool
/// call it rides on.
pub(crate) async fn read(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
    floor: Option<&str>,
) -> Pending {
    select(state, principal, session, floor, false).await
}

async fn select(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
    floor: Option<&str>,
    consume: bool,
) -> Pending {
    let mut pending = Pending::default();
    // A peek pages from its floor; a delivery reads from the cursor.
    let after = if consume { None } else { floor };
    match attention::read(state, principal, session, after, consume).await {
        Ok(read) => {
            for (settle, entries) in read.pages {
                pending.entries.extend(entries.iter().cloned());
                pending.attention.push((settle, entries));
            }
        }
        Err(err) => tracing::warn!(
            member = ?principal.member(),
            error = %err,
            "could not read the attention queue"
        ),
    }
    if let Some(read) = subscriptions::read(state, principal, session, floor).await {
        pending.entries.extend(read.entries.iter().cloned());
        for (advance, entries) in read.pages {
            pending.subscriptions.push((advance, entries));
        }
        pending.skips = read.skips;
    }
    pending
}

impl Pending {
    /// Deliver what was read: move each cursor by compare-and-set and keep the
    /// entries of the pages whose move won. A page another call moved first is
    /// that call's, so it is dropped here.
    ///
    /// Cursors move past what was scanned as well as what was reported, so
    /// events the caller cannot read, and events its filters drop, are not
    /// retried on every call. A call cancelled after this and before its reply
    /// is sent loses what it read.
    async fn settle(self, state: &AppState) -> Delivery {
        let mut delivered = Vec::new();
        let mut lost = false;
        for (settle, entries) in self.attention {
            let moved = settle.from == settle.to && settle.backlog.is_empty()
                || match crate::store::notifications::settle(&state.db, &settle).await {
                    Ok(moved) => moved,
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "could not advance the attention cursor"
                        );
                        false
                    }
                };
            if moved {
                delivered.extend(entries);
            } else if !entries.is_empty() {
                lost = true;
            }
        }
        for (advance, entries) in self.subscriptions {
            let moved = match crate::store::subscriptions::advance_from(
                &state.db,
                &advance.id,
                &advance.from,
                &advance.to,
            )
            .await
            {
                Ok(moved) => moved,
                Err(err) => {
                    tracing::warn!(
                        subscription = %advance.id,
                        error = %err,
                        "could not advance a subscription cursor"
                    );
                    false
                }
            };
            if moved {
                delivered.extend(entries);
            } else if !entries.is_empty() {
                lost = true;
            }
        }
        // An event the attention queue and a subscription both report is one
        // item: the first report of it, the attention queue's, is kept.
        let mut seen = std::collections::HashSet::new();
        delivered.retain(|entry| seen.insert(entry.key.clone()));
        Delivery { delivered, lost }
    }

    /// Move the subscription cursors that stop short of anything reportable.
    ///
    /// A row a subscription can never deliver is skipped for good by a peek as
    /// much as by a drain, so repeated peeks move on through a long filtered
    /// stretch instead of rescanning it. Nothing deliverable is passed.
    pub(crate) async fn settle_skips(&self, state: &AppState) {
        for advance in &self.skips {
            if let Err(err) = crate::store::subscriptions::advance_from(
                &state.db,
                &advance.id,
                &advance.from,
                &advance.to,
            )
            .await
            {
                tracing::warn!(
                    subscription = %advance.id,
                    error = %err,
                    "could not advance a subscription cursor"
                );
            }
        }
    }
}

/// What a drain delivered, and whether another call took a page first.
struct Delivery {
    delivered: Vec<Entry>,
    lost: bool,
}

/// How a drain came out.
pub(crate) enum Drained {
    /// The trailer to deliver.
    Delivered(Value),
    /// Nothing was waiting.
    Nothing,
    /// A page was read but another call delivered it first. A caller that
    /// waits selects again; a trailer shows nothing.
    Raced,
}

/// Read and deliver what is waiting for the caller in one session scope.
pub(crate) async fn drain(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
) -> Drained {
    let pending = select(state, principal, session, None, true).await;
    let skips = pending.skips.clone();
    let delivery = pending.settle(state).await;
    // A skip is a cursor move with nothing behind it, so losing it costs only a
    // rescan.
    for advance in &skips {
        let _ = crate::store::subscriptions::advance_from(
            &state.db,
            &advance.id,
            &advance.from,
            &advance.to,
        )
        .await;
    }
    if delivery.delivered.is_empty() {
        return if delivery.lost {
            Drained::Raced
        } else {
            Drained::Nothing
        };
    }
    let items: Vec<Value> = delivery
        .delivered
        .into_iter()
        .map(|entry| entry.item)
        .collect();
    Drained::Delivered(json!({ "pending": items }))
}

/// The trailer for one successful tool result, or `None` when there is
/// nothing to report. Delivering it advances the cursors it read from. A page
/// another call delivered first is not shown a second time.
pub(crate) async fn trailer(
    state: &AppState,
    principal: &Principal,
    session: Option<&str>,
) -> Option<Value> {
    match drain(state, principal, session).await {
        Drained::Delivered(trailer) => Some(trailer),
        Drained::Nothing | Drained::Raced => None,
    }
}
