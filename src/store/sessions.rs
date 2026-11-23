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

    let tx = super::begin_write(db).await?;

    assert_project_active(&tx, project_id).await?;

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
            0,
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

    let tx = super::begin_write(db).await?;

    assert_project_active(&tx, project_id).await?;

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
        0,
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
    let tx = super::begin_write(db).await?;

    assert_project_active(&tx, &source.project_id).await?;

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
        0,
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
    let tx = super::begin_write(db).await?;

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
        0,
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
    expected_owner: Option<&str>,
    handoff: Option<&str>,
) -> Result<()> {
    if let Some(handoff) = handoff {
        crate::limits::check_handoff(handoff)?;
    }
    let tx = super::begin_write(db).await?;

    // The status re-check and the lifecycle event share the transaction, so a
    // retried end and a concurrent end cannot both append.
    let session = get_on(&tx, session_id)
        .await?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;

    if expected_owner.is_some_and(|expected| session.agent != expected) {
        return Err(Error::Forbidden(format!(
            "session {} belongs to another agent owner={}",
            session.id, session.agent
        )));
    }

    if session.status == "ended" {
        return Ok(());
    }

    let now = crate::store::now_rfc3339();
    let moved = if let Some(expected) = expected_owner {
        tx.execute(
            "UPDATE sessions SET status = 'ended', last_activity = ?1,
                                 handoff = COALESCE(?2, handoff)
             WHERE id = ?3 AND agent = ?4 AND status = 'active' AND deleted_at IS NULL",
            vec![
                Value::Text(now),
                handoff.map_or(Value::Null, |note| Value::Text(note.to_string())),
                Value::Text(session_id.to_string()),
                Value::Text(expected.to_string()),
            ],
        )
        .await
        .map_err(engine)?
    } else {
        tx.execute(
            "UPDATE sessions SET status = 'ended', last_activity = ?1,
                                 handoff = COALESCE(?2, handoff)
             WHERE id = ?3 AND status = 'active' AND deleted_at IS NULL",
            vec![
                Value::Text(now),
                handoff.map_or(Value::Null, |note| Value::Text(note.to_string())),
                Value::Text(session_id.to_string()),
            ],
        )
        .await
        .map_err(engine)?
    };

    if moved == 0 {
        let current = get_on(&tx, session_id).await?;
        if let Some(s) = current {
            if s.status == "ended" {
                return Ok(());
            }
            if expected_owner.is_some_and(|expected| s.agent != expected) {
                return Err(Error::Forbidden(format!(
                    "session {} belongs to another agent owner={}",
                    session_id, s.agent
                )));
            }
        }
        return Err(Error::Conflict(format!(
            "session {session_id} state changed while ending it"
        )));
    }

    events::append_in_tx(
        &tx,
        0,
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
    // An ended session has no one left to deliver to, so the subscriptions made
    // for it go in the same commit.
    super::subscriptions::forget_session_in_tx(&tx, session_id).await?;
    tx.commit().await.map_err(engine)?;
    Ok(())
}

/// How often one session's activity timestamp reaches the store.
///
/// A constant, not configuration: it is the write rate, not the meaning of the
/// number. A busy agent makes many calls a minute and every one of them
/// resolves its session, so writing each would turn brain traffic into hub
/// writes for a timestamp no surface reads at that resolution.
pub const ACTIVITY_COALESCE_SECS: i64 = 60;

/// What the server remembers about when each session was last touched.
///
/// `last_activity` on a row means activity only if something writes it while
/// the agent works. Every tool call that resolves an active session touches it
/// through here, and this coalesces the writes.
#[derive(Debug, Default)]
pub struct Activity {
    last: std::sync::Mutex<std::collections::HashMap<String, time::OffsetDateTime>>,
}

impl Activity {
    /// A record with nothing touched yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a session active now.
    pub async fn touch(&self, db: &Database, session_id: &str) -> Result<bool> {
        self.touch_at(db, session_id, time::OffsetDateTime::now_utc())
            .await
    }

    /// Mark a session active at a given instant, reporting whether the row was
    /// written. Takes the instant so the coalescing window is testable without
    /// waiting out a minute.
    pub async fn touch_at(
        &self,
        db: &Database,
        session_id: &str,
        now: time::OffsetDateTime,
    ) -> Result<bool> {
        if !self.due(session_id, now) {
            return Ok(false);
        }
        let stamp = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let _turn = super::write_turn().await?;
        let conn = super::connect(db)?;
        // The guard keeps the timestamp monotonic: a server clock that reads
        // behind an earlier touch must not move a session back out of the
        // active window.
        if let Err(err) = conn
            .execute(
                "UPDATE sessions SET last_activity = ?1 WHERE id = ?2 AND last_activity < ?1",
                vec![Value::Text(stamp), Value::Text(session_id.to_string())],
            )
            .await
            .map_err(engine)
        {
            // The window is recorded only for a write that landed, so a failed
            // write is retried on the next call rather than skipped for a
            // minute. The record is a write-rate hint, so a rollback costs one
            // update and never correctness.
            self.roll_back(session_id);
            return Err(err);
        }
        self.record(session_id, now);
        Ok(true)
    }

    /// Whether this session's next write is due, without recording it.
    fn due(&self, session_id: &str, now: time::OffsetDateTime) -> bool {
        let last = match self.last.lock() {
            Ok(last) => last,
            // A poisoned lock means a panic while holding it. The record is a
            // write-rate hint, so losing it costs one extra update.
            Err(poisoned) => poisoned.into_inner(),
        };
        !last
            .get(session_id)
            .is_some_and(|at| (now - *at).whole_seconds() < ACTIVITY_COALESCE_SECS)
    }

    /// Record a served window for a session whose write landed.
    fn record(&self, session_id: &str, now: time::OffsetDateTime) {
        let mut last = self.lock_last();
        // Runs at most once per session per window, so the sweep of sessions
        // that have gone quiet costs nothing on the hot path.
        last.retain(|_, at| (now - *at).whole_seconds() < ACTIVITY_COALESCE_SECS * 2);
        last.insert(session_id.to_string(), now);
    }

    /// Drop a recorded window after its write failed, so the next call retries.
    fn roll_back(&self, session_id: &str) {
        let mut last = self.lock_last();
        last.remove(session_id);
    }

    /// The activity map under its lock, recovering a poisoned one.
    fn lock_last(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, time::OffsetDateTime>> {
        match self.last.lock() {
            Ok(last) => last,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// How many agents are working, fleet-wide or in one project.
///
/// An agent is active when it owns a live session that was touched inside the
/// window, so an agent with two sessions counts once and an agent whose token
/// was used but which holds no session does not count at all.
pub async fn agents_active(db: &Database, since: &str, project_id: Option<&str>) -> Result<i64> {
    let mut sql = String::from(
        "SELECT COUNT(DISTINCT agent) FROM sessions
         WHERE status = 'active' AND deleted_at IS NULL AND last_activity >= ?1",
    );
    let mut params = vec![Value::Text(since.to_string())];
    if let Some(project_id) = project_id {
        params.push(Value::Text(project_id.to_string()));
        sql.push_str(" AND project_id = ?2");
    }
    let conn = super::connect(db)?;
    crate::store::events::count_on(&conn, &sql, params).await
}

/// Live agents per project, in one grouped query.
///
/// The per-project form of [`agents_active`], for a listing that would
/// otherwise call that once per row.
pub async fn agents_active_by_project(db: &Database, since: &str) -> Result<Vec<(String, i64)>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT project_id, COUNT(DISTINCT agent) FROM sessions
             WHERE status = 'active' AND deleted_at IS NULL AND last_activity >= ?1
             GROUP BY project_id",
            vec![Value::Text(since.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let id = match row.get_value(0).map_err(engine)? {
            Value::Text(text) => text,
            _ => continue,
        };
        let count = row
            .get_value(1)
            .map_err(engine)?
            .as_integer()
            .copied()
            .unwrap_or(0);
        out.push((id, count));
    }
    Ok(out)
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
/// Every ended, unpruned session, with the project it belongs to. Used at
/// startup to fold a finished brain's log into its file, so a session that
/// ended before that became automatic is consolidated too.
pub async fn ended_unpruned(db: &Database) -> Result<Vec<(String, String)>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT project_id, id FROM sessions
             WHERE status = 'ended' AND deleted_at IS NULL",
            (),
        )
        .await
        .map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let project_id = match row.get_value(0).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        let id = match row.get_value(1).map_err(engine)? {
            Value::Text(value) => value,
            _ => continue,
        };
        out.push((project_id, id));
    }
    Ok(out)
}

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

async fn assert_project_active(tx: &crate::store::WriteTx, project_id: &str) -> Result<()> {
    let mut p_rows = tx
        .query(
            "SELECT status FROM projects WHERE id = ?1",
            vec![Value::Text(project_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let p_row = p_rows
        .next()
        .await
        .map_err(engine)?
        .ok_or_else(|| Error::NotFound(format!("project {project_id} not found")))?;
    let status: String = match p_row.get_value(0).map_err(engine)? {
        Value::Text(s) => s,
        _ => "active".to_string(),
    };
    if status != "active" {
        return Err(Error::NotFound(format!("project {project_id} not found")));
    }
    Ok(())
}

// Shared sessions: participants, names, and the write-order entry points.
// The MCP tools and the REST handlers call these; nothing outside the store
// writes `sessions` or `session_participants`.

/// One participant of a session: a member's agent that wrote or read in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Participant {
    pub member: String,
    /// The canonical agent slug; `''` for a participant migrated without one.
    pub agent: String,
    pub joined_at: String,
    pub last_seen_at: String,
    /// Set by `session_leave`; cleared by the participant's next call.
    pub left_at: Option<String>,
}

/// What a start found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    /// A new session.
    Created,
    /// A live session of that name, joined.
    Resumed,
    /// An ended session of that name, reopened.
    Reopened,
}

/// Whether a join inserted the caller's participant row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Joined {
    /// True when this call made the caller a participant, which also seeded
    /// its notify cursor at the session floor.
    pub first: bool,
}

/// A `session_start` request.
#[derive(Debug, Clone)]
pub struct StartRequest<'a> {
    pub project_id: &'a str,
    pub session_name: &'a str,
    /// Who starts it: the `started_*` columns for a new session, and the
    /// participant that joins. A human writer starts without joining.
    pub writer: &'a crate::author::Writer,
    /// Refuse any live or ended session of that name with
    /// [`SessionReason::NameTaken`](crate::error::SessionReason::NameTaken).
    pub exclusive: bool,
    /// Fork: copy this session's brain into the new session. The caller has
    /// resolved it already. Forking onto an existing name is refused.
    pub fork_from: Option<&'a str>,
}

/// What a start did.
#[derive(Debug, Clone)]
pub struct Started {
    pub session: Session,
    pub outcome: StartOutcome,
    pub joined: Joined,
}

/// The longest segment of a new session name.
pub const NAME_SEGMENT_MAX: usize = 64;

/// The longest new session name, separators included.
pub const NAME_MAX: usize = 128;

/// The immediate parent of a child name: `feat-x` for `feat-x/review`.
pub fn parent_name(name: &str) -> Option<&str> {
    name.rsplit_once('/').map(|(parent, _)| parent)
}

/// Check a name a call is about to create. Migrated names are never checked.
///
/// Each `/`-separated segment is 1 to 64 of `[A-Za-z0-9_-]`, the whole name is
/// at most 128, and no segment is a UUID (32 hex, dashes optional in
/// 8-4-4-4-12) or a ULID (26 Crockford base32). Whether each parent is live is
/// the store's check, not this one.
pub fn check_new_name(name: &str) -> Result<()> {
    use crate::error::SessionReason;
    let shape = || {
        Error::Session(
            SessionReason::Invalid,
            format!(
                "session name '{name}' must be segments of 1 to {NAME_SEGMENT_MAX} letters, digits, \
                 '-' or '_', joined by '/', at most {NAME_MAX} in all"
            ),
            None,
        )
    };
    if name.is_empty() || name.len() > NAME_MAX {
        return Err(shape());
    }
    for segment in name.split('/') {
        let valid = !segment.is_empty()
            && segment.len() <= NAME_SEGMENT_MAX
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(shape());
        }
        if is_opaque(segment) {
            return Err(Error::Session(
                SessionReason::NameOpaque,
                "session names are for people: use a branch, worktree or topic".to_string(),
                None,
            ));
        }
    }
    Ok(())
}

fn is_opaque(segment: &str) -> bool {
    let hex = |s: &str| s.chars().all(|c| c.is_ascii_hexdigit());
    let plain_uuid = segment.len() == 32 && hex(segment);
    let dashed_uuid = segment.len() == 36 && {
        let parts: Vec<&str> = segment.split('-').collect();
        parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| hex(p))
    };
    let ulid = segment.len() == 26
        && segment.chars().all(|c| {
            let c = c.to_ascii_uppercase();
            c.is_ascii_digit() || (c.is_ascii_uppercase() && !matches!(c, 'I' | 'L' | 'O' | 'U'))
        })
        && segment.starts_with(|c: char| ('0'..='7').contains(&c));
    plain_uuid || dashed_uuid || ulid
}

