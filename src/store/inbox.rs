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

/// Count the items that still wait on the human in one project.
///
/// `actor` confines the count to one actor; `None` counts every actor. Only an
/// open item counts, since a resolved item frees its slot and an unread or
/// read item never waited on the human.
pub(crate) async fn open_count(
    conn: &Connection,
    project_id: &str,
    actor: Option<&str>,
) -> Result<i64> {
    let mut sql = String::from(
        "SELECT COUNT(*) FROM inbox i JOIN events e ON e.id = i.event_id
         WHERE i.status IN ('action', 'waiting') AND e.project_id = ?1",
    );
    let mut params = vec![Value::Text(project_id.to_string())];
    if let Some(actor) = actor {
        params.push(Value::Text(actor.to_string()));
        sql.push_str(&format!(" AND e.actor = ?{}", params.len()));
    }
    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => row.get::<i64>(0).map_err(engine),
        None => Ok(0),
    }
}

/// Refuse a new open item when it would push a count past its cap.
///
/// Runs inside the writer's immediate transaction, so the count and the insert
/// it guards commit together and a concurrent writer cannot slip past. The
/// per-actor cap runs first, so the message names the narrowest limit.
pub(crate) async fn enforce_open_cap(
    conn: &Connection,
    project_id: &str,
    actor: &str,
    caps: &crate::limits::InboxCaps,
) -> Result<()> {
    if caps.per_actor > 0 {
        let open = open_count(conn, project_id, Some(actor)).await?;
        if open >= caps.per_actor {
            return Err(Error::RateLimited(format!(
                "{actor} already has {open} open items in {project_id}; the per-agent cap is {}",
                caps.per_actor
            )));
        }
    }
    if caps.per_project > 0 {
        let open = open_count(conn, project_id, None).await?;
        if open >= caps.per_project {
            return Err(Error::RateLimited(format!(
                "{project_id} already has {open} open items; the per-project cap is {}",
                caps.per_project
            )));
        }
    }
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
/// entries from those projects; an empty set yields nothing. The entries carry
/// their real status, so this is the human's own view.
pub async fn list_visible(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<Vec<InboxItem>> {
    list_as(db, status, project_id, limit, visible, false).await
}

/// The same listing as an agent may see it.
///
/// Whether the human has read something is the human's business, so the read
/// axis is collapsed: a `read` entry is reported as `unread`, carrying the
/// timestamp it carried before it was read, and `read` is not a status an agent
/// can filter on. An agent therefore sees exactly what it saw before the human
/// opened the item, and cannot poll the inbox to find out when that was.
pub async fn list_for_agent(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<Vec<InboxItem>> {
    list_as(db, status, project_id, limit, visible, true).await
}

async fn list_as(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
    collapse_read: bool,
) -> Result<Vec<InboxItem>> {
    if let Some(status) = status {
        if collapse_read && status == "read" {
            return Err(Error::InvalidArgument(format!(
                "unknown inbox status '{status}'"
            )));
        }
        validate_status(status)?;
    }
    let limit = limit.clamp(1, crate::limits::FEED_LIMIT_MAX);
    if let Some(visible) = visible
        && visible.is_empty()
    {
        return Ok(Vec::new());
    }

    // A read entry reads as unread and keeps the time it entered the inbox, so
    // no column of the answer moves when the human opens it.
    let projection = if collapse_read {
        "CASE WHEN i.status = 'read' THEN 'unread' ELSE i.status END,
                e.created_at,
                CASE WHEN i.status IN ('unread', 'read') THEN e.created_at ELSE i.updated_at END"
    } else {
        "i.status, e.created_at, i.updated_at"
    };
    let mut sql = format!(
        "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload,
                {projection}
         FROM inbox i JOIN events e ON e.id = i.event_id WHERE 1 = 1"
    );
    let mut params: Vec<Value> = Vec::new();
    if let Some(status) = status {
        params.push(Value::Text(status.to_string()));
        // An agent asking for unread work is asking for work the human has not
        // dealt with, which a read entry still is.
        if collapse_read && status == "unread" {
            sql.push_str(&format!(" AND i.status IN (?{}, 'read')", params.len()));
        } else {
            sql.push_str(&format!(" AND i.status = ?{}", params.len()));
        }
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
    // Ordered by the event id, which is minted in commit order and never
    // changes. Ordering on the entry's update time would let reading an item
    // pull it to the head of the list, above work that is genuinely newer, and
    // would shift the page a limit cuts.
    params.push(Value::Integer(limit));
    sql.push_str(&format!(" ORDER BY e.id DESC LIMIT ?{}", params.len()));

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

/// The two statuses the human's read verb moves between.
///
/// Read state is one axis and waiting on the human is another. An item that
/// waits, or one already resolved, carries no read state at all: marking an
/// approval read must not take it out of what waits on the human.
const READ_AXIS: &[&str] = &["unread", "read"];

/// What a read or unread call left behind.
#[derive(Debug, Clone, Serialize)]
pub struct ReadState {
    pub event_id: String,
    /// The status the entry carries now.
    pub status: String,
    /// Whether this call moved it. False when the entry was already there and
    /// false when it carries no read state.
    pub changed: bool,
}

/// Mark one inbox entry read.
pub async fn mark_read(db: &Database, event_id: &str) -> Result<ReadState> {
    set_read(db, event_id, "read").await
}

/// Mark one inbox entry unread.
pub async fn mark_unread(db: &Database, event_id: &str) -> Result<ReadState> {
    set_read(db, event_id, "unread").await
}

/// Move one entry along the read axis, leaving every other status alone.
///
/// The read and the write share an immediate transaction, so the status a
/// decision was made on is the status that is written over.
async fn set_read(db: &Database, event_id: &str, target: &str) -> Result<ReadState> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let current = status_in_tx(&tx, event_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("event {event_id} has no inbox entry to read")))?;
    if !READ_AXIS.contains(&current.as_str()) || current == target {
        return Ok(ReadState {
            event_id: event_id.to_string(),
            status: current,
            changed: false,
        });
    }
    set_status_in_tx(&tx, event_id, target).await?;
    tx.commit().await.map_err(engine)?;
    Ok(ReadState {
        event_id: event_id.to_string(),
        status: target.to_string(),
        changed: true,
    })
}

/// Mark every unread entry read, optionally within one project, and return how
/// many moved. Entries waiting on the human are left where they are.
pub async fn mark_all_read(db: &Database, project_id: Option<&str>) -> Result<i64> {
    let conn = super::connect(db)?;
    let mut sql =
        String::from("UPDATE inbox SET status = 'read', updated_at = ?1 WHERE status = 'unread'");
    let mut params = vec![Value::Text(crate::store::now_rfc3339())];
    if let Some(project_id) = project_id {
        params.push(Value::Text(project_id.to_string()));
        sql.push_str(&format!(
            " AND event_id IN (SELECT id FROM events WHERE project_id = ?{})",
            params.len()
        ));
    }
    let moved = conn.execute(&sql, params).await.map_err(engine)?;
    Ok(moved as i64)
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
