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
    /// The name the projects list shows, absent when no project row carries
    /// the id. A screen that lists items says where each is from by this.
    pub project_display_name: Option<String>,
    pub kind: String,
    pub actor: String,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// How an approval was decided, once it has been.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    /// How a question was answered, once it has been.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
}

/// The outcome of an approval, as its inbox entry shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    /// `approved` or `declined`.
    pub decision: String,
    /// What the human said with it, when they said anything.
    pub note: Option<String>,
    /// Who decided.
    pub actor: String,
    /// The answer event on the approval's thread that records the decision.
    pub event_id: String,
    pub decided_at: String,
}

/// How a question was answered, as its inbox entry shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    /// What was written in reply.
    pub body: String,
    /// Who answered.
    pub actor: String,
    /// The answer event on the question's thread that records the answer.
    pub event_id: String,
    pub answered_at: String,
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

/// A page of inbox entries with the cursor to poll from.
#[derive(Debug, Clone)]
pub struct InboxPage {
    /// The entries on this page.
    pub items: Vec<InboxItem>,
    /// The newest event this page accounts for: pass it as `since` to poll.
    /// For a resolved entry it is the answer or decision event, so a caller
    /// that has seen the answer is not shown the entry resolved again.
    pub next_since: Option<String>,
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
    Ok(
        list_as(db, status, project_id, None, None, limit, visible, false)
            .await?
            .items,
    )
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
    Ok(
        page_for_agent(db, status, project_id, None, None, limit, visible)
            .await?
            .items,
    )
}

/// A page of the same listing as an agent may see it, with a cursor.
///
/// `actor` keeps only entries written by that actor; `since` keeps only entries
/// whose own event, or the answer or decision that resolved them, is newer than
/// the cursor. The cursor covers the deciding event as well as the item itself,
/// so an answer that arrives after the caller last looked is returned even
/// though the item's own id is older. `next_since` points at the newest such
/// event on the page, or back at `since` when the page is empty, so a poll that
/// sees nothing does not lose its place.
pub async fn page_for_agent(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    actor: Option<&str>,
    since: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
) -> Result<InboxPage> {
    list_as(db, status, project_id, actor, since, limit, visible, true).await
}

/// A listing with the agent's filters and cursor, shared by the plain list and
/// the page. The parameters are the query the surfaces pass through.
#[allow(clippy::too_many_arguments)]
async fn list_as(
    db: &Database,
    status: Option<&str>,
    project_id: Option<&str>,
    actor: Option<&str>,
    since: Option<&str>,
    limit: i64,
    visible: Option<&[String]>,
    collapse_read: bool,
) -> Result<InboxPage> {
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
        return Ok(InboxPage {
            items: Vec::new(),
            next_since: since.map(str::to_string),
        });
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
    // The answer that decides the entry, when one has: an answer on the entry's
    // own thread in the same project. A cursor counts it, so an answer that
    // arrived after the caller's cursor is seen even though the entry's own id
    // is older. A question has one answer; `MAX` takes the latest if data is
    // odd, which is the safe choice for a cursor.
    let effective = "(SELECT MAX(a.id) FROM events a
         WHERE a.kind = 'answer' AND a.thread_id = e.id AND a.project_id = e.project_id)";
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
    if let Some(actor) = actor {
        params.push(Value::Text(actor.to_string()));
        sql.push_str(&format!(" AND e.actor = ?{}", params.len()));
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
    if let Some(since) = since {
        params.push(Value::Text(since.to_string()));
        sql.push_str(&format!(
            " AND COALESCE({effective}, e.id) > ?{}",
            params.len()
        ));
    }
    // Ordered by the event id, which is minted in commit order and never
    // changes. Ordering on the entry's update time would let reading an item
    // pull it to the head of the list, above work that is genuinely newer, and
    // would shift the page a limit cuts. A forward poll from a cursor orders by
    // the deciding event instead, so an answered question lands at the head it
    // belongs at, above older entries.
    params.push(Value::Integer(limit));
    let order = if since.is_some() {
        format!("COALESCE({effective}, e.id) DESC")
    } else {
        "e.id DESC".to_string()
    };
    sql.push_str(&format!(" ORDER BY {order} LIMIT ?{}", params.len()));

    let conn = super::connect(db)?;
    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        items.push(item_from_row(&row)?);
    }
    drop(rows);
    attach_decisions(&conn, &mut items).await?;
    attach_answers(&conn, &mut items).await?;
    name_projects(&conn, &mut items).await?;
    let next_since = match items.first() {
        Some(first) => Some(item_effective_id(first).to_string()),
        None => since.map(str::to_string),
    };
    Ok(InboxPage { items, next_since })
}

/// The event id a cursor counts for one entry.
///
/// It is the entry's own event, or the answer or decision that resolved it
/// when one has: the deciding event is minted after the item and so is always
/// the newer id.
pub(crate) fn item_effective_id(item: &InboxItem) -> &str {
    item.answer
        .as_ref()
        .map(|answer| answer.event_id.as_str())
        .or_else(|| {
            item.decision
                .as_ref()
                .map(|decision| decision.event_id.as_str())
        })
        .unwrap_or(item.event_id.as_str())
}

