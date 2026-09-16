//! The project feed: append-only, time-ordered, addressable events.

use serde::{Deserialize, Serialize};
use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::limits::{self, FEED_LIMIT_DEFAULT, FEED_LIMIT_MAX};
use crate::store::search::{SearchDoc, index_doc};

/// The closed set of event kinds.
pub const KINDS: &[&str] = &[
    "signal", "finished", "question", "answer", "approval", "artifact", "session", "system",
];

/// A stored event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub actor: String,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
    pub thread_id: Option<String>,
    pub needs_action: bool,
    pub created_at: String,
}

/// A new event to append.
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub project_id: String,
    pub kind: String,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
    pub needs_action: bool,
    pub thread_id: Option<String>,
}

/// How to read a page of the feed.
#[derive(Debug, Clone)]
pub struct FeedQuery {
    /// Return events after this id, oldest first.
    pub since: Option<String>,
    /// Return events before this id, newest first.
    pub before: Option<String>,
    /// Maximum events to return.
    pub limit: i64,
    /// Restrict to these kinds.
    pub kinds: Option<Vec<String>>,
}

impl Default for FeedQuery {
    fn default() -> Self {
        Self {
            since: None,
            before: None,
            limit: FEED_LIMIT_DEFAULT,
            kinds: None,
        }
    }
}

/// Append an event and index it, returning its id.
///
/// When an idempotency key is given and already recorded for the project, the
/// original event id is returned and nothing new is written. The event, its
/// search row, and the idempotency record commit together.
pub async fn append(
    db: &Database,
    actor: &str,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    validate_kind(&event.kind)?;
    let payload_text = event.payload.as_ref().map(|value| value.to_string());
    limits::check_event(
        &event.summary,
        payload_text.as_deref().map(str::len).unwrap_or(0),
    )?;

    let mut conn = db.connect().map_err(engine)?;

    if let Some(key) = idempotency_key
        && let Some(existing) =
            crate::store::idempotency::lookup(&conn, &event.project_id, key).await?
    {
        return Ok(existing);
    }

    let id = ulid::Ulid::generate().to_string();
    let created_at = now();
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    tx.execute(
        "INSERT INTO events(id, project_id, kind, actor, summary, payload, thread_id, needs_action, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        vec![
            Value::Text(id.clone()),
            Value::Text(event.project_id.clone()),
            Value::Text(event.kind.clone()),
            Value::Text(actor.to_string()),
            Value::Text(event.summary.clone()),
            optional_text(payload_text.as_deref()),
            optional_text(event.thread_id.as_deref()),
            Value::Integer(i64::from(event.needs_action)),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;

    index_doc(
        &tx,
        SearchDoc {
            doc_id: &format!("event:{id}"),
            project_id: &event.project_id,
            kind: "feed",
            ref_id: &id,
            session_id: None,
            title: Some(&event.summary),
            body: payload_text.as_deref().unwrap_or(""),
            updated_at: &created_at,
        },
    )
    .await?;

    if let Some(key) = idempotency_key {
        crate::store::idempotency::record(&tx, &event.project_id, key, &id, &created_at).await?;
    }

    tx.commit().await.map_err(engine)?;
    Ok(id)
}

/// Read a page of the feed.
pub async fn read_feed(db: &Database, project_id: &str, query: &FeedQuery) -> Result<Vec<Event>> {
    let conn = db.connect().map_err(engine)?;
    let limit = query.limit.clamp(1, FEED_LIMIT_MAX);

    let mut sql = String::from(
        "SELECT id, project_id, kind, actor, summary, payload, thread_id, needs_action, created_at
         FROM events WHERE project_id = ?1",
    );
    let mut params: Vec<Value> = vec![Value::Text(project_id.to_string())];

    if let Some(kinds) = query.kinds.as_ref().filter(|kinds| !kinds.is_empty()) {
        let mut placeholders = Vec::with_capacity(kinds.len());
        for kind in kinds {
            params.push(Value::Text(kind.clone()));
            placeholders.push(format!("?{}", params.len()));
        }
        sql.push_str(&format!(" AND kind IN ({})", placeholders.join(", ")));
    }
    if let Some(since) = &query.since {
        params.push(Value::Text(since.clone()));
        sql.push_str(&format!(" AND id > ?{}", params.len()));
    }
    if let Some(before) = &query.before {
        params.push(Value::Text(before.clone()));
        sql.push_str(&format!(" AND id < ?{}", params.len()));
    }

    // Forward from a cursor reads oldest first; otherwise newest first.
    if query.since.is_some() && query.before.is_none() {
        sql.push_str(" ORDER BY id ASC");
    } else {
        sql.push_str(" ORDER BY id DESC");
    }

    params.push(Value::Integer(limit));
    sql.push_str(&format!(" LIMIT ?{}", params.len()));

    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        events.push(event_from_row(&row)?);
    }
    Ok(events)
}

fn validate_kind(kind: &str) -> Result<()> {
    if KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "unknown event kind '{kind}'"
        )))
    }
}

fn event_from_row(row: &Row) -> Result<Event> {
    let payload = match text_at(row, 5)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored event payload is not JSON: {err}")))?,
        ),
        None => None,
    };
    let needs_action = match row.get_value(7).map_err(engine)? {
        Value::Integer(value) => value != 0,
        _ => false,
    };
    Ok(Event {
        id: required_text(row, 0)?,
        project_id: required_text(row, 1)?,
        kind: required_text(row, 2)?,
        actor: required_text(row, 3)?,
        summary: required_text(row, 4)?,
        payload,
        thread_id: text_at(row, 6)?,
        needs_action,
        created_at: required_text(row, 8)?,
    })
}

fn required_text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("event row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in a column, found {other:?}"
        ))),
    }
}

fn optional_text(value: Option<&str>) -> Value {
    match value {
        Some(text) => Value::Text(text.to_string()),
        None => Value::Null,
    }
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
