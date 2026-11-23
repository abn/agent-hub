//! The project feed: append-only, time-ordered, addressable events.

use serde::{Deserialize, Serialize};
use turso::{Connection, Database, Row, Value};

use crate::error::{Error, Result};
use crate::limits::{self, FEED_LIMIT_DEFAULT, FEED_LIMIT_MAX};
use crate::store::search::{SearchDoc, index_doc};

/// The closed set of event kinds.
pub const KINDS: &[&str] = &[
    "signal", "finished", "question", "answer", "approval", "artifact", "session", "system",
    "idea", "anchor", "score", "comment", "page",
];

/// The kinds written for agents and left out of the human's feed. A comment
/// and a page write are read where they happen, on the artifact and on the
/// page, so the feed, its unseen count and Home's recent list do not carry
/// them. An agent reads them through `feed_read` and subscriptions.
pub const AGENT_ONLY_KINDS: &[&str] = &["comment", "page"];

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
    /// Who wrote it.
    #[serde(default = "crate::author::AuthorView::unrecorded")]
    pub author: crate::author::AuthorView,
    /// `hub` for the hub's own act, null for a call.
    #[serde(default)]
    pub origin: Option<String>,
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
    /// Restrict to this session.
    pub session_id: Option<String>,
    /// Opt in to the hub's own audit events. This is a confinement rather
    /// than a filter: it defaults to false, so a query that leaves it unset
    /// never sees the audit trail, whatever kinds it names.
    pub include_audit: bool,
    /// Leave the events the human dismissed out of the page. The human
    /// surfaces set it; an agent's read leaves it false, because a dismissal
    /// is the human's view of the feed and not a deletion.
    pub hide_dismissed: bool,
    /// Restrict to events whose author passes this filter (`member`, `agent`,
    /// `kind`). Empty matches every author.
    pub author: crate::author::AuthorFilter,
}

