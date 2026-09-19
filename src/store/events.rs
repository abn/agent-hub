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

/// The kind carrying the hub's record of itself: who was created, whose trust
/// changed, which tokens were issued and revoked. The identity store is its
/// only writer and the human its only reader, so it is neither in the search
/// corpus nor in a feed read by an agent.
pub const AUDIT_KIND: &str = "system";

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
    /// The session the write happened during, when one is in scope. It is what
    /// a session detail screen counts over, so it covers ordinary work as well
    /// as the lifecycle events that name a session in their payload.
    pub session_id: Option<String>,
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
    /// Opt in to the hub's own audit events. This is a confinement rather
    /// than a filter: it defaults to false, so a query that leaves it unset
    /// never sees the audit trail, whatever kinds it names.
    pub include_audit: bool,
}

impl Default for FeedQuery {
    fn default() -> Self {
        Self {
            since: None,
            before: None,
            limit: FEED_LIMIT_DEFAULT,
            kinds: None,
            include_audit: false,
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
    if event.kind != "question"
        && let Some(tid) = &event.thread_id
    {
        let root = get_in_tx(tx, tid).await?;
        match root {
            Some(root) if root.project_id == event.project_id => {}
            _ => return Err(Error::NotFound(format!("event {tid} not found"))),
        }
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

    let id = crate::store::next_id();
    let created_at = crate::store::now_rfc3339();
    let thread_id = if event.kind == "question" {
        Some(id.clone())
    } else {
        event.thread_id.clone()
    };

    tx.execute(
        "INSERT INTO events(id, project_id, kind, actor, summary, payload, thread_id, needs_action, created_at, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            optional_text(event.session_id.as_deref()),
        ],
    )
    .await
    .map_err(engine)?;

    // The corpus carries no event kind, so an audit event that reaches it can
    // no longer be held back from a search. It is kept out instead; the human
    // reads the trail through the feed, by kind.
    if event.kind != AUDIT_KIND {
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
    }

    if let Some(key) = idempotency_key {
        crate::store::idempotency::record(tx, &event.project_id, key, &id, &created_at).await?;
    }

    // Finished work and anything that needs the human land in the inbox.
    if needs_action || event.kind == "finished" {
        crate::store::inbox::project(tx, &id, needs_action, &created_at).await?;
    }

    Ok(id)
}

/// How far the human has read one project's feed.
///
/// Read state on the feed is not the inbox's: nothing here is marked by hand.
/// Every event above the cursor is unseen, which is what draws the dot, and
/// opening the feed moves the cursor to the newest event on the page.
#[derive(Debug, Clone, Serialize)]
pub struct Seen {
    pub project_id: String,
    /// The newest event the human has seen, absent until the feed is opened.
    pub last_seen: Option<String>,
    /// Whether this call moved the cursor.
    pub advanced: bool,
}

/// What one project holds above its cursor.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectUnseen {
    pub project_id: String,
    /// Feed events newer than the cursor, the hub's own audit trail excluded.
    pub events: i64,
}

/// The last event id the human has seen in one project.
pub async fn last_seen(db: &Database, project_id: &str) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    last_seen_on(&conn, project_id).await
}

async fn last_seen_on(conn: &Connection, project_id: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT last_seen_event_id FROM project_feed_cursors WHERE project_id = ?1",
            vec![Value::Text(project_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => text_at(&row, 0),
        None => Ok(None),
    }
}

/// Move a project's cursor up to an event the human has seen.
///
/// The cursor never moves backwards, and an id that is not an event of this
/// project is ignored rather than trusted: it says nothing about this feed.
/// The read and the write share an immediate transaction, so a cursor cannot
/// step over an event committed between them.
pub async fn mark_seen(db: &Database, project_id: &str, event_id: &str) -> Result<Seen> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let current = last_seen_on(&tx, project_id).await?;
    let belongs = get_on(&tx, event_id)
        .await?
        .is_some_and(|event| event.project_id == project_id);
    let forward = belongs && current.as_deref().is_none_or(|seen| event_id > seen);
    if !forward {
        return Ok(Seen {
            project_id: project_id.to_string(),
            last_seen: current,
            advanced: false,
        });
    }

    tx.execute(
        "INSERT OR REPLACE INTO project_feed_cursors(project_id, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3)",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(event_id.to_string()),
            Value::Text(crate::store::now_rfc3339()),
        ],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;

    Ok(Seen {
        project_id: project_id.to_string(),
        last_seen: Some(event_id.to_string()),
        advanced: true,
    })
}

