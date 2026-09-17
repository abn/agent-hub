//! The project feed: append-only, time-ordered, addressable events.

use serde::{Deserialize, Serialize};
use turso::{Connection, Database, Row, Value};

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
    /// The inbox status when the event is tracked there. `None` when it never
    /// entered the inbox. An action is open while this is `action` or
    /// `waiting`, and closed once it is `resolved`.
    pub inbox_status: Option<String>,
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
    append_with_caps(db, None, actor, idempotency_key, event).await
}

/// Append an open item with the inbox cap applied.
///
/// An open item is a question or an approval, the two kinds that wait on the
/// human. The cap check, the event insert, and the inbox projection commit in
/// one immediate transaction, so two concurrent writers cannot both slip past.
/// The cap is not applied to a replayed write: the idempotency lookup returns
/// the original id before the check, so a retry of an accepted write never
/// turns into a refusal.
pub async fn append_action(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    actor: &str,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    append_with_caps(db, Some(caps), actor, idempotency_key, event).await
}

async fn append_with_caps(
    db: &Database,
    caps: Option<&crate::limits::InboxCaps>,
    actor: &str,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let id = append_in_tx_capped(&tx, caps, actor, idempotency_key, event).await?;
    tx.commit().await.map_err(engine)?;
    Ok(id)
}

/// Append an event inside a caller's transaction.
///
/// Lets a related write, an identity change for instance, and its audit event
/// commit together, so neither can survive without the other. The inbox cap is
/// not applied here; an actionable write goes through [`append_action`].
pub(crate) async fn append_in_tx(
    tx: &turso::transaction::Transaction<'_>,
    actor: &str,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    append_in_tx_capped(tx, None, actor, idempotency_key, event).await
}

async fn append_in_tx_capped(
    tx: &turso::transaction::Transaction<'_>,
    caps: Option<&crate::limits::InboxCaps>,
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

    // A question always needs the human and roots its own thread, whichever
    // tool wrote it. An answer must name the question it replies to.
    if event.kind == "answer" && event.thread_id.is_none() {
        return Err(Error::InvalidArgument(
            "an answer must name the question it replies to".to_string(),
        ));
    }
    // A question and an approval both wait on the human, whichever surface
    // wrote them, so the rule lives here rather than at each write path.
    let needs_action = event.needs_action || event.kind == "question" || event.kind == "approval";

    // Inside the immediate transaction, so a concurrent retry with the same
    // key serialises and sees the recorded row rather than racing it. This runs
    // before the cap so a replay of an accepted write is returned as it was.
    if let Some(key) = idempotency_key
        && let Some(existing) =
            crate::store::idempotency::lookup(tx, &event.project_id, key).await?
    {
        return Ok(existing);
    }

    // Checked before any row is written, so a refused write leaves nothing
    // behind and the count and the insert share one transaction.
    if needs_action && let Some(caps) = caps {
        crate::store::inbox::enforce_open_cap(tx, &event.project_id, actor, caps).await?;
    }

    let id = ulid::Ulid::generate().to_string();
    let created_at = crate::store::now_rfc3339();
    let thread_id = if event.kind == "question" {
        Some(id.clone())
    } else {
        event.thread_id.clone()
    };

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
            optional_text(thread_id.as_deref()),
            Value::Integer(i64::from(needs_action)),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;

    index_doc(
        tx,
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
        crate::store::idempotency::record(tx, &event.project_id, key, &id, &created_at).await?;
    }

    // Finished work and anything that needs the human land in the inbox.
    if needs_action || event.kind == "finished" {
        crate::store::inbox::project(tx, &id, needs_action, &created_at).await?;
    }

    Ok(id)
}

/// Fetch one event by id.
pub async fn get(db: &Database, event_id: &str) -> Result<Option<Event>> {
    let conn = super::connect(db)?;
    get_on(&conn, event_id).await
}

/// Fetch one event inside a caller's transaction, so a read and the write it
/// guards serialise against a concurrent writer.
pub(crate) async fn get_in_tx(conn: &Connection, event_id: &str) -> Result<Option<Event>> {
    get_on(conn, event_id).await
}

