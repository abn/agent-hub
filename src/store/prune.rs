//! Pruning: the only deletion path, reversible for a short window.
//!
//! A prune soft-deletes a session. Within the undo window it can be restored;
//! once the window passes, a sweep commits it, removing the brain file, the
//! metadata, the search rows, and the session's feed events.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Value};

use crate::error::{Error, Result};

/// How long a prune can be undone.
pub const UNDO_WINDOW_SECS: i64 = 30;

/// The token returned by a prune, used to undo it.
#[derive(Debug, Clone, Serialize)]
pub struct PruneToken {
    pub undo_token: String,
    pub undo_expires_at: String,
}

/// Soft-delete a session and return the token that can undo it.
///
/// The session must be ended first: pruning removes the brain file, and the
/// wrapper must not be writing to an open session.
pub async fn prune_session(db: &Database, session_id: &str) -> Result<PruneToken> {
    let session = crate::store::sessions::get(db, session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;

    if session.deleted_at.is_some() {
        return Err(Error::Conflict(format!(
            "session {session_id} is already pruned"
        )));
    }
    if session.status != "ended" {
        return Err(Error::Conflict(
            "end the session before pruning it".to_string(),
        ));
    }

    let now = time::OffsetDateTime::now_utc();
    let conn = db.connect().map_err(engine)?;
    conn.execute(
        "UPDATE sessions SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
        vec![
            Value::Text(format_time(now)),
            Value::Text(session.id.clone()),
        ],
    )
    .await
    .map_err(engine)?;

    Ok(PruneToken {
        undo_token: session.id,
        undo_expires_at: format_time(now + time::Duration::seconds(UNDO_WINDOW_SECS)),
    })
}

/// Restore a session pruned within the window.
pub async fn undo(db: &Database, token: &str) -> Result<()> {
    let session = crate::store::sessions::get(db, token)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {token} not found")))?;
    let deleted_at = session
        .deleted_at
        .ok_or_else(|| Error::NotFound(format!("session {token} is not pruned")))?;

    let now = time::OffsetDateTime::now_utc();
    let pruned_at =
        time::OffsetDateTime::parse(&deleted_at, &time::format_description::well_known::Rfc3339)
            .unwrap_or(now);
    if (now - pruned_at).whole_seconds() >= UNDO_WINDOW_SECS {
        return Err(Error::Conflict(
            "the undo window for this prune has passed".to_string(),
        ));
    }

    let conn = db.connect().map_err(engine)?;
    conn.execute(
        "UPDATE sessions SET deleted_at = NULL WHERE id = ?1",
        vec![Value::Text(token.to_string())],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// Commit every prune whose window has passed. Returns how many were committed.
pub async fn sweep(db: &Database, data_dir: &Path) -> Result<u64> {
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT id, project_id, deleted_at FROM sessions WHERE deleted_at IS NOT NULL",
            (),
        )
        .await
        .map_err(engine)?;

    let mut pending = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let id = match row.get_value(0).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        let project_id = match row.get_value(1).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        let deleted_at = match row.get_value(2).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        pending.push((id, project_id, deleted_at));
    }

    let now = time::OffsetDateTime::now_utc();
    let mut committed = 0;
    for (id, project_id, deleted_at) in pending {
        let pruned_at = time::OffsetDateTime::parse(
            &deleted_at,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap_or(now);
        if (now - pruned_at).whole_seconds() < UNDO_WINDOW_SECS {
            continue;
        }
        commit(db, data_dir, &id, &project_id).await?;
        committed += 1;
    }
    Ok(committed)
}

async fn commit(db: &Database, data_dir: &Path, session_id: &str, project_id: &str) -> Result<()> {
    // Removing the file is the point of the prune, through the wrapper so it
    // takes the session lock; a failure must abort the commit.
    let removed = crate::brain::BrainStore::for_data_dir(data_dir)
        .remove(project_id, session_id)
        .await?;
    if !removed {
        tracing::warn!(session_id, "prune found no brain file to remove");
    }

    let mut conn = db.connect().map_err(engine)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    // The session's lifecycle events are indexed; drop their rows first.
    tx.execute(
        "DELETE FROM search_docs WHERE doc_id IN
         (SELECT 'event:' || id FROM events WHERE kind = 'session' AND payload LIKE ?1)",
        vec![Value::Text(format!("%\"session_id\":\"{session_id}\"%"))],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM events WHERE kind = 'session' AND payload LIKE ?1",
        vec![Value::Text(format!("%\"session_id\":\"{session_id}\"%"))],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM search_docs WHERE session_id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM sessions WHERE id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)
}

fn format_time(ts: time::OffsetDateTime) -> String {
    ts.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
