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

/// A feed event as Home shows it: the event, and the name of its project.
#[derive(Debug, Clone, Serialize)]
pub struct HomeEvent {
    #[serde(flatten)]
    pub event: Event,
    /// The name the projects list shows, absent when no project row carries
    /// the id.
    pub project_display_name: Option<String>,
}

/// What one project holds above its cursor, with the project's name.
#[derive(Debug, Clone, Serialize)]
pub struct HomeUnseen {
    #[serde(flatten)]
    pub unseen: events::ProjectUnseen,
    pub project_display_name: Option<String>,
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
    pub recent: Vec<HomeEvent>,
    /// Per project, how many feed events sit above the human's last-seen
    /// cursor, so the newest rows can be drawn as seen or not. A project with
    /// nothing unseen is absent rather than zero.
    pub unseen: Vec<HomeUnseen>,
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
    let unseen = events::unseen_counts(db).await?;
    let agents_active = crate::store::sessions::agents_active(db, active_since, None).await?;

    // One read names every project the response mentions.
    let conn = super::connect(db)?;
    let ids: Vec<&str> = recent
        .iter()
        .map(|event| event.project_id.as_str())
        .chain(unseen.iter().map(|row| row.project_id.as_str()))
        .collect();
    let names = crate::store::projects::display_names(&conn, &ids).await?;
    let recent: Vec<HomeEvent> = recent
        .into_iter()
        .map(|event| HomeEvent {
            project_display_name: names.get(&event.project_id).cloned(),
            event,
        })
        .collect();
    let unseen = unseen
        .into_iter()
        .map(|unseen| HomeUnseen {
            project_display_name: names.get(&unseen.project_id).cloned(),
            unseen,
        })
        .collect();
    Ok(Home {
        unread: counts.unread,
        waiting: counts.waiting,
        agents_active,
        last_event_at: recent.first().map(|row| row.event.created_at.clone()),
        recent,
        unseen,
        storage: HomeStorage {
            used_bytes: usage.used_bytes,
            capacity_bytes: usage.capacity_bytes,
            free_bytes: usage.free_bytes,
        },
        prunable: usage.prunable,
    })
}