/// A session by id, directly or through `session_aliases`, live, ended or
/// tombstoned.
pub async fn lookup_id(db: &Database, session_id: &str) -> Result<Option<Session>> {
    let _ = (db, session_id);
    Err(super::unbuilt("sessions::lookup_id"))
}

/// The session holding `name` in a project: the live or ended one if any,
/// else the newest tombstoned one.
pub async fn lookup_name(db: &Database, project_id: &str, name: &str) -> Result<Option<Session>> {
    let _ = (db, project_id, name);
    Err(super::unbuilt("sessions::lookup_name"))
}

/// Live sessions named `name` in any project that the participant is in, for
/// a bare name with no project on a read.
pub async fn live_named_for(
    db: &Database,
    key: &crate::author::ParticipantKey,
    name: &str,
) -> Result<Vec<Session>> {
    let _ = (db, key, name);
    Err(super::unbuilt("sessions::live_named_for"))
}

/// `session_start`: find, reopen or create, then join.
pub async fn start_session(db: &Database, request: &StartRequest<'_>) -> Result<Started> {
    let _ = (db, request);
    Err(super::unbuilt("sessions::start_session"))
}

/// The session a write lands in: the live session of that name in the
/// project, auto-created when absent (the name is checked as new), joined by
/// the writer's participant in the same transaction. Ended answers
/// `session_ended`, tombstoned answers `session_pruned`.
pub async fn open_for_write(
    db: &Database,
    project_id: &str,
    name: &str,
    writer: &crate::author::Writer,
) -> Result<(Session, Joined)> {
    let _ = (db, project_id, name, writer);
    Err(super::unbuilt("sessions::open_for_write"))
}