impl Default for FeedQuery {
    fn default() -> Self {
        Self {
            since: None,
            before: None,
            limit: FEED_LIMIT_DEFAULT,
            kinds: None,
            session_id: None,
            include_audit: false,
            hide_dismissed: false,
            author: crate::author::AuthorFilter::default(),
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
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    append_with_caps(
        db,
        None,
        events_per_project,
        writer,
        idempotency_key,
        event,
        None,
    )
    .await
    .map(|appended| appended.id)
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
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    append_action_outcome(
        db,
        caps,
        events_per_project,
        writer,
        idempotency_key,
        event,
        None,
    )
    .await
    .map(|appended| appended.id)
}

/// What an append did: the event's id, and whether it was a replay of an
/// earlier write with the same idempotency key rather than a new event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    pub id: String,
    pub replayed: bool,
}

/// [`append_action_with_deadline`], also saying whether the write was a
/// replay.
pub async fn append_action_outcome(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
    deadline: Option<&crate::store::inbox::Deadline>,
) -> Result<Appended> {
    append_with_caps(
        db,
        Some(caps),
        events_per_project,
        writer,
        idempotency_key,
        event,
        deadline,
    )
    .await
}

/// As [`append_action`], with the deadline the asking agent set on the item.
///
/// The deadline is written in the same transaction as the item, so an item
/// with a deadline is never visible without it. A replayed write returns the
/// original item and leaves its deadline as it was.
pub async fn append_action_with_deadline(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
    deadline: Option<&crate::store::inbox::Deadline>,
) -> Result<String> {
    append_action_outcome(
        db,
        caps,
        events_per_project,
        writer,
        idempotency_key,
        event,
        deadline,
    )
    .await
    .map(|appended| appended.id)
}

pub async fn append_for_principal(
    db: &Database,
    events_per_project: i64,
    principal: &crate::principal::Principal,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    append_with_caps_and_principal(
        db,
        None,
        events_per_project,
        principal,
        idempotency_key,
        event,
        None,
    )
    .await
    .map(|appended| appended.id)
}

/// An open item written under a principal, with the deadline the agent set.
pub async fn append_action_for_principal(
    db: &Database,
    caps: &crate::limits::InboxCaps,
    events_per_project: i64,
    principal: &crate::principal::Principal,
    idempotency_key: Option<&str>,
    event: NewEvent,
    deadline: Option<&crate::store::inbox::Deadline>,
) -> Result<Appended> {
    append_with_caps_and_principal(
        db,
        Some(caps),
        events_per_project,
        principal,
        idempotency_key,
        event,
        deadline,
    )
    .await
}

async fn append_with_caps(
    db: &Database,
    caps: Option<&crate::limits::InboxCaps>,
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
    deadline: Option<&crate::store::inbox::Deadline>,
) -> Result<Appended> {
    let tx = super::begin_write(db).await?;
    let appended = append_in_tx_capped(
        &tx,
        caps,
        events_per_project,
        writer,
        idempotency_key,
        event,
    )
    .await?;
    if !appended.replayed
        && let Some(deadline) = deadline
    {
        crate::store::inbox::set_deadline_in_tx(&tx, &appended.id, deadline).await?;
    }
    tx.commit().await.map_err(engine)?;
    Ok(appended)
}

async fn append_with_caps_and_principal(
    db: &Database,
    caps: Option<&crate::limits::InboxCaps>,
    events_per_project: i64,
    principal: &crate::principal::Principal,
    idempotency_key: Option<&str>,
    event: NewEvent,
    deadline: Option<&crate::store::inbox::Deadline>,
) -> Result<Appended> {
    let tx = super::begin_write(db).await?;
    crate::policy::authorize_in_tx(
        &tx,
        principal,
        &event.project_id,
        crate::policy::Access::Write,
    )
    .await?;
    let writer = principal.writer(event.session_id.as_deref());
    let appended = append_in_tx_capped(
        &tx,
        caps,
        events_per_project,
        writer,
        idempotency_key,
        event,
    )
    .await?;
    if !appended.replayed
        && let Some(deadline) = deadline
    {
        crate::store::inbox::set_deadline_in_tx(&tx, &appended.id, deadline).await?;
    }
    tx.commit().await.map_err(engine)?;
    Ok(appended)
}

/// Append an event inside a caller's transaction.
///
/// Lets a related write, an identity change for instance, and its audit event
/// commit together, so neither can survive without the other. The inbox cap is
/// not applied here; an actionable write goes through [`append_action`]. A
/// caller with no configured ceiling passes `0`.
pub(crate) async fn append_in_tx(
    tx: &crate::store::WriteTx,
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<String> {
    append_in_tx_capped(tx, None, events_per_project, writer, idempotency_key, event)
        .await
        .map(|appended| appended.id)
}

/// Append an event, reporting whether it is new. A write an idempotency key
/// replayed returns the original id, marked as a replay.
async fn append_in_tx_capped(
    tx: &crate::store::WriteTx,
    caps: Option<&crate::limits::InboxCaps>,
    events_per_project: i64,
    writer: impl Into<crate::author::Writer>,
    idempotency_key: Option<&str>,
    event: NewEvent,
) -> Result<Appended> {
    let writer: crate::author::Writer = writer.into();
    let actor = writer.actor();
    validate_kind(&event.kind)?;
    let payload_text = event.payload.as_ref().map(|value| value.to_string());
    limits::check_event(
        &event.summary,
        payload_text.as_deref().map(str::len).unwrap_or(0),
    )?;

    let mut p_rows = tx
        .query(
            "SELECT status FROM projects WHERE id = ?1",
            vec![Value::Text(event.project_id.clone())],
        )
        .await
        .map_err(engine)?;
    let p_row = p_rows
        .next()
        .await
        .map_err(engine)?
        .ok_or_else(|| Error::NotFound(format!("project {} not found", event.project_id)))?;
    let status: String = match p_row.get_value(0).map_err(engine)? {
        Value::Text(s) => s,
        _ => "active".to_string(),
    };
    if status != "active" {
        return Err(Error::NotFound(format!(
            "project {} not found",
            event.project_id
        )));
    }

    // A question and an approval both wait on the human, whichever surface
    // wrote them, so the rule lives here rather than at each write path.
    let needs_action = event.needs_action || event.kind == "question" || event.kind == "approval";

    // Inside the immediate transaction, so a concurrent retry with the same
    // key serialises and sees the recorded row rather than racing it. This runs
    // before the thread check and the cap, so a replay of an accepted write is
    // returned as it was even when its thread no longer resolves.
    if let Some(key) = idempotency_key
        && let Some(existing) =
            crate::store::idempotency::lookup(tx, &event.project_id, &event.kind, key).await?
    {
        return Ok(Appended {
            id: existing,
            replayed: true,
        });
    }

    // A project's feed is bounded so a runaway writer cannot fill the store one
    // event at a time. The count runs on this transaction, so it sees the same
    // committed state the insert below extends, and the hub's own audit trail is
    // excluded exactly as `count_for_project` excludes it. A replay returned
    // above is never refused. Zero disables the ceiling. An audit event is
    // exempt because it is not counted in the first place.
    if events_per_project > 0 && event.kind != AUDIT_KIND {
        let count = count_on(
            tx,
            "SELECT COUNT(*) FROM events WHERE project_id = ?1 AND kind <> 'system'",
            vec![Value::Text(event.project_id.clone())],
        )
        .await?;
        if count >= events_per_project {
            return Err(Error::Conflict(format!(
                "project {} has reached its event ceiling of {} (HUB_EVENTS_PER_PROJECT); \
                 promote durable work to the project knowledge base or prune the feed",
                event.project_id, events_per_project
            )));
        }
    }

    // A question roots its own thread, whichever tool wrote it. An answer must
    // name the question or the approval it replies to, and an approval may sit
    // inside a thread. Anything else names a root, so a thread is read by one
    // id and never forks under one of its own replies.
    if event.kind == "answer" && event.thread_id.is_none() {
        return Err(Error::InvalidArgument(
            "an answer must name the question it replies to".to_string(),
        ));
    }
    if event.kind != "question"
        && let Some(tid) = &event.thread_id
    {
        let root = match get_in_tx(tx, tid).await? {
            Some(root) if root.project_id == event.project_id => root,
            _ => return Err(Error::NotFound(format!("event {tid} not found"))),
        };
        if event.kind != "answer"
            && let Some(thread) = root.thread_id.as_deref()
            && thread != root.id
        {
            return Err(Error::InvalidArgument(format!(
                "event {tid} is a reply in thread {thread}; name that thread instead"
            )));
        }
    }

    // Checked before any row is written, so a refused write leaves nothing
    // behind and the count and the insert share one transaction.
    if needs_action && let Some(caps) = caps {
        crate::store::inbox::enforce_open_cap(tx, &event.project_id, actor, caps).await?;
    }

    let id = crate::store::next_id();
    let created_at = crate::store::now_rfc3339();
    let thread_id =
        if (event.kind == "question" || event.kind == "idea") && event.thread_id.is_none() {
            Some(id.clone())
        } else {
            event.thread_id.clone()
        };

    let session_id = event
        .session_id
        .clone()
        .or_else(|| writer.author.session().map(str::to_string));
    tx.execute(
        "INSERT INTO events(id, project_id, kind, actor, summary, payload, thread_id, needs_action,
            created_at, session_id, author_kind, member_id, agent, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
            optional_text(session_id.as_deref()),
            Value::Text(writer.author.kind().as_str().to_string()),
            optional_text(writer.author.member()),
            optional_text(writer.author.agent()),
            optional_text(writer.origin.as_column()),
        ],
    )
    .await
    .map_err(engine)?;

    // One relaxed add on the path that just wrote the row. The registry is
    // global because this transaction helper has no `AppState` in scope.
    crate::metrics::record_event(&event.kind);

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
        crate::store::idempotency::record(
            tx,
            &event.project_id,
            &event.kind,
            key,
            &id,
            &created_at,
        )
        .await?;
    }

