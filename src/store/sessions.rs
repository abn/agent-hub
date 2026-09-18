//! Session metadata: the durable mapping from an agent's session name to a
//! brain file, plus its lifecycle.
//!
//! State itself lives in the brain file; this table records identity, status,
//! and the timestamps retention layers will need.

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::store::events::{self, NewEvent};

/// A session as recorded in the hub store.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub session_name: String,
    pub agent: String,
    pub status: String,
    pub brain_path: String,
    pub created_at: String,
    pub last_activity: String,
    pub deleted_at: Option<String>,
    /// The session this one was forked from, if any.
    pub forked_from: Option<String>,
    /// The session this one was adopted from, when the previous owner differs.
    pub adopted_from: Option<String>,
    /// The note the owner left when it ended the session.
    pub handoff: Option<String>,
}

/// Every session column, in the order [`session_from_row`] reads them.
const COLUMNS: &str = "id, project_id, session_name, agent, status, brain_path, \
                       created_at, last_activity, deleted_at, forked_from, adopted_from, handoff";

/// Start a session, or resume it when the same project and name already exist.
///
/// The mapping is what lets a resume find the same brain file after a
/// compaction or a restart, so it is idempotent on the name.
pub async fn start(
    db: &Database,
    project_id: &str,
    session_name: &str,
    agent: &str,
) -> Result<Session> {
    Ok(start_resumed(db, project_id, session_name, agent).await?.0)
}

/// Start a session, reporting whether it resumed one that already existed.
///
/// The surfaces tell the agent which of the two happened; the stores that only
/// need the row call [`start`].
pub async fn start_resumed(
    db: &Database,
    project_id: &str,
    session_name: &str,
    agent: &str,
) -> Result<(Session, bool)> {
    validate_id("project", project_id)?;
    validate_id("session name", session_name)?;

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    let existing = find_owned_on(&tx, project_id, agent, session_name).await?;
    if existing.is_none()
        && let Some(pruned) = find_pruned_on(&tx, project_id, agent, session_name).await?
    {
        // The name is held by a session the human pruned. Taking it would put a
        // second session behind an undo token that still points at the first,
        // and the hub does not restore what the human deleted.
        return Err(Error::Conflict(format!(
            "session '{session_name}' was pruned and can be restored with undo, or started under another name pruned_session_id={}",
            pruned.id
        )));
    }
    let created = existing.is_none();
    let now = crate::store::now_rfc3339();

    let session = match existing {
        Some(mut session) => {
            tx.execute(
                "UPDATE sessions SET status = 'active', last_activity = ?1 WHERE id = ?2",
                vec![Value::Text(now.clone()), Value::Text(session.id.clone())],
            )
            .await
            .map_err(engine)?;
            session.status = "active".to_string();
            session.last_activity = now;
            session
        }
        None => {
            let id = crate::store::next_id();
            let brain_path = brain_path(project_id, &id);
            tx.execute(
                "INSERT INTO sessions(id, project_id, session_name, agent, status, brain_path, created_at, last_activity)
                 VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7)",
                vec![
                    Value::Text(id.clone()),
                    Value::Text(project_id.to_string()),
                    Value::Text(session_name.to_string()),
                    Value::Text(agent.to_string()),
                    Value::Text(brain_path.clone()),
                    Value::Text(now.clone()),
                    Value::Text(now.clone()),
                ],
            )
            .await
            .map_err(engine)?;
            Session {
                id,
                project_id: project_id.to_string(),
                session_name: session_name.to_string(),
                agent: agent.to_string(),
                status: "active".to_string(),
                brain_path,
                created_at: now.clone(),
                last_activity: now,
                deleted_at: None,
                forked_from: None,
                adopted_from: None,
                handoff: None,
            }
        }
    };

    // A new session and its lifecycle event commit together, so a failure
    // cannot leave a live session with no `started` record.
    if created {
        events::append_in_tx(
            &tx,
            agent,
            None,
            NewEvent {
                project_id: project_id.to_string(),
                kind: "session".to_string(),
                summary: format!("session {} started", session.session_name),
                payload: Some(serde_json::json!({
                    "action": "started",
                    "session_id": session.id,
                    "session_name": session.session_name,
                })),
                needs_action: false,
                thread_id: None,
                session_id: Some(session.id.clone()),
            },
        )
        .await?;
    }

    tx.commit().await.map_err(engine)?;
    Ok((session, !created))
}