/// Upsert the participant: insert seeds its notify cursor at the session
/// floor; update refreshes `last_seen_at` (coalesced) and clears `left_at`.
/// Only a live session is joined.
pub async fn join(
    db: &Database,
    session_id: &str,
    key: &crate::author::ParticipantKey,
) -> Result<Joined> {
    let _ = (db, session_id, key);
    Err(super::unbuilt("sessions::join"))
}

/// `session_leave`: set the caller's `left_at`. Ends nothing.
pub async fn leave(
    db: &Database,
    session_id: &str,
    key: &crate::author::ParticipantKey,
) -> Result<()> {
    let _ = (db, session_id, key);
    Err(super::unbuilt("sessions::leave"))
}

/// The session and its live descendants, parent first: the gates an end or a
/// prune takes, in this set.
pub async fn family(db: &Database, session_id: &str) -> Result<Vec<String>> {
    let _ = (db, session_id);
    Err(super::unbuilt("sessions::family"))
}

/// End a session for everyone, with its live children, one `ended` event
/// each authored by `writer`. The caller holds the exclusive gate of every id
/// [`family`] returned. Returns the ids it ended.
pub async fn end_session(
    db: &Database,
    _gates: &[tokio::sync::OwnedRwLockWriteGuard<()>],
    session_id: &str,
    writer: &crate::author::Writer,
    handoff: Option<&str>,
) -> Result<Vec<String>> {
    let _ = (db, session_id, writer, handoff);
    Err(super::unbuilt("sessions::end_session"))
}