    // Finished work and anything that needs the human land in the inbox.
    if needs_action || event.kind == "finished" {
        crate::store::inbox::project(tx, &id, needs_action, &created_at).await?;
    }

    Ok(Appended {
        id,
        replayed: false,
    })
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
/// project's feed is ignored rather than trusted: it says nothing about this
/// feed. The events written for agents are not on it.
/// The read and the write share an immediate transaction, so a cursor cannot
/// step over an event committed between them.
pub async fn mark_seen(db: &Database, project_id: &str, event_id: &str) -> Result<Seen> {
    let tx = super::begin_write(db).await?;
    let current = last_seen_on(&tx, project_id).await?;
    let belongs = get_on(&tx, event_id).await?.is_some_and(|event| {
        event.project_id == project_id && !AGENT_ONLY_KINDS.contains(&event.kind.as_str())
    });
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
    tx: &crate::store::WriteTx,
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

/// What a dismissal call did.
#[derive(Debug, Clone, Serialize)]
pub struct Dismissed {
    /// The events this call newly dismissed. An event already dismissed, an
    /// event of another project, and an id that names nothing are all left
    /// out rather than counted again.
    pub dismissed: i64,
}

/// Dismiss named events from one project's human feed.
///
/// A dismissal is read state, not deletion: the event keeps its id, its inbox
/// row, and its search document, so an agent still reads it. The events are
/// checked against the project in the same transaction as the insert, so an
/// id from another project can never be dismissed here, and the hub's own
/// audit trail and the events written for agents are left alone.
pub async fn dismiss(db: &Database, project_id: &str, event_ids: &[String]) -> Result<Dismissed> {
    let tx = super::begin_write(db).await?;
    let now = crate::store::now_rfc3339();
    let mut dismissed = 0;
    for event_id in event_ids {
        dismissed += tx
            .execute(
                "INSERT OR IGNORE INTO feed_dismissals(event_id, project_id, dismissed_at)
                 SELECT id, project_id, ?3 FROM events
                  WHERE id = ?1 AND project_id = ?2 AND kind <> 'system'
                    AND kind NOT IN ('comment', 'page')",
                vec![
                    Value::Text(event_id.clone()),
                    Value::Text(project_id.to_string()),
                    Value::Text(now.clone()),
                ],
            )
            .await
            .map_err(engine)?;
    }
    tx.commit().await.map_err(engine)?;
    Ok(Dismissed {
        dismissed: dismissed as i64,
    })
}

/// Dismiss every event at or below the project's read cursor.
///
/// That is what "read" means on a feed: the cursor is a high-water mark, so
/// every event at or under it counts as seen, whether or not it was on screen.
/// A project with no cursor has read nothing, so the call dismisses nothing.
/// The hub's own audit trail is left alone: it is read by kind on purpose, and
/// a bulk dismissal must not quietly hide it. A long history is written in
/// batches, so the process-wide write turn is released between them.
pub async fn dismiss_read(db: &Database, project_id: &str) -> Result<Dismissed> {
    let Some(cursor) = last_seen(db, project_id).await? else {
        return Ok(Dismissed { dismissed: 0 });
    };
    let now = crate::store::now_rfc3339();
    let mut total = 0;
    // The seek moves with the batch, so a long history is read once rather
    // than walked again for every batch.
    let mut after = String::new();
    loop {
        let tx = super::begin_write(db).await?;
        let ids = {
            let mut rows = tx
                .query(
                    "SELECT e.id FROM events e
                      WHERE e.project_id = ?1 AND e.id > ?2 AND e.id <= ?3
                        AND e.kind <> 'system'
                        AND e.kind NOT IN ('comment', 'page')
                        AND NOT EXISTS (SELECT 1 FROM feed_dismissals d WHERE d.event_id = e.id)
                      ORDER BY e.id ASC LIMIT ?4",
                    vec![
                        Value::Text(project_id.to_string()),
                        Value::Text(after.clone()),
                        Value::Text(cursor.clone()),
                        Value::Integer(crate::limits::FEED_DISMISS_BATCH),
                    ],
                )
                .await
                .map_err(engine)?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await.map_err(engine)? {
                ids.push(required_text(&row, 0)?);
            }
            ids
        };
        if ids.is_empty() {
            tx.commit().await.map_err(engine)?;
            break;
        }
        after = ids.last().cloned().unwrap_or_default();
        let placeholders = ids
            .iter()
            .map(|_| "(?, ?, ?)")
            .collect::<Vec<_>>()
            .join(",");
        let mut params = Vec::with_capacity(ids.len() * 3);
        for id in &ids {
            params.push(Value::Text(id.clone()));
            params.push(Value::Text(project_id.to_string()));
            params.push(Value::Text(now.clone()));
        }
        let inserted = tx
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO feed_dismissals(event_id, project_id, dismissed_at)
                     VALUES {placeholders}"
                ),
                params,
            )
            .await
            .map_err(engine)?;
        total += inserted as i64;
        let full = ids.len() as i64 == crate::limits::FEED_DISMISS_BATCH;
        tx.commit().await.map_err(engine)?;
        if !full {
            break;
        }
    }
    Ok(Dismissed { dismissed: total })
}