/// What picking up a session turned out to be.
///
/// The hub decides from the source's state, never the caller: an agent cannot
/// tell whether the source is still running, and a wrong guess either forks a
/// dead session or adopts a live one.
#[derive(Debug)]
pub enum Pickup {
    /// The caller named its own session under its own name: an ordinary resume.
    Resumed(Session),
    /// The source had ended, so ownership moved and the brain stayed put.
    Adopted {
        session: Session,
        /// The owner the session came from.
        from_agent: String,
    },
    /// The source is still running, so the caller gets a copy of it. The row is
    /// written by [`insert_fork`] once the brain has been copied.
    Fork(Session),
}

/// Take over or branch from another session, inside one transaction.
///
/// The resolution, every check and the write share one immediate transaction,
/// so two agents picking up one ended session cannot both be told they got it:
/// the guarded update names the row it checked, and a zero-row update is the
/// loser's conflict.
pub async fn start_from(
    db: &Database,
    project_id: &str,
    session_name: &str,
    caller: &str,
    source_id: &str,
) -> Result<Pickup> {
    validate_id("project", project_id)?;
    validate_id("session name", session_name)?;

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    let source = get_on(&tx, source_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {source_id} not found")))?;
    if source.project_id != project_id {
        return Err(Error::InvalidArgument(format!(
            "session {source_id} belongs to another project, and a brain is project-scoped from_project_id={}",
            source.project_id
        )));
    }
    if source.deleted_at.is_some() {
        // The human pruned it deliberately and holds the undo; the hub does not
        // resurrect a file the sweep is about to remove.
        return Err(Error::Conflict(format!(
            "session {source_id} is pruned; restore it with undo before picking it up session_id={source_id}"
        )));
    }
    if source.agent == caller && source.session_name == session_name {
        let now = crate::store::now_rfc3339();
        tx.execute(
            "UPDATE sessions SET status = 'active', last_activity = ?1 WHERE id = ?2",
            vec![Value::Text(now.clone()), Value::Text(source.id.clone())],
        )
        .await
        .map_err(engine)?;
        tx.commit().await.map_err(engine)?;
        let mut session = source;
        session.status = "active".to_string();
        session.last_activity = now;
        return Ok(Pickup::Resumed(session));
    }
    if source.status != "ended" {
        // A live session is copied, not taken: its owner is still writing. The
        // name is checked here as well as when the fork's row is written, so a
        // name the caller already holds is refused before a brain of any size
        // is copied for nothing.
        collision_free(&tx, project_id, caller, session_name, None).await?;
        tx.commit().await.map_err(engine)?;
        return Ok(Pickup::Fork(source));
    }

    collision_free(&tx, project_id, caller, session_name, Some(&source.id)).await?;

    let now = crate::store::now_rfc3339();
    let previous_owner = source.agent.clone();
    let adopted_from = (previous_owner != caller).then(|| source.id.clone());
    // The guard carries every condition the branch was chosen on, so a source
    // that was resumed or pruned in between matches no row.
    let moved = tx
        .execute(
            "UPDATE sessions
                SET agent = ?1, session_name = ?2, status = 'active',
                    last_activity = ?3, adopted_from = ?4
              WHERE id = ?5 AND deleted_at IS NULL AND status = 'ended'",
            vec![
                Value::Text(caller.to_string()),
                Value::Text(session_name.to_string()),
                Value::Text(now.clone()),
                adopted_from.clone().map_or(Value::Null, Value::Text),
                Value::Text(source.id.clone()),
            ],
        )
        .await
        .map_err(engine)?;
    if moved == 0 {
        // Another adopter, or the owner itself, moved the row first. Only the
        // row as it stands now says who holds it.
        let current = get_on(&tx, source_id).await?;
        return Err(Error::Conflict(match current {
            Some(current) => format!(
                "session {source_id} was picked up or resumed while adopting it owner={}",
                current.agent
            ),
            None => format!("session {source_id} is gone owner=none"),
        }));
    }

    events::append_in_tx(
        &tx,
        caller,
        None,
        NewEvent {
            project_id: project_id.to_string(),
            kind: "session".to_string(),
            summary: format!("session {session_name} picked up from {previous_owner}"),
            payload: Some(serde_json::json!({
                "action": "adopted",
                "session_id": source.id,
                "session_name": session_name,
                "from_session_id": source.id,
                "from_agent": previous_owner,
                "handoff": source.handoff,
            })),
            needs_action: false,
            thread_id: None,
            session_id: Some(source.id.clone()),
        },
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    let session = Session {
        agent: caller.to_string(),
        session_name: session_name.to_string(),
        status: "active".to_string(),
        last_activity: now,
        adopted_from,
        ..source
    };
    Ok(Pickup::Adopted {
        session,
        from_agent: previous_owner,
    })
}

/// Record a fork once its brain file is in place.
///
/// The row, the copied search rows and the lifecycle event commit together, so
/// a forked session is never listed without the entries it was forked with.
pub async fn insert_fork(
    db: &Database,
    source: &Session,
    session_name: &str,
    caller: &str,
    new_id: &str,
) -> Result<Session> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    collision_free(&tx, &source.project_id, caller, session_name, None).await?;

    let now = crate::store::now_rfc3339();
    let brain_path = brain_path(&source.project_id, new_id);
    tx.execute(
        "INSERT INTO sessions(id, project_id, session_name, agent, status, brain_path,
                              created_at, last_activity, forked_from)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?6, ?7)",
        vec![
            Value::Text(new_id.to_string()),
            Value::Text(source.project_id.clone()),
            Value::Text(session_name.to_string()),
            Value::Text(caller.to_string()),
            Value::Text(brain_path.clone()),
            Value::Text(now.clone()),
            Value::Text(source.id.clone()),
        ],
    )
    .await
    .map_err(engine)?;

    // The copy holds the source's entries, so it holds the source's search
    // rows too; `ref_id` is already the brain path, so only the keys change.
    tx.execute(
        "INSERT INTO search_docs(doc_id, project_id, type, ref_id, session_id, title, body, updated_at)
         SELECT 'brain:' || ?1 || ':' || ref_id, project_id, 'brain', ref_id, ?1, title, body, updated_at
           FROM search_docs WHERE type = 'brain' AND session_id = ?2",
        vec![
            Value::Text(new_id.to_string()),
            Value::Text(source.id.clone()),
        ],
    )
    .await
    .map_err(engine)?;

    events::append_in_tx(
        &tx,
        caller,
        None,
        NewEvent {
            project_id: source.project_id.clone(),
            kind: "session".to_string(),
            summary: format!("session {session_name} forked from {}", source.agent),
            payload: Some(serde_json::json!({
                "action": "forked",
                "session_id": new_id,
                "session_name": session_name,
                "from_session_id": source.id,
                "from_agent": source.agent,
            })),
            needs_action: false,
            thread_id: None,
            session_id: Some(new_id.to_string()),
        },
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    Ok(Session {
        id: new_id.to_string(),
        project_id: source.project_id.clone(),
        session_name: session_name.to_string(),
        agent: caller.to_string(),
        status: "active".to_string(),
        brain_path,
        created_at: now.clone(),
        last_activity: now,
        deleted_at: None,
        forked_from: Some(source.id.clone()),
        adopted_from: None,
        handoff: None,
    })
}

