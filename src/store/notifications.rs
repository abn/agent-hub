//! Attention cursors: what each caller has been shown.
//!
//! The notification trailer on every tool result delivers each resolved item
//! once. The cursor records the newest id delivered, keyed by the resolved
//! actor, the same stable label the feed cursors use, never the token.

use turso::{Connection, Database, Value};

use crate::error::Result;

/// The newest notification event one agent has been shown, if any.
pub async fn cursor(db: &Database, agent: &str) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    cursor_on(&conn, agent).await
}

async fn cursor_on(conn: &Connection, agent: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT last_seen_event_id FROM agent_notify_cursors WHERE agent = ?1",
            vec![Value::Text(agent.to_string())],
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

/// Advance one agent's cursor to an event it has been shown.
///
/// The cursor never moves backwards, and an empty id moves nothing, so an
/// empty read leaves the cursor alone. The read and the write share an
/// immediate transaction, so a cursor cannot step over an event committed
/// between them.
pub async fn advance(db: &Database, agent: &str, event_id: &str) -> Result<()> {
    if event_id.is_empty() {
        return Ok(());
    }
    let tx = super::begin_write(db).await?;
    let current = cursor_on(&tx, agent).await?;
    if current.as_deref().is_some_and(|seen| event_id <= seen) {
        return Ok(());
    }
    tx.execute(
        "INSERT OR REPLACE INTO agent_notify_cursors(agent, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3)",
        vec![
            Value::Text(agent.to_string()),
            Value::Text(event_id.to_string()),
            Value::Text(super::now_rfc3339()),
        ],
    )
    .await
    .map_err(super::engine)?;
    tx.commit().await.map_err(super::engine)?;
    Ok(())
}