/// Forget a project's dismissals inside a caller's transaction, so a deleted
/// project takes them along with everything else scoped to it.
pub(crate) async fn forget_dismissals_in_tx(
    tx: &crate::store::WriteTx,
    project_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM feed_dismissals WHERE project_id = ?1",
        vec![Value::Text(project_id.to_string())],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// Forget one session's dismissals inside a caller's transaction, as prune
/// removes the session lifecycle events they point at.
pub(crate) async fn forget_session_dismissals_in_tx(
    tx: &crate::store::WriteTx,
    session_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM feed_dismissals WHERE event_id IN
         (SELECT id FROM events WHERE kind = 'session' AND session_id = ?1)",
        vec![Value::Text(session_id.to_string())],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// An agent's own read position in one project's feed.
///
/// This is the durable, server-side cursor `feed_read` falls back to when the
/// caller sends no `since`: the agent records no cursor itself and a restarted
/// agent resumes where it stopped. The key is the resolved actor, the stable
/// label on `events.actor` and `sessions.agent`, not the token.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSeen {
    pub project_id: String,
    pub agent: String,
    /// The newest event this agent has read, absent until it has read a page.
    pub last_seen: Option<String>,
    /// Whether this call moved the cursor.
    pub advanced: bool,
}

/// The last event id one agent has read in one project.
pub async fn agent_cursor(db: &Database, project_id: &str, agent: &str) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    agent_cursor_on(&conn, project_id, agent).await
}

async fn agent_cursor_on(
    conn: &Connection,
    project_id: &str,
    agent: &str,
) -> Result<Option<String>> {
    feed_cursor_on(
        conn,
        project_id,
        &crate::author::ParticipantKey::from_label(agent),
    )
    .await
}

async fn feed_cursor_on(
    conn: &Connection,
    project_id: &str,
    key: &crate::author::ParticipantKey,
) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT last_seen_event_id FROM feed_cursors
             WHERE project_id = ?1 AND member_id = ?2 AND agent = ?3",
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(key.member.clone()),
                Value::Text(key.agent.clone()),
            ],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => text_at(&row, 0),
        None => Ok(None),
    }
}

/// Advance one agent's cursor in one project to an event it has read.
///
/// The cursor never moves backwards, and an id that is not an event of this
/// project is ignored rather than trusted: it says nothing about this feed.
/// The read and the write share an immediate transaction, so a cursor cannot
/// step over an event committed between them. An empty id is likewise ignored,
/// which lets a caller pass the `next_since` of an empty forward poll without
/// moving the cursor or inserting an empty row.
pub async fn advance_agent_cursor(
    db: &Database,
    project_id: &str,
    agent: &str,
    event_id: &str,
) -> Result<AgentSeen> {
    let tx = super::begin_write(db).await?;
    let current = agent_cursor_on(&tx, project_id, agent).await?;
    let belongs = !event_id.is_empty()
        && get_on(&tx, event_id)
            .await?
            .is_some_and(|event| event.project_id == project_id);
    let forward = belongs && current.as_deref().is_none_or(|seen| event_id > seen);
    if !forward {
        return Ok(AgentSeen {
            project_id: project_id.to_string(),
            agent: agent.to_string(),
            last_seen: current,
            advanced: false,
        });
    }

    let key = crate::author::ParticipantKey::from_label(agent);
    tx.execute(
        "INSERT OR REPLACE INTO feed_cursors(project_id, member_id, agent, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(key.member),
            Value::Text(key.agent),
            Value::Text(event_id.to_string()),
            Value::Text(crate::store::now_rfc3339()),
        ],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;

    Ok(AgentSeen {
        project_id: project_id.to_string(),
        agent: agent.to_string(),
        last_seen: Some(event_id.to_string()),
        advanced: true,
    })
}