/// Move an active session to another agent, which only the human does.
pub async fn reassign(
    db: &Database,
    session_id: &str,
    to_agent: &str,
    actor: &str,
) -> Result<Session> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    let session = get_on(&tx, session_id)
        .await?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;
    if session.agent == to_agent {
        return Err(Error::Conflict(format!(
            "session {session_id} already belongs to {to_agent} owner={to_agent}"
        )));
    }
    // An owner no token resolves to could never resume, end or write the
    // session again, and only another reassign would recover it.
    let mut known = tx
        .query(
            "SELECT 1 FROM agents WHERE id = ?1",
            vec![Value::Text(to_agent.to_string())],
        )
        .await
        .map_err(engine)?;
    if known.next().await.map_err(engine)?.is_none() {
        return Err(Error::NotFound(format!("agent {to_agent} not found")));
    }
    drop(known);
    collision_free(
        &tx,
        &session.project_id,
        to_agent,
        &session.session_name,
        Some(session_id),
    )
    .await?;

    let now = crate::store::now_rfc3339();
    let moved = tx
        .execute(
            "UPDATE sessions SET agent = ?1, last_activity = ?2
              WHERE id = ?3 AND deleted_at IS NULL AND agent = ?4",
            vec![
                Value::Text(to_agent.to_string()),
                Value::Text(now.clone()),
                Value::Text(session_id.to_string()),
                Value::Text(session.agent.clone()),
            ],
        )
        .await
        .map_err(engine)?;
    if moved == 0 {
        return Err(Error::Conflict(format!(
            "session {session_id} changed while reassigning it owner={}",
            session.agent
        )));
    }

    events::append_in_tx(
        &tx,
        actor,
        None,
        NewEvent {
            project_id: session.project_id.clone(),
            kind: "session".to_string(),
            summary: format!("session {} reassigned to {to_agent}", session.session_name),
            payload: Some(serde_json::json!({
                "action": "reassigned",
                "session_id": session.id,
                "session_name": session.session_name,
                "from_agent": session.agent,
                "to_agent": to_agent,
            })),
            needs_action: false,
            thread_id: None,
            session_id: Some(session.id.clone()),
        },
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    Ok(Session {
        agent: to_agent.to_string(),
        last_activity: now,
        ..session
    })
}