async fn get_on(conn: &Connection, event_id: &str) -> Result<Option<Event>> {
    let mut rows = conn
        .query(
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status
             FROM events e LEFT JOIN inbox i ON i.event_id = e.id WHERE e.id = ?1",
            vec![Value::Text(event_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(event_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Events the human's feed surfaces show. Hub bookkeeping, `system`, is
/// excluded: the audit trail stays available to agents and the store, but a
/// calm human feed does not carry it.
pub fn human_kinds() -> Vec<String> {
    KINDS
        .iter()
        .filter(|kind| **kind != "system")
        .map(|kind| kind.to_string())
        .collect()
}

/// Read the most recent human-visible events across every project, newest
/// first.
pub async fn recent(db: &Database, limit: i64) -> Result<Vec<Event>> {
    let conn = super::connect(db)?;
    let limit = limit.clamp(1, FEED_LIMIT_MAX);
    let mut rows = conn
        .query(
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status
             FROM events e LEFT JOIN inbox i ON i.event_id = e.id WHERE e.kind <> 'system' ORDER BY e.id DESC LIMIT ?1",
            vec![Value::Integer(limit)],
        )
        .await
        .map_err(engine)?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        events.push(event_from_row(&row)?);
    }
    Ok(events)
}

/// A page of the feed with the cursors to continue in either direction.
#[derive(Debug, Clone)]
pub struct FeedPage {
    /// The events on this page, in read order.
    pub events: Vec<Event>,
    /// Newest id on the page: pass as `since` to poll for newer events.
    pub next_since: Option<String>,
    /// Oldest id on the page: pass as `before` to page further back.
    pub next_before: Option<String>,
}

/// Read a page of the feed.
pub async fn read_feed(db: &Database, project_id: &str, query: &FeedQuery) -> Result<FeedPage> {
    let conn = super::connect(db)?;
    let limit = query.limit.clamp(1, FEED_LIMIT_MAX);

    let mut sql = String::from(
        "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status
         FROM events e LEFT JOIN inbox i ON i.event_id = e.id WHERE e.project_id = ?1",
    );
    let mut params: Vec<Value> = vec![Value::Text(project_id.to_string())];

    if let Some(kinds) = query.kinds.as_ref().filter(|kinds| !kinds.is_empty()) {
        let mut placeholders = Vec::with_capacity(kinds.len());
        for kind in kinds {
            params.push(Value::Text(kind.clone()));
            placeholders.push(format!("?{}", params.len()));
        }
        sql.push_str(&format!(" AND e.kind IN ({})", placeholders.join(", ")));
    }
    if let Some(since) = &query.since {
        params.push(Value::Text(since.clone()));
        sql.push_str(&format!(" AND e.id > ?{}", params.len()));
    }
    if let Some(before) = &query.before {
        params.push(Value::Text(before.clone()));
        sql.push_str(&format!(" AND e.id < ?{}", params.len()));
    }

    // Forward from a cursor reads oldest first; otherwise newest first.
    let ascending = query.since.is_some() && query.before.is_none();
    if ascending {
        sql.push_str(" ORDER BY e.id ASC");
    } else {
        sql.push_str(" ORDER BY e.id DESC");
    }

    params.push(Value::Integer(limit));
    sql.push_str(&format!(" LIMIT ?{}", params.len()));

    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        events.push(event_from_row(&row)?);
    }

    // A forward poll that sees no new events must not drop the caller's
    // cursor, so it reports the `since` it was given and the client keeps
    // polling from the same position. An empty backward page has no older
    // event to point at, and a mixed query is not a polling shape, so both
    // stay cursorless.
    let (next_since, next_before) = match (events.first(), events.last()) {
        (Some(first), Some(last)) => {
            let (newest, oldest) = if ascending {
                (last, first)
            } else {
                (first, last)
            };
            (Some(newest.id.clone()), Some(oldest.id.clone()))
        }
        _ if ascending => (query.since.clone(), None),
        _ => (None, None),
    };
    Ok(FeedPage {
        events,
        next_since,
        next_before,
    })
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
        inbox_status: text_at(row, 9)?,
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

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