/// Forget every agent's cursor for one project inside a caller's transaction,
/// so a deleted project takes them along with everything else scoped to it.
pub(crate) async fn forget_agent_cursors_in_tx(
    tx: &crate::store::WriteTx,
    project_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM feed_cursors WHERE project_id = ?1",
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
     WHERE e.project_id = ?1 AND e.id > ?2 AND e.kind <> 'system'
       AND e.kind NOT IN ('comment', 'page')
       AND NOT EXISTS (SELECT 1 FROM feed_dismissals d WHERE d.event_id = e.id)";

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

/// Which of a project's events above a cursor a session brief reads.
///
/// The audit trail is never among them.
#[derive(Debug, Clone, Copy)]
pub enum BriefFilter<'a> {
    /// Every event.
    All,
    /// Items still waiting on someone: the inbox status is `action` or
    /// `waiting`.
    Open,
    /// Answers and decisions on the questions and approvals this actor
    /// posted.
    AnswersTo(&'a str),
}

/// The clause and parameters a brief read runs on, numbered from `?1`.
fn brief_clause(
    project_id: &str,
    since: Option<&str>,
    filter: BriefFilter<'_>,
) -> (String, Vec<Value>) {
    let mut sql = String::from(
        "FROM events e LEFT JOIN inbox i ON i.event_id = e.id
         WHERE e.project_id = ?1 AND e.id > ?2 AND e.kind <> 'system'",
    );
    let mut params = vec![
        Value::Text(project_id.to_string()),
        Value::Text(since.unwrap_or_default().to_string()),
    ];
    match filter {
        BriefFilter::All => {}
        BriefFilter::Open => sql.push_str(" AND i.status IN ('action', 'waiting')"),
        BriefFilter::AnswersTo(actor) => {
            params.push(Value::Text(actor.to_string()));
            sql.push_str(
                " AND e.kind = 'answer' AND EXISTS (
                     SELECT 1 FROM events q WHERE q.id = e.thread_id AND q.actor = ?3
                       AND q.kind IN ('question', 'approval'))",
            );
        }
    }
    (sql, params)
}

/// The newest of one project's events above `since` that `filter` keeps, at
/// most `limit`. With no `since` every event of the project qualifies.
pub async fn brief_events(
    db: &Database,
    project_id: &str,
    since: Option<&str>,
    filter: BriefFilter<'_>,
    limit: i64,
) -> Result<Vec<Event>> {
    let conn = super::connect(db)?;
    let (clause, mut params) = brief_clause(project_id, since, filter);
    params.push(Value::Integer(limit.clamp(1, FEED_LIMIT_MAX)));
    let sql = format!(
        "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id,
                e.needs_action, e.created_at, i.status, e.author_kind, e.member_id, e.agent, e.session_id, e.origin, (SELECT s.session_name FROM sessions s WHERE s.id = e.session_id)
         {clause} ORDER BY e.id DESC LIMIT ?{}",
        params.len()
    );
    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        events.push(event_from_row(&row)?);
    }
    Ok(events)
}

/// How many of the same events there are, counting no further than
/// `count_max`.
pub async fn brief_count(
    db: &Database,
    project_id: &str,
    since: Option<&str>,
    filter: BriefFilter<'_>,
    count_max: i64,
) -> Result<i64> {
    let conn = super::connect(db)?;
    let (clause, mut params) = brief_clause(project_id, since, filter);
    params.push(Value::Integer(count_max.max(1)));
    let sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 {clause} LIMIT ?{})",
        params.len()
    );
    count_on(&conn, &sql, params).await
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
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status, e.author_kind, e.member_id, e.agent, e.session_id, e.origin, (SELECT s.session_name FROM sessions s WHERE s.id = e.session_id)
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
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status, e.author_kind, e.member_id, e.agent, e.session_id, e.origin, (SELECT s.session_name FROM sessions s WHERE s.id = e.session_id)
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
        .filter(|kind| **kind != AUDIT_KIND && !AGENT_ONLY_KINDS.contains(kind))
        .map(|kind| kind.to_string())
        .collect()
}

/// What a comment was posted on.
#[derive(Debug, Clone, Copy)]
pub enum CommentTarget<'a> {
    Artifact {
        project_id: &'a str,
        artifact_id: &'a str,
        title: &'a str,
    },
    Page {
        project_id: &'a str,
        path: &'a str,
    },
}