/// Forget a project's cursor inside a caller's transaction, so a deleted
/// project takes it along with everything else scoped to it.
pub(crate) async fn forget_cursor_in_tx(
    tx: &turso::transaction::Transaction<'_>,
    project_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM project_feed_cursors WHERE project_id = ?1",
        vec![Value::Text(project_id.to_string())],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// How many events one project holds above its cursor.
///
/// Both bounds are parameters, so the engine seeks into the feed index by
/// project and by id and touches only the events above the cursor. A project
/// with no cursor passes the empty string, which sorts below every id.
pub const UNSEEN_COUNT_SQL: &str = "SELECT COUNT(*) FROM events e
     WHERE e.project_id = ?1 AND e.id > ?2 AND e.kind <> 'system'";

/// How many events one project holds above its cursor.
pub async fn unseen_count(db: &Database, project_id: &str) -> Result<i64> {
    let conn = super::connect(db)?;
    let cursor = last_seen_on(&conn, project_id).await?;
    count_unseen(&conn, project_id, cursor.as_deref()).await
}

async fn count_unseen(conn: &Connection, project_id: &str, cursor: Option<&str>) -> Result<i64> {
    count_on(
        conn,
        UNSEEN_COUNT_SQL,
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(cursor.unwrap_or_default().to_string()),
        ],
    )
    .await
}

/// The same count for every project that has anything unseen.
///
/// One small read of the projects and their cursors, then one seek per project
/// into the feed index: the feed itself is never scanned, whatever it holds.
/// The projects are the roll, so nothing is reported for an id the project
/// listing does not carry, and a project with nothing above its cursor is
/// absent rather than zero.
pub async fn unseen_counts(db: &Database) -> Result<Vec<ProjectUnseen>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT p.id, c.last_seen_event_id FROM projects p
             LEFT JOIN project_feed_cursors c ON c.project_id = p.id",
            (),
        )
        .await
        .map_err(engine)?;
    let mut cursors = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        cursors.push((required_text(&row, 0)?, text_at(&row, 1)?));
    }

    let mut counts = Vec::new();
    for (project_id, cursor) in cursors {
        let events = count_unseen(&conn, &project_id, cursor.as_deref()).await?;
        if events > 0 {
            counts.push(ProjectUnseen { project_id, events });
        }
    }
    Ok(counts)
}

/// How many events one session produced.
///
/// One count over `events_session`, so a session detail screen shows a real
/// number without reading a single payload.
pub async fn count_for_session(db: &Database, session_id: &str) -> Result<i64> {
    let conn = super::connect(db)?;
    count_on(
        &conn,
        "SELECT COUNT(*) FROM events WHERE session_id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
}

/// How many events a project holds, the hub's own audit trail excluded.
pub async fn count_for_project(db: &Database, project_id: &str) -> Result<i64> {
    let conn = super::connect(db)?;
    count_on(
        &conn,
        "SELECT COUNT(*) FROM events WHERE project_id = ?1 AND kind <> 'system'",
        vec![Value::Text(project_id.to_string())],
    )
    .await
}

/// The newest event a session produced, which is the line a session detail
/// shows as its audit row.
pub async fn latest_for_session(db: &Database, session_id: &str) -> Result<Option<Event>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status
             FROM events e LEFT JOIN inbox i ON i.event_id = e.id
             WHERE e.session_id = ?1 ORDER BY e.id DESC LIMIT 1",
            vec![Value::Text(session_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(event_from_row(&row)?)),
        None => Ok(None),
    }
}

pub(crate) async fn count_on(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<i64> {
    let mut rows = conn.query(sql, params).await.map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(match row.get_value(0).map_err(engine)? {
            Value::Integer(count) => count,
            _ => 0,
        }),
        None => Ok(0),
    }
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
        .filter(|kind| **kind != AUDIT_KIND)
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
    if !query.include_audit {
        params.push(Value::Text(AUDIT_KIND.to_string()));
        sql.push_str(&format!(" AND e.kind <> ?{}", params.len()));
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