/// Every participant of a session, newest `last_seen_at` first.
pub async fn participants(db: &Database, session_id: &str) -> Result<Vec<Participant>> {
    let _ = (db, session_id);
    Err(super::unbuilt("sessions::participants"))
}

/// How many sessions are live (active, with a participant seen inside the
/// activity window and not left), in one project or across the hub.
pub async fn live_count(db: &Database, since: &str, project_id: Option<&str>) -> Result<i64> {
    let _ = (db, since, project_id);
    Err(super::unbuilt("sessions::live_count"))
}

/// The filters `GET /api/v1/sessions` and `session_list` take.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter<'a> {
    pub project_id: Option<&'a str>,
    pub status: Option<&'a str>,
    pub member: Option<&'a str>,
    /// A canonical slug.
    pub agent: Option<&'a str>,
    /// Only live sessions (`live=1`).
    pub live: bool,
    /// Only children of this session id.
    pub parent: Option<&'a str>,
    /// The activity window `live` counts in.
    pub since: Option<&'a str>,
    pub limit: Option<i64>,
}

/// A session row with what a listing shows beside it.
#[derive(Debug, Clone, Serialize)]
pub struct ListedSession {
    #[serde(flatten)]
    pub session: Session,
    pub participants: Vec<Participant>,
    pub children_count: i64,
    pub started_by: crate::author::AuthorView,
}

