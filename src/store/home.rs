//! The home summary: what waits on the human and what just happened.

use serde::Serialize;
use turso::Database;

use crate::error::Result;
use crate::store::events::{self, Event};
use crate::store::inbox;

/// The storage numbers Home shows without opening the storage screen.
#[derive(Debug, Clone, Serialize)]
pub struct HomeStorage {
    pub used_bytes: i64,
    pub capacity_bytes: Option<i64>,
    pub free_bytes: Option<i64>,
}

/// The overview shown on Home.
///
/// One response: the screen's summary line, its two cards and its storage
/// hint all come from here, so Home makes one request.
#[derive(Debug, Clone, Serialize)]
pub struct Home {
    /// Finished work not yet read.
    pub unread: i64,
    /// Items waiting on a decision.
    pub waiting: i64,
    /// Agents with a session touched inside the active window.
    pub agents_active: i64,
    /// When the newest event landed, absent when nothing has happened yet.
    pub last_event_at: Option<String>,
    /// The newest events across every project.
    pub recent: Vec<Event>,
    pub storage: HomeStorage,
    /// Ended sessions and the bytes pruning them would free.
    pub prunable: crate::store::storage::Prunable,
}

/// Build the home summary.
///
/// Every number here is a count over an index or a memoised file total, so the
/// screen's one request stays one round of cheap queries.
pub async fn home(
    db: &Database,
    recent_limit: i64,
    active_since: &str,
    usage: crate::store::storage::StorageUsage,
) -> Result<Home> {
    let counts = inbox::counts(db).await?;
    let recent = events::recent(db, recent_limit).await?;
    let agents_active = crate::store::sessions::agents_active(db, active_since, None).await?;
    Ok(Home {
        unread: counts.unread,
        waiting: counts.waiting,
        agents_active,
        last_event_at: recent.first().map(|event| event.created_at.clone()),
        recent,
        storage: HomeStorage {
            used_bytes: usage.used_bytes,
            capacity_bytes: usage.capacity_bytes,
            free_bytes: usage.free_bytes,
        },
        prunable: usage.prunable,
    })
}
