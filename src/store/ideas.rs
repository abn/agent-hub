//! Ideas: acting on a captured idea.

use turso::Database;

use crate::error::{Error, Result};
use crate::store::events;
use crate::store::inbox;

/// Mark a captured idea as resolved.
pub async fn resolve(db: &Database, idea_id: &str) -> Result<()> {
    set_status(db, idea_id, "resolved").await
}

/// Clear a captured idea from the inbox.
pub async fn clear(db: &Database, idea_id: &str) -> Result<()> {
    set_status(db, idea_id, "cleared").await
}

async fn set_status(db: &Database, idea_id: &str, status: &str) -> Result<()> {
    let event = events::get(db, idea_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("idea {idea_id} not found")))?;
    if event.kind != "idea" {
        return Err(Error::InvalidArgument(format!(
            "event {idea_id} is a {} and is not an idea",
            event.kind
        )));
    }
    inbox::set_status(db, idea_id, status).await
}
