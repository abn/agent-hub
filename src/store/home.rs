//! The home summary: what waits on the human and what just happened.

use serde::Serialize;
use turso::Database;

use crate::error::Result;
use crate::store::events::{self, Event};
use crate::store::inbox;

/// The overview shown on Home.
#[derive(Debug, Clone, Serialize)]
pub struct Home {
    /// Finished work not yet read.
    pub unread: i64,
    /// Items waiting on a decision.
    pub waiting: i64,
    /// The newest events across every project.
    pub recent: Vec<Event>,
}

/// Build the home summary.
pub async fn home(db: &Database, recent_limit: i64) -> Result<Home> {
    let counts = inbox::counts(db).await?;
    let recent = events::recent(db, recent_limit).await?;
    Ok(Home {
        unread: counts.unread,
        waiting: counts.waiting,
        recent,
    })
}