/// Refuse a name the caller already holds live, naming what holds it.
async fn collision_free(
    conn: &turso::Connection,
    project_id: &str,
    agent: &str,
    session_name: &str,
    except: Option<&str>,
) -> Result<()> {
    if let Some(existing) = find_owned_on(conn, project_id, agent, session_name).await?
        && Some(existing.id.as_str()) != except
    {
        return Err(Error::Conflict(format!(
            "session name '{session_name}' is already in use existing_session_id={}",
            existing.id
        )));
    }
    Ok(())
}

/// Mark a session ended, with the note its owner leaves behind.
///
/// Retry-safe: ending an already-ended session is a no-op, so a retried call
/// does not append a second lifecycle event and does not clear an earlier note.
/// The note lives on the row and in the event, never in the brain: a session
/// that never wrote must not get a brain file just because it ended.
pub async fn end(
    db: &Database,
    session_id: &str,
    actor: &str,
    handoff: Option<&str>,
) -> Result<()> {
    if let Some(handoff) = handoff {
        crate::limits::check_handoff(handoff)?;
    }
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    // The status re-check and the lifecycle event share the transaction, so a
    // retried end and a concurrent end cannot both append.
    let session = get_on(&tx, session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;

    if session.status == "ended" {
        return Ok(());
    }

    let now = crate::store::now_rfc3339();
    tx.execute(
        "UPDATE sessions SET status = 'ended', last_activity = ?1,
                             handoff = COALESCE(?2, handoff)
         WHERE id = ?3",
        vec![
            Value::Text(now),
            handoff.map_or(Value::Null, |note| Value::Text(note.to_string())),
            Value::Text(session_id.to_string()),
        ],
    )
    .await
    .map_err(engine)?;

    events::append_in_tx(
        &tx,
        actor,
        None,
        NewEvent {
            project_id: session.project_id,
            kind: "session".to_string(),
            summary: format!("session {} ended", session.session_name),
            payload: Some(serde_json::json!({
                "action": "ended",
                "session_id": session.id,
                "session_name": session.session_name,
                "handoff": handoff,
            })),
            needs_action: false,
            thread_id: None,
            session_id: Some(session.id.clone()),
        },
    )
    .await?;
    tx.commit().await.map_err(engine)?;
    Ok(())
}

/// Fetch one session by id.
pub async fn get(db: &Database, session_id: &str) -> Result<Option<Session>> {
    let conn = super::connect(db)?;
    get_on(&conn, session_id).await
}

async fn get_on(conn: &turso::Connection, session_id: &str) -> Result<Option<Session>> {
    one(
        conn,
        "WHERE id = ?1",
        vec![Value::Text(session_id.to_string())],
    )
    .await
}

/// List a project's sessions, most recently active first.
pub async fn list(db: &Database, project_id: &str) -> Result<Vec<Session>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            &format!(
                "SELECT {COLUMNS} FROM sessions
                 WHERE project_id = ?1 AND deleted_at IS NULL
                 ORDER BY last_activity DESC"
            ),
            vec![Value::Text(project_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        sessions.push(session_from_row(&row)?);
    }
    Ok(sessions)
}

/// Where a session's brain file sits under the data directory.
fn brain_path(project_id: &str, session_id: &str) -> String {
    format!("sessions/{project_id}/{session_id}.db")
}

/// What an agent's session listing asks for.
#[derive(Debug, Default)]
pub struct SessionQuery<'a> {
    /// One project, or every project the caller may read.
    pub project_id: Option<&'a str>,
    /// `active` or `ended`.
    pub status: Option<&'a str>,
    /// The owner.
    pub agent: Option<&'a str>,
    /// The projects the caller may reach at all, or `None` for every project.
    pub visible: Option<&'a [String]>,
    /// Rows to return, clamped to the listing cap.
    pub limit: i64,
}