/// Give each item on a page the name of its project, in one read for the page.
///
/// The name is of a project the item already names by id, so a confined
/// listing learns nothing it was not already shown.
async fn name_projects(conn: &Connection, items: &mut [InboxItem]) -> Result<()> {
    let ids: Vec<&str> = items.iter().map(|item| item.project_id.as_str()).collect();
    let names = super::projects::display_names(conn, &ids).await?;
    for item in items.iter_mut() {
        item.project_display_name = names.get(&item.project_id).cloned();
    }
    Ok(())
}

/// Give each decided approval on a page its outcome, in one read for the page.
///
/// The decision is the answer event the decision route appends to the
/// approval's thread. It is looked up by the ids of entries already listed,
/// and an answer held by another project is not the entry's own, so a listing
/// shows nothing its confinement did not already allow.
async fn attach_decisions(conn: &Connection, items: &mut [InboxItem]) -> Result<()> {
    let decided: Vec<&str> = items
        .iter()
        .filter(|item| item.kind == "approval" && item.status == "resolved")
        .map(|item| item.event_id.as_str())
        .collect();
    if decided.is_empty() {
        return Ok(());
    }
    let holes: Vec<String> = (1..=decided.len()).map(|at| format!("?{at}")).collect();
    let mut rows = conn
        .query(
            &format!(
                "SELECT thread_id, project_id, id, actor, payload, created_at FROM events
                 WHERE kind = 'answer' AND thread_id IN ({}) ORDER BY id ASC",
                holes.join(", ")
            ),
            decided
                .iter()
                .map(|id| Value::Text(id.to_string()))
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(engine)?;
    let mut found: Vec<(String, String, Decision)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let payload: serde_json::Value = match text_at(&row, 4)? {
            Some(json) => serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored payload is not JSON: {err}")))?,
            None => continue,
        };
        let Some(decision) = payload.get("decision").and_then(|value| value.as_str()) else {
            continue;
        };
        found.push((
            required_text(&row, 0)?,
            required_text(&row, 1)?,
            Decision {
                decision: decision.to_string(),
                note: payload
                    .get("note")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                actor: required_text(&row, 3)?,
                event_id: required_text(&row, 2)?,
                decided_at: required_text(&row, 5)?,
            },
        ));
    }
    for item in items.iter_mut() {
        if item.kind != "approval" || item.status != "resolved" {
            continue;
        }
        item.decision = found
            .iter()
            .find(|(thread, project, _)| *thread == item.event_id && *project == item.project_id)
            .map(|(_, _, decision)| decision.clone());
    }
    Ok(())
}

/// Give each resolved question on a page its answer, in one read for the page.
///
/// The answer is the answer event the question route appends to the question's
/// thread. It is looked up by the ids of entries already listed, and an answer
/// held by another project is not the entry's own, so a listing shows nothing
/// its confinement did not already allow.
async fn attach_answers(conn: &Connection, items: &mut [InboxItem]) -> Result<()> {
    let answered: Vec<&str> = items
        .iter()
        .filter(|item| item.kind == "question" && item.status == "resolved")
        .map(|item| item.event_id.as_str())
        .collect();
    if answered.is_empty() {
        return Ok(());
    }
    let holes: Vec<String> = (1..=answered.len()).map(|at| format!("?{at}")).collect();
    let mut rows = conn
        .query(
            &format!(
                "SELECT thread_id, project_id, id, actor, payload, created_at FROM events
                 WHERE kind = 'answer' AND thread_id IN ({}) ORDER BY id ASC",
                holes.join(", ")
            ),
            answered
                .iter()
                .map(|id| Value::Text(id.to_string()))
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(engine)?;
    let mut found: Vec<(String, String, Answer)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let payload: serde_json::Value = match text_at(&row, 4)? {
            Some(json) => serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored payload is not JSON: {err}")))?,
            None => continue,
        };
        let Some(body) = payload.get("body").and_then(|value| value.as_str()) else {
            continue;
        };
        found.push((
            required_text(&row, 0)?,
            required_text(&row, 1)?,
            Answer {
                body: body.to_string(),
                actor: required_text(&row, 3)?,
                event_id: required_text(&row, 2)?,
                answered_at: required_text(&row, 5)?,
            },
        ));
    }
    for item in items.iter_mut() {
        if item.kind != "question" || item.status != "resolved" {
            continue;
        }
        item.answer = found
            .iter()
            .find(|(thread, project, _)| *thread == item.event_id && *project == item.project_id)
            .map(|(_, _, answer)| answer.clone());
    }
    Ok(())
}

/// The newest items that still wait on the human, across every project.
///
/// This is the human's own view, so it is not confined. The count of the whole
/// queue is [`counts`]; this is only the head of it.
pub async fn open_items(db: &Database, limit: i64) -> Result<Vec<InboxItem>> {
    let limit = limit.clamp(1, crate::limits::FEED_LIMIT_MAX);
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload,
                    i.status, e.created_at, i.updated_at
             FROM inbox i JOIN events e ON e.id = i.event_id
             WHERE i.status IN ('action', 'waiting')
             ORDER BY e.id DESC LIMIT ?1",
            vec![Value::Integer(limit)],
        )
        .await
        .map_err(engine)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        items.push(item_from_row(&row)?);
    }
    drop(rows);
    name_projects(&conn, &mut items).await?;
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
        decision: None,
        answer: None,
        project_display_name: None,
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