/// The feed event for a comment that was just posted.
///
/// Its payload names the actors the comment concerns in `audience`, so a
/// subscription with `about_self` can keep the discussion of its own work and
/// drop the rest. For an artifact that is whoever published or updated it and
/// whoever commented before; for a page it is whoever commented before and the
/// page's last writer. Only comments posted before this one count, so a
/// concurrent comment is never read as an earlier one. The author is left out:
/// a comment is not news to the one who wrote it.
pub async fn comment_event(
    db: &Database,
    target: CommentTarget<'_>,
    comment_id: &str,
    author: &str,
) -> Result<NewEvent> {
    let conn = super::connect(db)?;
    let mut reached: Vec<Reached> = Vec::new();
    let (project_id, summary, mut audience, payload) = match target {
        CommentTarget::Artifact {
            project_id,
            artifact_id,
            title,
        } => {
            let mut audience = actors_of(
                &conn,
                "SELECT DISTINCT actor FROM events
                  WHERE project_id = ?1 AND kind = 'artifact'
                    AND json_extract(payload, '$.action') IN ('published', 'updated')
                    AND json_extract(payload, '$.artifact_id') = ?2",
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Text(artifact_id.to_string()),
                ],
            )
            .await?;
            audience.extend(
                actors_of(
                    &conn,
                    "SELECT DISTINCT author FROM comments WHERE artifact_id = ?1 AND id < ?2",
                    vec![
                        Value::Text(artifact_id.to_string()),
                        Value::Text(comment_id.to_string()),
                    ],
                )
                .await?,
            );
            reached.extend(
                reached_by(
                    &conn,
                    "SELECT actor, session_id, member_id, agent FROM events
                      WHERE project_id = ?1 AND kind = 'artifact'
                        AND json_extract(payload, '$.action') IN ('published', 'updated')
                        AND json_extract(payload, '$.artifact_id') = ?2",
                    vec![
                        Value::Text(project_id.to_string()),
                        Value::Text(artifact_id.to_string()),
                    ],
                )
                .await?,
            );
            reached.extend(
                reached_by(
                    &conn,
                    "SELECT author, session_id, member_id, agent FROM comments
                      WHERE artifact_id = ?1 AND id < ?2",
                    vec![
                        Value::Text(artifact_id.to_string()),
                        Value::Text(comment_id.to_string()),
                    ],
                )
                .await?,
            );
            (
                project_id,
                format!("comment on {title}"),
                audience,
                serde_json::json!({"target": "artifact", "artifact_id": artifact_id}),
            )
        }
        CommentTarget::Page { project_id, path } => {
            let mut audience = actors_of(
                &conn,
                "SELECT DISTINCT author FROM kb_comments
                  WHERE project_id = ?1 AND path = ?2 AND id < ?3",
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Text(path.to_string()),
                    Value::Text(comment_id.to_string()),
                ],
            )
            .await?;
            reached.extend(
                reached_by(
                    &conn,
                    "SELECT author, session_id, member_id, agent FROM kb_comments
                      WHERE project_id = ?1 AND path = ?2 AND id < ?3",
                    vec![
                        Value::Text(project_id.to_string()),
                        Value::Text(path.to_string()),
                        Value::Text(comment_id.to_string()),
                    ],
                )
                .await?,
            );
            if let Some(written) = newest_page_event(&conn, project_id, path).await? {
                reached.push(Reached {
                    actor: written.actor.clone(),
                    session: written.session.clone(),
                    member: written.member.clone(),
                    agent: written.agent.clone(),
                });
                audience.push(written.actor);
            }
            (
                project_id,
                format!("comment on {path}"),
                audience,
                serde_json::json!({"target": "page", "path": path}),
            )
        }
    };
    audience.retain(|actor| actor != author);
    audience.sort();
    audience.dedup();
    let mut payload = payload;
    payload["comment_id"] = serde_json::json!(comment_id);
    payload["author"] = serde_json::json!(author);
    payload["audience"] = serde_json::json!(audience);
    reached.retain(|row| row.actor != author);
    let (sessions, participants) = audience_of(&reached);
    payload["audience_sessions"] = serde_json::json!(sessions);
    payload["audience_participants"] = serde_json::json!(participants);
    Ok(NewEvent {
        project_id: project_id.to_string(),
        kind: "comment".to_string(),
        summary,
        payload: Some(payload),
        needs_action: false,
        thread_id: None,
        session_id: None,
    })
}

/// Append the feed event for a comment that was just posted.
///
/// The comment is already stored, so a failure here is logged and costs the
/// post nothing: the event is the notice, not the record.
pub async fn record_comment(
    db: &Database,
    events_per_project: i64,
    target: CommentTarget<'_>,
    comment_id: &str,
    author: impl Into<crate::author::Writer>,
) {
    let writer: crate::author::Writer = author.into();
    let appended = async {
        let event = comment_event(db, target, comment_id, writer.actor()).await?;
        append(db, events_per_project, &writer, None, event).await
    }
    .await;
    if let Err(err) = appended {
        tracing::warn!(comment_id, error = %err, "could not append a comment event");
    }
}

/// How long one actor's writes to a page stay a single `page` event.
const PAGE_COALESCE_SECS: i64 = 10 * 60;

