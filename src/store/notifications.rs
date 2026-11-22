//! Attention cursors: what each caller has been shown.
//!
//! The notification trailer on every tool result delivers each resolved item
//! once. The cursor records the newest id delivered, keyed by the resolved
//! actor, the same stable label the feed cursors use, never the token, and by
//! the session the items were asked in. The empty session stands for items
//! asked outside any session, so two sessions of one actor each keep their own
//! place and neither takes the other's answers.

use turso::{Connection, Database, Value};

use crate::error::Result;

/// The newest notification event one agent has been shown for one session
/// scope, if any. The empty session is the scope of items asked outside any
/// session.
pub async fn cursor(db: &Database, agent: &str, session_id: &str) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    cursor_on(&conn, agent, session_id).await
}

async fn cursor_on(conn: &Connection, agent: &str, session_id: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT last_seen_event_id FROM agent_notify_cursors
             WHERE agent = ?1 AND session_id = ?2",
            vec![
                Value::Text(agent.to_string()),
                Value::Text(session_id.to_string()),
            ],
        )
        .await
        .map_err(super::engine)?;
    match rows.next().await.map_err(super::engine)? {
        Some(row) => match row.get_value(0).map_err(super::engine)? {
            Value::Text(id) => Ok(Some(id)),
            _ => Ok(None),
        },
        None => Ok(None),
    }
}

/// Advance one agent's cursor for one session scope to an event it has been
/// shown.
///
/// The cursor never moves backwards, and an empty id moves nothing, so an
/// empty read leaves the cursor alone. The read and the write share an
/// immediate transaction, so a cursor cannot step over an event committed
/// between them.
///
/// A session scope keeps a cursor only while the session is active: delivery
/// that finishes after the session ended or was pruned writes nothing, so the
/// cleanup of that session is not undone by a late settle.
pub async fn advance(db: &Database, agent: &str, session_id: &str, event_id: &str) -> Result<()> {
    if event_id.is_empty() {
        return Ok(());
    }
    let tx = super::begin_write(db).await?;
    if !session_id.is_empty() && !session_is_active(&tx, session_id).await? {
        return Ok(());
    }
    let current = cursor_on(&tx, agent, session_id).await?;
    if current.as_deref().is_some_and(|seen| event_id <= seen) {
        return Ok(());
    }
    tx.execute(
        "INSERT OR REPLACE INTO agent_notify_cursors(agent, session_id, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)",
        vec![
            Value::Text(agent.to_string()),
            Value::Text(session_id.to_string()),
            Value::Text(event_id.to_string()),
            Value::Text(super::now_rfc3339()),
        ],
    )
    .await
    .map_err(super::engine)?;
    tx.commit().await.map_err(super::engine)?;
    Ok(())
}

pub(crate) async fn session_is_active(conn: &Connection, session_id: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM sessions
             WHERE id = ?1 AND status = 'active' AND deleted_at IS NULL",
            vec![Value::Text(session_id.to_string())],
        )
        .await
        .map_err(super::engine)?;
    Ok(rows.next().await.map_err(super::engine)?.is_some())
}

/// Forget every attention cursor kept for one session, so a pruned session
/// takes its place with it.
pub(crate) async fn forget_session_in_tx(tx: &super::WriteTx, session_id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM agent_notify_cursors WHERE session_id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(super::engine)?;
    Ok(())
}