/// List sessions a caller may see, most recently active first.
///
/// Soft-deleted sessions are omitted: a pruned session is gone from a reader's
/// point of view, undo window or not.
pub async fn query(db: &Database, query: &SessionQuery<'_>) -> Result<Vec<Session>> {
    if let Some(status) = query.status
        && status != "active"
        && status != "ended"
    {
        return Err(Error::InvalidArgument(format!(
            "status '{status}' must be 'active' or 'ended'"
        )));
    }
    if let Some(visible) = query.visible
        && visible.is_empty()
    {
        return Ok(Vec::new());
    }

    let mut sql = format!("SELECT {COLUMNS} FROM sessions WHERE deleted_at IS NULL");
    let mut params: Vec<Value> = Vec::new();
    if let Some(project_id) = query.project_id {
        params.push(Value::Text(project_id.to_string()));
        sql.push_str(&format!(" AND project_id = ?{}", params.len()));
    }
    if let Some(status) = query.status {
        params.push(Value::Text(status.to_string()));
        sql.push_str(&format!(" AND status = ?{}", params.len()));
    }
    if let Some(agent) = query.agent {
        params.push(Value::Text(agent.to_string()));
        sql.push_str(&format!(" AND agent = ?{}", params.len()));
    }
    if let Some(visible) = query.visible {
        let mut placeholders = Vec::with_capacity(visible.len());
        for id in visible {
            params.push(Value::Text(id.clone()));
            placeholders.push(format!("?{}", params.len()));
        }
        sql.push_str(&format!(" AND project_id IN ({})", placeholders.join(", ")));
    }
    params.push(Value::Integer(
        query.limit.clamp(1, crate::limits::SESSION_LIST_LIMIT_MAX),
    ));
    sql.push_str(&format!(
        " ORDER BY last_activity DESC, id DESC LIMIT ?{}",
        params.len()
    ));

    let conn = super::connect(db)?;
    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        sessions.push(session_from_row(&row)?);
    }
    Ok(sessions)
}

/// Find a live session by the agent that owns it and the name it runs under.
///
/// This is the key lookup: a name belongs to one owner inside a project, so
/// another agent's session of the same name is a different session.
pub async fn find_owned(
    db: &Database,
    project_id: &str,
    agent: &str,
    session_name: &str,
) -> Result<Option<Session>> {
    let conn = super::connect(db)?;
    find_owned_on(&conn, project_id, agent, session_name).await
}

/// Find the caller's own live session of a name, inside a caller's connection.
async fn find_owned_on(
    conn: &turso::Connection,
    project_id: &str,
    agent: &str,
    session_name: &str,
) -> Result<Option<Session>> {
    one(
        conn,
        "WHERE project_id = ?1 AND session_name = ?2 AND agent = ?3 AND deleted_at IS NULL",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(session_name.to_string()),
            Value::Text(agent.to_string()),
        ],
    )
    .await
}

/// Find a pruned session still holding a name, tombstone and all.
///
/// The undo window is not the boundary: a row the sweep is about to remove is
/// not a name an agent may quietly re-take either, and once the sweep runs the
/// name is free.
async fn find_pruned_on(
    conn: &turso::Connection,
    project_id: &str,
    agent: &str,
    session_name: &str,
) -> Result<Option<Session>> {
    one(
        conn,
        "WHERE project_id = ?1 AND session_name = ?2 AND agent = ?3 AND deleted_at IS NOT NULL
         ORDER BY deleted_at DESC",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(session_name.to_string()),
            Value::Text(agent.to_string()),
        ],
    )
    .await
}

/// The first session matching a clause, or none.
async fn one(
    conn: &turso::Connection,
    clause: &str,
    params: Vec<Value>,
) -> Result<Option<Session>> {
    let mut rows = conn
        .query(&format!("SELECT {COLUMNS} FROM sessions {clause}"), params)
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(session_from_row(&row)?)),
        None => Ok(None),
    }
}

fn session_from_row(row: &Row) -> Result<Session> {
    let text = |index: usize| -> Result<String> {
        match row.get_value(index).map_err(engine)? {
            Value::Text(value) => Ok(value),
            other => Err(Error::Engine(format!(
                "expected text in a session column, found {other:?}"
            ))),
        }
    };
    let optional = |index: usize| -> Result<Option<String>> {
        Ok(match row.get_value(index).map_err(engine)? {
            Value::Text(value) => Some(value),
            _ => None,
        })
    };
    Ok(Session {
        id: text(0)?,
        project_id: text(1)?,
        session_name: text(2)?,
        agent: text(3)?,
        status: text(4)?,
        brain_path: text(5)?,
        created_at: text(6)?,
        last_activity: text(7)?,
        deleted_at: optional(8)?,
        forked_from: optional(9)?,
        adopted_from: optional(10)?,
        handoff: optional(11)?,
    })
}

fn validate_id(kind: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "{kind} '{value}' must be non-empty and contain only ASCII alphanumerics, hyphens, and underscores"
        )))
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
