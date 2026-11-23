//! Writes to a shared session brain, in the order that keeps the file, the
//! search index and session status consistent.
//!
//! The caller has resolved and joined the session (step 1) and holds its gate
//! shared (step 2). [`write`] does the rest: it takes the brain file lock,
//! re-reads the session status and marks `index_pending` in one hub
//! transaction (step 3), checks the base and writes the file (step 4), then
//! indexes, appends the event and clears the marker in one hub transaction
//! (step 5).

use crate::app::AppState;
use crate::author::Writer;
use crate::error::Result;
use crate::store::events::NewEvent;

/// The change a session brain write makes.
#[derive(Debug, Clone, Copy)]
pub enum BrainOp<'a> {
    Put {
        path: &'a str,
        bytes: &'a [u8],
    },
    Delete {
        path: &'a str,
    },
    /// Put back the bytes of an earlier version. A revert is an overwrite.
    Revert {
        path: &'a str,
        version: &'a str,
    },
}

/// One write to a session brain.
#[derive(Debug, Clone)]
pub struct SessionWrite<'a> {
    pub session_id: &'a str,
    pub writer: &'a Writer,
    /// The write-log op, such as `brain.put`.
    pub op_name: &'a str,
    pub op: BrainOp<'a>,
    /// The base the caller wrote against. A mismatch, or (without it) an
    /// existing key last written by a different participant, answers
    /// `stale_base` with `details.version`.
    pub if_version: Option<&'a str>,
    /// The event the write records, appended in the index transaction.
    pub event: Option<NewEvent>,
}

/// What a write stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrote {
    pub path: String,
    /// The version stored; none for a delete.
    pub version: Option<String>,
    pub event_id: Option<String>,
}

/// Steps 3 to 5 of a session brain write. Ended answers `session_ended`,
/// pruned answers `session_pruned`.
pub async fn write(
    state: &AppState,
    _gate: &tokio::sync::OwnedRwLockReadGuard<()>,
    write: SessionWrite<'_>,
) -> Result<Wrote> {
    let _ = (state, write);
    Err(crate::store::unbuilt("brain::shared::write"))
}

/// Rebuild the brain search docs of every session marked `index_pending` and
/// clear the marks. Startup runs it after `reconcile_sessions`.
pub async fn reindex_pending(state: &AppState) -> Result<usize> {
    let _ = state;
    Err(crate::store::unbuilt("brain::shared::reindex_pending"))
}
