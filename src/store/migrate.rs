//! The members upgrade: preflight plan, placement of old owner labels, and
//! the journalled brain merge.
//!
//! `check` is read-only and backs `agent-hub migrate --check`. `run_merges`
//! is the startup merge worker; `AppState::open` runs it to completion before
//! the hub serves.

use std::path::Path;

use crate::author::Origin;
use crate::error::Result;

/// Where an old owner label lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Participant {
        member: String,
        agent: String,
    },
    Local {
        agent: String,
    },
    Human {
        origin: Origin,
    },
    /// The operator map drops it (cursors and subscriptions only).
    Drop,
    Unplaced,
}

/// One label the store holds and where it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedLabel {
    pub label: String,
    pub placement: Placement,
    pub sessions: usize,
    pub cursors: usize,
    pub subscriptions: usize,
}

/// Live sessions of one name in one project, merged into the survivor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeGroup {
    pub project_id: String,
    pub session_name: String,
    pub survivor: String,
    pub losers: Vec<String>,
}

/// The preflight plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The version the store is at.
    pub from_version: i64,
    pub labels: Vec<PlacedLabel>,
    pub groups: Vec<MergeGroup>,
    /// Each stop's message, worded as the start prints it. Empty when clean.
    pub stops: Vec<String>,
}

impl Plan {
    /// Whether the upgrade may proceed.
    pub fn is_clean(&self) -> bool {
        self.stops.is_empty()
    }
}

/// Plan the upgrade of the store in `data_dir` without writing to it.
///
/// A store below v27 is copied with `VACUUM INTO` under `staging_root`,
/// migrated there, and planned on the copy; the copy is removed before this
/// returns. A store at v28 or newer plans nothing. A store whose engine lock a
/// serving hub holds is refused with an error for which
/// [`Error::is_locked`](crate::error::Error::is_locked) is true.
pub async fn check(data_dir: &Path, staging_root: &Path) -> Result<Plan> {
    let _ = (data_dir, staging_root);
    Err(super::unbuilt("migrate::check"))
}

/// Where a test stops the merge worker: after a stage's effect, before its
/// commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    AfterMap,
    AfterCopy,
    AfterVerify,
    AfterClose,
    /// Renamed, directories not yet synced, stage not committed.
    AfterArchiveRename,
    AfterArchive,
}

/// Drive every merge journal row to `done`. `open` passes `None`.
#[doc(hidden)]
pub async fn run_merges(
    db: &turso::Database,
    data_dir: &Path,
    fail: Option<FailPoint>,
) -> Result<()> {
    let _ = (db, data_dir, fail);
    Err(super::unbuilt("migrate::run_merges"))
}