/// List sessions with their participants.
pub async fn list_filtered(
    db: &Database,
    filter: &SessionFilter<'_>,
) -> Result<Vec<ListedSession>> {
    let _ = (db, filter);
    Err(super::unbuilt("sessions::list_filtered"))
}

#[cfg(test)]
mod name_tests {
    use super::*;
    use crate::error::SessionReason;

    fn reason(name: &str) -> Option<SessionReason> {
        match check_new_name(name) {
            Ok(()) => None,
            Err(Error::Session(reason, _, _)) => Some(reason),
            Err(other) => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn people_names_and_children_pass() {
        assert_eq!(reason("feat-redesign"), None);
        assert_eq!(reason("feat-x/review-1a2b3c4d"), None);
        assert_eq!(reason(&"a".repeat(64)), None);
        assert_eq!(parent_name("feat-x/review"), Some("feat-x"));
        assert_eq!(parent_name("feat-x"), None);
    }

    #[test]
    fn bad_shapes_and_opaque_ids_are_refused() {
        assert_eq!(reason(""), Some(SessionReason::Invalid));
        assert_eq!(reason("a b"), Some(SessionReason::Invalid));
        assert_eq!(reason("feat-x/"), Some(SessionReason::Invalid));
        assert_eq!(reason(&"a".repeat(65)), Some(SessionReason::Invalid));
        assert_eq!(
            reason(&format!("{}/{}", "a".repeat(64), "b".repeat(64))),
            Some(SessionReason::Invalid)
        );
        assert_eq!(
            reason("0f8e5c2a-1b3d-4e5f-8a9b-0c1d2e3f4a5b"),
            Some(SessionReason::NameOpaque)
        );
        assert_eq!(
            reason("0f8e5c2a1b3d4e5f8a9b0c1d2e3f4a5b"),
            Some(SessionReason::NameOpaque)
        );
        assert_eq!(
            reason("feat/01JD8X7Q2ZC5M3N4P6R8S9T0VW"),
            Some(SessionReason::NameOpaque)
        );
    }
}