/// The `page` event for a write, or `None` when the actor's earlier write to
/// the same page is recent enough that this one is part of the same stretch of
/// editing.
///
/// `prev_actor` is whoever wrote the page before this write. It is the
/// audience: they are the one who may want to know their page changed under
/// them. It stays the audience when the writer is the same actor, and
/// `exclude_self` on a subscription decides whether they hear of it.
pub async fn page_event(
    db: &Database,
    project_id: &str,
    path: &str,
    actor: &str,
    prev_actor: Option<&str>,
    created: bool,
) -> Result<Option<NewEvent>> {
    let conn = super::connect(db)?;
    if let Some(newest) = newest_page_event(&conn, project_id, path).await?
        && newest.actor == actor
        && age_secs(&newest.created_at).is_some_and(|age| age < PAGE_COALESCE_SECS)
    {
        return Ok(None);
    }
    let audience: Vec<&str> = prev_actor.into_iter().collect();
    // The previous writer's session and participant, when the newest page
    // event is theirs.
    let previous = newest_page_event(&conn, project_id, path)
        .await?
        .filter(|written| Some(written.actor.as_str()) == prev_actor)
        .map(|written| Reached {
            actor: written.actor,
            session: written.session,
            member: written.member,
            agent: written.agent,
        });
    let (sessions, participants) = audience_of(previous.as_slice());
    Ok(Some(NewEvent {
        project_id: project_id.to_string(),
        kind: "page".to_string(),
        summary: format!(
            "page {path} {}",
            if created { "created" } else { "updated" }
        ),
        payload: Some(serde_json::json!({
            "path": path,
            "action": if created { "created" } else { "updated" },
            "prev_actor": prev_actor,
            "audience": audience,
            "audience_sessions": sessions,
            "audience_participants": participants,
        })),
        needs_action: false,
        thread_id: None,
        session_id: None,
    }))
}

struct PageEvent {
    actor: String,
    created_at: String,
    session: Option<String>,
    member: Option<String>,
    agent: Option<String>,
}

/// A row that puts its writer in a comment or page audience.
struct Reached {
    actor: String,
    session: Option<String>,
    member: Option<String>,
    agent: Option<String>,
}

async fn reached_by(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Reached>> {
    let mut rows = conn.query(sql, params).await.map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        out.push(Reached {
            actor: required_text_at(&row, 0)?,
            session: text_at(&row, 1)?,
            member: text_at(&row, 2)?,
            agent: text_at(&row, 3)?,
        });
    }
    Ok(out)
}

/// The sessions and participants a set of writers names, deduplicated.
fn audience_of(reached: &[Reached]) -> (Vec<String>, Vec<serde_json::Value>) {
    let mut sessions: Vec<String> = reached.iter().filter_map(|r| r.session.clone()).collect();
    sessions.sort();
    sessions.dedup();
    let mut keys: Vec<(String, String)> = reached
        .iter()
        .filter_map(|r| {
            r.member
                .clone()
                .map(|member| (member, r.agent.clone().unwrap_or_default()))
        })
        .collect();
    keys.sort();
    keys.dedup();
    let participants = keys
        .into_iter()
        .map(|(member, agent)| serde_json::json!({"member": member, "agent": agent}))
        .collect();
    (sessions, participants)
}

async fn newest_page_event(
    conn: &Connection,
    project_id: &str,
    path: &str,
) -> Result<Option<PageEvent>> {
    let mut rows = conn
        .query(
            "SELECT actor, created_at, session_id, member_id, agent FROM events
              WHERE project_id = ?1 AND kind = 'page'
                AND json_extract(payload, '$.path') = ?2
              ORDER BY id DESC LIMIT 1",
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(path.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        return Ok(None);
    };
    Ok(Some(PageEvent {
        actor: required_text_at(&row, 0)?,
        created_at: required_text_at(&row, 1)?,
        session: text_at(&row, 2)?,
        member: text_at(&row, 3)?,
        agent: text_at(&row, 4)?,
    }))
}

async fn actors_of(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<String>> {
    let mut rows = conn.query(sql, params).await.map_err(engine)?;
    let mut actors = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        actors.push(required_text_at(&row, 0)?);
    }
    Ok(actors)
}

fn required_text_at(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("event row is missing a column".to_string()))
}

/// Seconds since an RFC 3339 timestamp, or `None` when it does not parse.
fn age_secs(stamp: &str) -> Option<i64> {
    let then =
        time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339).ok()?;
    Some((time::OffsetDateTime::now_utc() - then).whole_seconds())
}

