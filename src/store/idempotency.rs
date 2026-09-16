//! Idempotency keys, so a retried write does not duplicate an event.

use turso::{Connection, Value};

use crate::error::{Error, Result};

/// Return the event id already recorded for a key, if any.
pub async fn lookup(conn: &Connection, project_id: &str, key: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT event_id FROM idempotency WHERE project_id = ?1 AND idempotency_key = ?2",
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(key.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    if let Some(row) = rows.next().await.map_err(engine)?
        && let Value::Text(id) = row.get_value(0).map_err(engine)?
    {
        return Ok(Some(id));
    }
    Ok(None)
}

/// Record a key against the event it produced.
pub async fn record(
    conn: &Connection,
    project_id: &str,
    key: &str,
    event_id: &str,
    created_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO idempotency(project_id, idempotency_key, event_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(key.to_string()),
            Value::Text(event_id.to_string()),
            Value::Text(created_at.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
