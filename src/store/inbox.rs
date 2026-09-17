//! The human's inbox: a thin projection over the events that need attention.
//!
//! Finished work lands as `unread`; anything flagged as needing action lands
//! as `action`. An answer resolves the question it replies to.

use serde::Serialize;
use turso::{Connection, Database, Row, Value};

use crate::error::{Error, Result};

/// Inbox statuses.
pub const STATUSES: &[&str] = &["unread", "read", "action", "waiting", "resolved"];

/// One inbox entry, joined with its event.
#[derive(Debug, Clone, Serialize)]
pub struct InboxItem {
    pub event_id: String,
    pub project_id: String,
    pub kind: String,
    pub actor: String,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Status counts for the home summary.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Counts {
    /// Finished work not yet read.
    pub unread: i64,
    /// Items waiting on a decision: action and waiting.
    pub waiting: i64,
}

/// Add an event to the inbox. Called by the event writer in its transaction.
pub async fn project(
    conn: &Connection,
    event_id: &str,
    needs_action: bool,
    created_at: &str,
) -> Result<()> {
    let status = if needs_action { "action" } else { "unread" };
    conn.execute(
        "INSERT OR IGNORE INTO inbox(event_id, status, assigned_to, updated_at)
         VALUES (?1, ?2, 'human', ?3)",
        vec![
            Value::Text(event_id.to_string()),
            Value::Text(status.to_string()),
            Value::Text(created_at.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// List inbox entries, newest first, optionally filtered.
pub async fn list(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    limit: i64,
) -> Result<Vec<InboxItem>> {
    list_visible(db, status, project_id, limit, None).await
}

/// List inbox entries with an optional project confinement.
///
/// `None` means every project (the admin surface). `Some(set)` keeps only
/// entries from those projects; an empty set yields nothing.
pub async fn list_visible(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<Vec<InboxItem>> {
    if let Some(status) = status {
        validate_status(status)?;
    }
    let limit = limit.clamp(1, crate::limits::FEED_LIMIT_MAX);
    if let Some(visible) = visible
        && visible.is_empty()
    {
        return Ok(Vec::new());
    }

    let mut sql = String::from(
        "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload,
                i.status, e.created_at, i.updated_at
         FROM inbox i JOIN events e ON e.id = i.event_id WHERE 1 = 1",
    );
    let mut params: Vec<Value> = Vec::new();
    if let Some(status) = status {
        params.push(Value::Text(status.to_string()));
        sql.push_str(&format!(" AND i.status = ?{}", params.len()));
    }
    if let Some(project_id) = project_id {
        params.push(Value::Text(project_id.to_string()));
        sql.push_str(&format!(" AND e.project_id = ?{}", params.len()));
    }
    if let Some(visible) = visible {
        let mut placeholders = Vec::with_capacity(visible.len());
        for id in visible {
            params.push(Value::Text(id.clone()));
            placeholders.push(format!("?{}", params.len()));
        }
        sql.push_str(&format!(
            " AND e.project_id IN ({})",
            placeholders.join(", ")
        ));
    }
    params.push(Value::Integer(limit));
    sql.push_str(&format!(
        " ORDER BY i.updated_at DESC, e.id DESC LIMIT ?{}",
        params.len()
    ));

    let conn = super::connect(db)?;
    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        items.push(item_from_row(&row)?);
    }
    Ok(items)
}

/// The status of an inbox entry, if it is tracked there.
///
/// Reads inside a caller's transaction so a status check and the write it
/// guards cannot race a concurrent writer.
pub(crate) async fn status_in_tx(conn: &Connection, event_id: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT status FROM inbox WHERE event_id = ?1",
            vec![Value::Text(event_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => match row.get_value(0).map_err(engine)? {
            Value::Text(status) => Ok(Some(status)),
            other => Err(Error::Engine(format!(
                "expected text in an inbox column, found {other:?}"
            ))),
        },
        None => Ok(None),
    }
}

/// Set an inbox entry's status.
pub async fn set_status(db: &Database, event_id: &str, status: &str) -> Result<()> {
    let conn = super::connect(db)?;
    set_status_in_tx(&conn, event_id, status).await
}

/// Set an inbox entry's status inside a caller's transaction.
pub(crate) async fn set_status_in_tx(
    conn: &Connection,
    event_id: &str,
    status: &str,
) -> Result<()> {
    validate_status(status)?;
    conn.execute(
        "UPDATE inbox SET status = ?1, updated_at = ?2 WHERE event_id = ?3",
        vec![
            Value::Text(status.to_string()),
            Value::Text(crate::store::now_rfc3339()),
            Value::Text(event_id.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// Count unread and waiting items.
pub async fn counts(db: &Database) -> Result<Counts> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query("SELECT status, COUNT(*) FROM inbox GROUP BY status", ())
        .await
        .map_err(engine)?;
    let mut counts = Counts {
        unread: 0,
        waiting: 0,
    };
    while let Some(row) = rows.next().await.map_err(engine)? {
        let status = match row.get_value(0).map_err(engine)? {
            Value::Text(status) => status,
            _ => continue,
        };
        let count = match row.get_value(1).map_err(engine)? {
            Value::Integer(count) => count,
            _ => 0,
        };
        match status.as_str() {
            "unread" => counts.unread += count,
            "action" | "waiting" => counts.waiting += count,
            _ => {}
        }
    }
    Ok(counts)
}

fn item_from_row(row: &Row) -> Result<InboxItem> {
    let payload = match text_at(row, 5)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored payload is not JSON: {err}")))?,
        ),
        None => None,
    };
    Ok(InboxItem {
        event_id: required_text(row, 0)?,
        project_id: required_text(row, 1)?,
        kind: required_text(row, 2)?,
        actor: required_text(row, 3)?,
        summary: required_text(row, 4)?,
        payload,
        status: required_text(row, 6)?,
        created_at: required_text(row, 7)?,
        updated_at: required_text(row, 8)?,
    })
}

fn validate_status(status: &str) -> Result<()> {
    if STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "unknown inbox status '{status}'"
        )))
    }
}

fn required_text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("inbox row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in an inbox column, found {other:?}"
        ))),
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