/// Read the most recent human-visible events across every project, newest
/// first. Events the human dismissed are left out: Home carries the same calm
/// view as the feed, not everything the store holds.
pub async fn recent(db: &Database, limit: i64) -> Result<Vec<Event>> {
    let conn = super::connect(db)?;
    let limit = limit.clamp(1, FEED_LIMIT_MAX);
    let mut rows = conn
        .query(
            "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status, e.author_kind, e.member_id, e.agent, e.session_id, e.origin, (SELECT s.session_name FROM sessions s WHERE s.id = e.session_id)
             FROM events e LEFT JOIN inbox i ON i.event_id = e.id
             WHERE e.kind <> 'system'
               AND e.kind NOT IN ('comment', 'page')
               AND NOT EXISTS (SELECT 1 FROM feed_dismissals d WHERE d.event_id = e.id)
             ORDER BY e.id DESC LIMIT ?1",
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

/// Read a page of the feed, optionally scoped to a project.
pub async fn read_feed_scoped(
    db: &Database,
    project_id: Option<&str>,
    query: &FeedQuery,
) -> Result<FeedPage> {
    let conn = super::connect(db)?;
    let limit = query.limit.clamp(1, FEED_LIMIT_MAX);

    let mut sql = String::from(
        "SELECT e.id, e.project_id, e.kind, e.actor, e.summary, e.payload, e.thread_id, e.needs_action, e.created_at, i.status, e.author_kind, e.member_id, e.agent, e.session_id, e.origin, (SELECT s.session_name FROM sessions s WHERE s.id = e.session_id)
         FROM events e LEFT JOIN inbox i ON i.event_id = e.id WHERE 1=1",
    );
    let mut params: Vec<Value> = Vec::new();

    if let Some(project_id) = project_id {
        params.push(Value::Text(project_id.to_string()));
        sql.push_str(&format!(" AND e.project_id = ?{}", params.len()));
    }
    if let Some(session_id) = &query.session_id {
        let resolved = match crate::store::sessions::lookup_id(db, session_id).await? {
            Some(s) if s.deleted_at.is_none() => s.id,
            _ => {
                return Ok(FeedPage {
                    events: Vec::new(),
                    next_since: None,
                    next_before: None,
                });
            }
        };
        params.push(Value::Text(resolved));
        sql.push_str(&format!(" AND e.session_id = ?{}", params.len()));
    }
    if let Some(kinds) = query.kinds.as_ref().filter(|kinds| !kinds.is_empty()) {
        let mut placeholders = Vec::with_capacity(kinds.len());
        for kind in kinds {
            params.push(Value::Text(kind.clone()));
            placeholders.push(format!("?{}", params.len()));
        }
        sql.push_str(&format!(" AND e.kind IN ({})", placeholders.join(", ")));
    }
    let author = &query.author;
    if let Some(member) = &author.member {
        params.push(Value::Text(member.clone()));
        sql.push_str(&format!(" AND e.member_id = ?{}", params.len()));
    }
    if let Some(agent) = &author.agent {
        params.push(Value::Text(agent.clone()));
        sql.push_str(&format!(" AND e.agent = ?{}", params.len()));
    }
    if let Some(kind) = author.kind {
        params.push(Value::Text(kind.as_str().to_string()));
        sql.push_str(&format!(" AND e.author_kind = ?{}", params.len()));
    }
    if let Some(label) = &author.label {
        params.push(Value::Text(label.clone()));
        sql.push_str(&format!(
            " AND e.author_kind = 'legacy' AND e.actor = ?{}",
            params.len()
        ));
    }
    if !query.include_audit {
        params.push(Value::Text(AUDIT_KIND.to_string()));
        sql.push_str(&format!(" AND e.kind <> ?{}", params.len()));
    }
    if query.hide_dismissed {
        // A dismissal is the human's read state: the event stays for every
        // agent read, and only this page leaves it out.
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM feed_dismissals d WHERE d.event_id = e.id)");
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

/// Read a page of one project's feed.
pub async fn read_feed(db: &Database, project_id: &str, query: &FeedQuery) -> Result<FeedPage> {
    read_feed_scoped(db, Some(project_id), query).await
}

/// Read a page of the feed across all projects.
pub async fn read_global_feed(db: &Database, query: &FeedQuery) -> Result<FeedPage> {
    read_feed_scoped(db, None, query).await
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
        author: author_at(row, 10, 3)?.view(text_at(row, 15)?.as_deref()),
        origin: text_at(row, 14)?,
    })
}

/// The author record of a row whose author columns start at `first`
/// (`author_kind, member_id, agent, session_id`), with its `actor` column at
/// `actor`.
pub(crate) fn author_at(row: &Row, first: usize, actor: usize) -> Result<crate::author::Author> {
    Ok(crate::author::Author::from_columns(
        text_at(row, first)?.as_deref(),
        text_at(row, first + 1)?.as_deref(),
        text_at(row, first + 2)?.as_deref(),
        text_at(row, first + 3)?.as_deref(),
        &text_at(row, actor)?.unwrap_or_default(),
    ))
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

// Feed cursors per participant.

/// The newest event a participant has read in a project's feed.
pub async fn feed_cursor(
    db: &Database,
    project_id: &str,
    key: &crate::author::ParticipantKey,
) -> Result<Option<String>> {
    let conn = super::connect(db)?;
    feed_cursor_on(&conn, project_id, key).await
}

/// Move a participant's feed cursor from `from` to `to`. False when another
/// read moved it first.
pub async fn advance_feed_cursor(
    db: &Database,
    project_id: &str,
    key: &crate::author::ParticipantKey,
    from: Option<&str>,
    to: &str,
) -> Result<bool> {
    let tx = super::begin_write(db).await?;
    let current = feed_cursor_on(&tx, project_id, key).await?;
    if current.as_deref() != from {
        return Ok(false);
    }
    if from.is_some_and(|from| to <= from) {
        return Ok(true);
    }
    tx.execute(
        "INSERT OR REPLACE INTO feed_cursors(project_id, member_id, agent, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(key.member.clone()),
            Value::Text(key.agent.clone()),
            Value::Text(to.to_string()),
            Value::Text(crate::store::now_rfc3339()),
        ],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;
    Ok(true)
}
