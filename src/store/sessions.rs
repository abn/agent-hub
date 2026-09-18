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
            let brain_path = format!("sessions/{project_id}/{id}.db");
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
            },
        )
        .await?;
    }

    tx.commit().await.map_err(engine)?;
    Ok(session)
}

/// Mark a session ended. The brain is retained until the human prunes it.
///
/// Retry-safe: ending an already-ended session is a no-op, so a retried call
/// does not append a second lifecycle event.
pub async fn end(db: &Database, session_id: &str, actor: &str) -> Result<()> {
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
        "UPDATE sessions SET status = 'ended', last_activity = ?1 WHERE id = ?2",
        vec![Value::Text(now), Value::Text(session_id.to_string())],
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
            })),
            needs_action: false,
            thread_id: None,
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
