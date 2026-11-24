//! Sessions: the durable mapping from a session name in a project to a brain
//! file, its participants, and its lifecycle.
//!
//! A session belongs to its project, not to whoever started it: one live name
//! per project, and any participant writes. State itself lives in the brain
//! file; this table records identity, status, and the timestamps retention
//! layers will need.

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::author::{Author, AuthorView, ParticipantKey, Writer};
use crate::error::{Error, Result, SessionReason};
use crate::store::events::{self, NewEvent};

/// A session as recorded in the hub store.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub session_name: String,
    pub status: String,
    pub brain_path: String,
    pub created_at: String,
    pub last_activity: String,
    pub deleted_at: Option<String>,
    /// The session this one was forked from, if any.
    pub forked_from: Option<String>,
    /// History only: the session this one was adopted from before sessions
    /// were shared.
    pub adopted_from: Option<String>,
    /// The note left when the session ended.
    pub handoff: Option<String>,
    /// The parent of a `<parent>/<task>` child.
    pub parent_id: Option<String>,
    /// The write id of a file-backed write between its file and its index.
    pub index_pending: Option<String>,
    /// Who started it, as a label: the member, `human`, `local`, or the old
    /// owner label of a migrated session.
    pub started_label: String,
    /// Who started it. A listing serialises it beside the session.
    #[serde(skip)]
    pub started_by: AuthorView,
}

/// Every session column, in the order [`session_from_row`] reads them.
const COLUMNS: &str = "id, project_id, session_name, COALESCE(started_label, ''), status, \
                       brain_path, created_at, last_activity, deleted_at, forked_from, \
                       adopted_from, handoff, parent_id, index_pending, started_kind, \
                       started_member, started_agent";

fn now() -> String {
    crate::store::now_rfc3339()
}

fn t(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn ot(value: Option<&str>) -> Value {
    value.map_or(Value::Null, t)
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

/// Live top-level sessions per project, in one grouped query: the per-project
/// form of [`live_count`].
pub async fn live_by_project(db: &Database, since: &str) -> Result<Vec<(String, i64)>> {
    grouped_counts(
        db,
        &format!(
            "SELECT s.project_id, COUNT(DISTINCT s.id) FROM sessions s
              WHERE {LIVE} AND s.parent_id IS NULL GROUP BY s.project_id"
        ),
        since,
    )
    .await
}

/// Live subagent (child) sessions per project: the per-project form of
/// [`live_subagent_count`].
pub async fn live_subagents_by_project(db: &Database, since: &str) -> Result<Vec<(String, i64)>> {
    grouped_counts(
        db,
        &format!(
            "SELECT s.project_id, COUNT(DISTINCT s.id) FROM sessions s
              WHERE {LIVE} AND s.parent_id IS NOT NULL GROUP BY s.project_id"
        ),
        since,
    )
    .await
}

/// Live top-level sessions and live subagents each member takes part in, as
/// `(member, sessions, subagents)`. A member with none is absent.
pub async fn live_by_member(db: &Database, since: &str) -> Result<Vec<(String, i64, i64)>> {
    async fn count(db: &Database, since: &str, top: bool) -> Result<Vec<(String, i64)>> {
        let sql = format!(
            "SELECT p.member_id, COUNT(DISTINCT s.id) FROM sessions s
               JOIN session_participants p ON p.session_id = s.id
              WHERE s.status = 'active' AND s.deleted_at IS NULL
                AND p.left_at IS NULL AND p.last_seen_at >= ?1
                AND s.parent_id IS {} NULL GROUP BY p.member_id",
            if top { "" } else { "NOT" }
        );
        grouped_counts(db, &sql, since).await
    }
    let sessions = count(db, since, true).await?;
    let subagents = count(db, since, false).await?;
    let mut out: Vec<(String, i64, i64)> = sessions
        .into_iter()
        .map(|(member, n)| (member, n, 0))
        .collect();
    for (member, n) in subagents {
        match out.iter_mut().find(|row| row.0 == member) {
            Some(row) => row.2 = n,
            None => out.push((member, 0, n)),
        }
    }
    Ok(out)
}

async fn grouped_counts(db: &Database, sql: &str, since: &str) -> Result<Vec<(String, i64)>> {
    let conn = super::connect(db)?;
    let mut rows = conn.query(sql, vec![t(since)]).await.map_err(engine)?;
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

/// A live session, as a SQL condition over `s` with `?1` the activity window:
/// active, with a participant that has not left and was seen inside it.
const LIVE: &str = "s.status = 'active' AND s.deleted_at IS NULL AND EXISTS (
    SELECT 1 FROM session_participants p
     WHERE p.session_id = s.id AND p.left_at IS NULL AND p.last_seen_at >= ?1)";

/// Whether a session is live under the same rule as [`LIVE`]: active, with a
/// participant that has not left and was seen at or after `since`.
pub fn is_live(session: &Session, participants: &[Participant], since: &str) -> bool {
    session.status == "active"
        && session.deleted_at.is_none()
        && participants
            .iter()
            .any(|p| p.left_at.is_none() && p.last_seen_at.as_str() >= since)
}

/// Fetch one session by id.
pub async fn get(db: &Database, session_id: &str) -> Result<Option<Session>> {
    let conn = super::connect(db)?;
    get_on(&conn, session_id).await
}

async fn get_on(conn: &turso::Connection, session_id: &str) -> Result<Option<Session>> {
    one(conn, "WHERE id = ?1", vec![t(session_id)]).await
}

/// Every ended, unpruned session, with the project it belongs to. Used at
/// startup to fold a finished brain's log into its file.
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

/// List a project's sessions, most recently active first.
pub async fn list(db: &Database, project_id: &str) -> Result<Vec<Session>> {
    many(
        db,
        "WHERE project_id = ?1 AND deleted_at IS NULL ORDER BY last_activity DESC",
        vec![t(project_id)],
    )
    .await
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
    /// The label it was started under.
    pub agent: Option<&'a str>,
    /// The projects the caller may reach at all, or `None` for every project.
    pub visible: Option<&'a [String]>,
    /// Rows to return, clamped to the listing cap.
    pub limit: i64,
}

fn check_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status
        && status != "active"
        && status != "ended"
    {
        return Err(Error::InvalidArgument(format!(
            "status '{status}' must be 'active' or 'ended'"
        )));
    }
    Ok(())
}

fn in_list(sql: &mut String, params: &mut Vec<Value>, column: &str, ids: &[String]) {
    let mut placeholders = Vec::with_capacity(ids.len());
    for id in ids {
        params.push(t(id));
        placeholders.push(format!("?{}", params.len()));
    }
    sql.push_str(&format!(" AND {column} IN ({})", placeholders.join(", ")));
}

/// List sessions a caller may see, most recently active first.
///
/// Soft-deleted sessions are omitted: a pruned session is gone from a reader's
/// point of view, undo window or not.
pub async fn query(db: &Database, query: &SessionQuery<'_>) -> Result<Vec<Session>> {
    check_status(query.status)?;
    if query.visible.is_some_and(<[String]>::is_empty) {
        return Ok(Vec::new());
    }
    let mut sql = String::from("WHERE deleted_at IS NULL");
    let mut params: Vec<Value> = Vec::new();
    if let Some(project_id) = query.project_id {
        params.push(t(project_id));
        sql.push_str(&format!(" AND project_id = ?{}", params.len()));
    }
    if let Some(status) = query.status {
        params.push(t(status));
        sql.push_str(&format!(" AND status = ?{}", params.len()));
    }
    if let Some(agent) = query.agent {
        params.push(t(agent));
        sql.push_str(&format!(" AND started_label = ?{}", params.len()));
    }
    if let Some(visible) = query.visible {
        in_list(&mut sql, &mut params, "project_id", visible);
    }
    params.push(Value::Integer(
        query.limit.clamp(1, crate::limits::SESSION_LIST_LIMIT_MAX),
    ));
    sql.push_str(&format!(
        " ORDER BY last_activity DESC, id DESC LIMIT ?{}",
        params.len()
    ));
    many(db, &sql, params).await
}

/// The session that holds a name in a project and is not pruned.
async fn live_or_ended_on(
    conn: &turso::Connection,
    project_id: &str,
    session_name: &str,
) -> Result<Option<Session>> {
    one(
        conn,
        "WHERE project_id = ?1 AND session_name = ?2 AND deleted_at IS NULL",
        vec![t(project_id), t(session_name)],
    )
    .await
}

/// The newest pruned session that held a name, tombstone and all.
async fn pruned_on(
    conn: &turso::Connection,
    project_id: &str,
    session_name: &str,
) -> Result<Option<Session>> {
    one(
        conn,
        "WHERE project_id = ?1 AND session_name = ?2 AND deleted_at IS NOT NULL
         ORDER BY deleted_at DESC",
        vec![t(project_id), t(session_name)],
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

async fn many(db: &Database, clause: &str, params: Vec<Value>) -> Result<Vec<Session>> {
    let conn = super::connect(db)?;
    many_on(&conn, clause, params).await
}

async fn many_on(
    conn: &turso::Connection,
    clause: &str,
    params: Vec<Value>,
) -> Result<Vec<Session>> {
    let mut rows = conn
        .query(&format!("SELECT {COLUMNS} FROM sessions {clause}"), params)
        .await
        .map_err(engine)?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        sessions.push(session_from_row(&row)?);
    }
    Ok(sessions)
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
    let label = text(3)?;
    let author = Author::from_columns(
        optional(14)?.as_deref(),
        optional(15)?.as_deref(),
        optional(16)?.as_deref(),
        None,
        &label,
    );
    Ok(Session {
        id: text(0)?,
        project_id: text(1)?,
        session_name: text(2)?,
        status: text(4)?,
        brain_path: text(5)?,
        created_at: text(6)?,
        last_activity: text(7)?,
        deleted_at: optional(8)?,
        forked_from: optional(9)?,
        adopted_from: optional(10)?,
        handoff: optional(11)?,
        parent_id: optional(12)?,
        index_pending: optional(13)?,
        started_label: label,
        started_by: author.view(None),
    })
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}

async fn assert_project_active(tx: &crate::store::WriteTx, project_id: &str) -> Result<()> {
    let mut rows = tx
        .query(
            "SELECT status FROM projects WHERE id = ?1",
            vec![t(project_id)],
        )
        .await
        .map_err(engine)?;
    let row = rows
        .next()
        .await
        .map_err(engine)?
        .ok_or_else(|| Error::NotFound(format!("project {project_id} not found")))?;
    let status: String = match row.get_value(0).map_err(engine)? {
        Value::Text(s) => s,
        _ => "active".to_string(),
    };
    if status != "active" {
        return Err(Error::NotFound(format!("project {project_id} not found")));
    }
    Ok(())
}

fn name_taken(name: &str, session_id: &str) -> Error {
    Error::Session(
        SessionReason::NameTaken,
        format!("session name '{name}' is already in use existing_session_id={session_id}"),
        Some(serde_json::Map::from_iter([(
            "session_id".to_string(),
            serde_json::Value::from(session_id),
        )])),
    )
}

fn pruned(name: &str, session_id: &str) -> Error {
    Error::Session(
        SessionReason::Pruned,
        format!(
            "session '{name}' was pruned and can be restored with undo, or started under another name pruned_session_id={session_id}"
        ),
        Some(serde_json::Map::from_iter([(
            "session_id".to_string(),
            serde_json::Value::from(session_id),
        )])),
    )
}

fn ended(session: &Session) -> Error {
    Error::Session(
        SessionReason::Ended,
        format!(
            "session '{}' has ended; call session_start to reopen it",
            session.session_name
        ),
        Some(serde_json::Map::from_iter([(
            "session_id".to_string(),
            serde_json::Value::from(session.id.clone()),
        )])),
    )
}

/// Refuse a write to a session that is gone, pruned or ended, inside the
/// caller's transaction. The shared brain write and every artifact write
/// that names a session re-check this under the session gate.
pub(crate) async fn writable_in_tx(conn: &turso::Connection, session_id: &str) -> Result<Session> {
    let session = get_on(conn, session_id).await?.ok_or_else(|| {
        Error::Session(
            SessionReason::Unknown,
            format!("session {session_id} not found"),
            None,
        )
    })?;
    if session.deleted_at.is_some() {
        return Err(pruned(&session.session_name, &session.id));
    }
    if session.status != "active" {
        return Err(ended(&session));
    }
    Ok(session)
}

/// Insert a new session row from its writer.
async fn insert_session(
    tx: &crate::store::WriteTx,
    id: &str,
    project_id: &str,
    session_name: &str,
    parent_id: Option<&str>,
    writer: &Writer,
    forked_from: Option<&str>,
) -> Result<Session> {
    let at = now();
    let path = brain_path(project_id, id);
    let author = &writer.author;
    tx.execute(
        "INSERT INTO sessions(id, project_id, session_name, parent_id, status, brain_path,
            created_at, last_activity, forked_from, started_kind, started_member, started_agent,
            started_label)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?6, ?7, ?8, ?9, ?10, ?11)",
        vec![
            t(id),
            t(project_id),
            t(session_name),
            ot(parent_id),
            t(&path),
            t(&at),
            ot(forked_from),
            t(author.kind().as_str()),
            ot(author.member()),
            ot(author.agent()),
            t(writer.actor()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(Session {
        id: id.to_string(),
        project_id: project_id.to_string(),
        session_name: session_name.to_string(),
        status: "active".to_string(),
        brain_path: path,
        created_at: at.clone(),
        last_activity: at,
        deleted_at: None,
        forked_from: forked_from.map(str::to_string),
        adopted_from: None,
        handoff: None,
        parent_id: parent_id.map(str::to_string),
        index_pending: None,
        started_label: writer.actor().to_string(),
        started_by: author.clone().in_session(None).view(None),
    })
}

async fn copy_search_rows(tx: &crate::store::WriteTx, from: &str, to: &str) -> Result<()> {
    // The copy holds the source's entries, so it holds the source's search
    // rows too; `ref_id` is already the brain path, so only the keys change.
    tx.execute(
        "INSERT INTO search_docs(doc_id, project_id, type, ref_id, session_id, title, body, updated_at)
         SELECT 'brain:' || ?1 || ':' || ref_id, project_id, 'brain', ref_id, ?1, title, body, updated_at
           FROM search_docs WHERE type = 'brain' AND session_id = ?2",
        vec![t(to), t(from)],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

async fn append_lifecycle(
    tx: &crate::store::WriteTx,
    writer: &Writer,
    session: &Session,
    summary: String,
    payload: serde_json::Value,
) -> Result<String> {
    let writer = Writer {
        author: writer.author.clone().in_session(Some(session.id.clone())),
        origin: writer.origin,
    };
    events::append_in_tx(
        tx,
        0,
        &writer,
        None,
        NewEvent {
            project_id: session.project_id.clone(),
            kind: "session".to_string(),
            summary,
            payload: Some(payload),
            needs_action: false,
            thread_id: None,
            session_id: Some(session.id.clone()),
        },
    )
    .await
}

/// The parent a new child name hangs under: every `/`-prefix must be a live
/// session of the project. Returns the immediate parent's id.
async fn parent_for(
    conn: &turso::Connection,
    project_id: &str,
    session_name: &str,
) -> Result<Option<String>> {
    let Some(parent) = parent_name(session_name) else {
        return Ok(None);
    };
    let mut prefix = String::new();
    let mut immediate = None;
    for segment in parent.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        match live_or_ended_on(conn, project_id, &prefix).await? {
            Some(held) if held.status == "active" => immediate = Some(held.id),
            _ => {
                return Err(Error::Session(
                    SessionReason::ParentMissing,
                    format!(
                        "no live session '{prefix}' to hold '{session_name}'; use '{}'",
                        session_name.replace('/', "-")
                    ),
                    None,
                ));
            }
        }
    }
    Ok(immediate)
}

/// Find the not-pruned session of a name, or create it: refused when the
/// only holder is pruned. Runs in the caller's transaction.
async fn find_or_create_in_tx(
    tx: &crate::store::WriteTx,
    project_id: &str,
    session_name: &str,
    writer: &Writer,
) -> Result<(Session, bool)> {
    if let Some(existing) = live_or_ended_on(tx, project_id, session_name).await? {
        return Ok((existing, false));
    }
    if let Some(held) = pruned_on(tx, project_id, session_name).await? {
        return Err(pruned(session_name, &held.id));
    }
    check_new_name(session_name)?;
    let parent = parent_for(tx, project_id, session_name).await?;
    let id = crate::store::next_id();
    let session = insert_session(
        tx,
        &id,
        project_id,
        session_name,
        parent.as_deref(),
        writer,
        None,
    )
    .await?;
    append_lifecycle(
        tx,
        writer,
        &session,
        format!("session {session_name} started"),
        serde_json::json!({
            "action": "started",
            "session_id": session.id,
            "session_name": session.session_name,
        }),
    )
    .await?;
    Ok((session, true))
}

/// Join a writer's participant, if it has one, inside the caller's
/// transaction.
async fn join_writer_in_tx(
    tx: &crate::store::WriteTx,
    session_id: &str,
    writer: &Writer,
) -> Result<Joined> {
    match writer.author.participant() {
        Some(key) => join_in_tx(tx, session_id, &key).await,
        None => Ok(Joined { first: false }),
    }
}

/// Upsert a participant row; a first join seeds the notify cursor at the
/// session floor in the same transaction.
pub(crate) async fn join_in_tx(
    tx: &crate::store::WriteTx,
    session_id: &str,
    key: &ParticipantKey,
) -> Result<Joined> {
    let at = time::OffsetDateTime::now_utc();
    let stamp = at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let mut rows = tx
        .query(
            "SELECT last_seen_at, left_at FROM session_participants
              WHERE session_id = ?1 AND member_id = ?2 AND agent = ?3",
            vec![t(session_id), t(&key.member), t(&key.agent)],
        )
        .await
        .map_err(engine)?;
    if let Some(row) = rows.next().await.map_err(engine)? {
        let seen = match row.get_value(0).map_err(engine)? {
            Value::Text(seen) => seen,
            _ => String::new(),
        };
        let left = !matches!(row.get_value(1).map_err(engine)?, Value::Null);
        drop(rows);
        let cutoff = (at - time::Duration::seconds(ACTIVITY_COALESCE_SECS))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        if left || seen < cutoff {
            tx.execute(
                "UPDATE session_participants SET last_seen_at = ?1, left_at = NULL
                  WHERE session_id = ?2 AND member_id = ?3 AND agent = ?4",
                vec![t(&stamp), t(session_id), t(&key.member), t(&key.agent)],
            )
            .await
            .map_err(engine)?;
        }
        return Ok(Joined { first: false });
    }
    drop(rows);
    let floor = session_floor(tx, session_id).await?;
    tx.execute(
        "INSERT INTO session_participants(session_id, member_id, agent, joined_at, last_seen_at, left_at)
         VALUES (?1, ?2, ?3, ?4, ?4, NULL)",
        vec![t(session_id), t(&key.member), t(&key.agent), t(&stamp)],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "INSERT OR IGNORE INTO notify_cursors(member_id, agent, session_id, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        vec![
            t(&key.member),
            t(&key.agent),
            t(session_id),
            t(&floor),
            t(&stamp),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(Joined { first: true })
}

/// The cursor a newcomer starts from in a session: the lowest of the other
/// participants' cursors there, and the id just before the oldest item asked
/// in the session that is still open. Empty when neither exists, which in a
/// session scope is the session's own items only, never the project backlog.
async fn session_floor(conn: &turso::Connection, session_id: &str) -> Result<String> {
    let mut floor: Option<String> = None;
    let mut lower = |candidate: Option<String>| {
        if let Some(candidate) = candidate
            && floor.as_ref().is_none_or(|held| candidate < *held)
        {
            floor = Some(candidate);
        }
    };
    let cursor = scalar(
        conn,
        "SELECT MIN(last_seen_event_id) FROM notify_cursors WHERE session_id = ?1",
        vec![t(session_id)],
    )
    .await?;
    lower(cursor);
    let open = scalar(
        conn,
        "SELECT MIN(e.id) FROM inbox i JOIN events e ON e.id = i.event_id
          WHERE e.session_id = ?1 AND i.status IN ('action', 'waiting')",
        vec![t(session_id)],
    )
    .await?;
    if let Some(open) = open {
        let before = scalar(
            conn,
            "SELECT MAX(id) FROM events WHERE id < ?1",
            vec![t(&open)],
        )
        .await?;
        lower(Some(before.unwrap_or_default()));
    }
    Ok(floor.unwrap_or_default())
}

async fn scalar(conn: &turso::Connection, sql: &str, params: Vec<Value>) -> Result<Option<String>> {
    let mut rows = conn.query(sql, params).await.map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(match row.get_value(0).map_err(engine)? {
            Value::Text(value) => Some(value),
            _ => None,
        }),
        None => Ok(None),
    }
}

/// The session and its not-pruned descendants, parent first.
async fn family_on(conn: &turso::Connection, session_id: &str) -> Result<Vec<String>> {
    let mut out = vec![session_id.to_string()];
    let mut next = 0;
    while next < out.len() {
        let parent = out[next].clone();
        next += 1;
        let mut rows = conn
            .query(
                "SELECT id FROM sessions WHERE parent_id = ?1 AND deleted_at IS NULL ORDER BY id",
                vec![t(&parent)],
            )
            .await
            .map_err(engine)?;
        while let Some(row) = rows.next().await.map_err(engine)? {
            if let Value::Text(id) = row.get_value(0).map_err(engine)?
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
    }
    Ok(out)
}

/// End a session and its live children, one `ended` event each.
async fn end_inner(
    db: &Database,
    session_id: &str,
    writer: &Writer,
    handoff: Option<&str>,
) -> Result<Vec<String>> {
    if let Some(handoff) = handoff {
        crate::limits::check_handoff(handoff)?;
    }
    let tx = super::begin_write(db).await?;
    let root = get_on(&tx, session_id)
        .await?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;
    let mut ended_ids = Vec::new();
    for id in family_on(&tx, &root.id).await? {
        let Some(session) = get_on(&tx, &id).await? else {
            continue;
        };
        if session.status != "active" || session.deleted_at.is_some() {
            continue;
        }
        let note = (id == root.id).then_some(handoff).flatten();
        let moved = tx
            .execute(
                "UPDATE sessions SET status = 'ended', last_activity = ?1,
                                     handoff = COALESCE(?2, handoff)
                 WHERE id = ?3 AND status = 'active' AND deleted_at IS NULL",
                vec![t(&now()), ot(note), t(&id)],
            )
            .await
            .map_err(engine)?;
        if moved == 0 {
            continue;
        }
        append_lifecycle(
            &tx,
            writer,
            &session,
            format!("session {} ended", session.session_name),
            serde_json::json!({
                "action": "ended",
                "session_id": session.id,
                "session_name": session.session_name,
                "handoff": note,
            }),
        )
        .await?;
        // An ended session has no one left to deliver to, so the
        // subscriptions made for it go in the same commit.
        super::subscriptions::forget_session_in_tx(&tx, &id).await?;
        ended_ids.push(id);
    }
    tx.commit().await.map_err(engine)?;
    Ok(ended_ids)
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
    let conn = super::connect(db)?;
    lookup_id_on(&conn, session_id).await
}

pub(crate) async fn lookup_id_on(
    conn: &turso::Connection,
    session_id: &str,
) -> Result<Option<Session>> {
    if let Some(session) = get_on(conn, session_id).await? {
        return Ok(Some(session));
    }
    let Some(target) = scalar(
        conn,
        "SELECT session_id FROM session_aliases WHERE alias_id = ?1",
        vec![t(session_id)],
    )
    .await?
    else {
        return Ok(None);
    };
    get_on(conn, &target).await
}

/// The session holding `name` in a project: the live or ended one if any,
/// else the newest tombstoned one.
pub async fn lookup_name(db: &Database, project_id: &str, name: &str) -> Result<Option<Session>> {
    let conn = super::connect(db)?;
    if let Some(held) = live_or_ended_on(&conn, project_id, name).await? {
        return Ok(Some(held));
    }
    pruned_on(&conn, project_id, name).await
}

/// Live sessions named `name` in any project that the participant is in, for
/// a bare name with no project on a read.
pub async fn live_named_for(
    db: &Database,
    key: &crate::author::ParticipantKey,
    name: &str,
) -> Result<Vec<Session>> {
    many(
        db,
        "WHERE session_name = ?1 AND status = 'active' AND deleted_at IS NULL
           AND EXISTS (SELECT 1 FROM session_participants p
                        WHERE p.session_id = sessions.id AND p.member_id = ?2
                          AND (p.agent = ?3 OR p.agent = ''))
         ORDER BY last_activity DESC",
        vec![t(name), t(&key.member), t(&key.agent)],
    )
    .await
}

/// `session_start`: find, reopen or create, then join.
pub async fn start_session(db: &Database, request: &StartRequest<'_>) -> Result<Started> {
    let writer = request.writer;
    let tx = super::begin_write(db).await?;
    assert_project_active(&tx, request.project_id).await?;

    if let Some(source_id) = request.fork_from {
        let source = lookup_id_on(&tx, source_id)
            .await?
            .filter(|source| source.deleted_at.is_none())
            .ok_or_else(|| Error::NotFound(format!("session {source_id} not found")))?;
        if let Some(existing) =
            live_or_ended_on(&tx, request.project_id, request.session_name).await?
        {
            return Err(name_taken(request.session_name, &existing.id));
        }
        check_new_name(request.session_name)?;
        let parent = parent_for(&tx, request.project_id, request.session_name).await?;
        let id = crate::store::next_id();
        let session = insert_session(
            &tx,
            &id,
            request.project_id,
            request.session_name,
            parent.as_deref(),
            writer,
            Some(&source.id),
        )
        .await?;
        copy_search_rows(&tx, &source.id, &id).await?;
        append_lifecycle(
            &tx,
            writer,
            &session,
            format!(
                "session {} forked from {}",
                request.session_name, source.session_name
            ),
            serde_json::json!({
                "action": "forked",
                "session_id": id,
                "session_name": request.session_name,
                "from_session_id": source.id,
                "from_agent": source.started_label,
            }),
        )
        .await?;
        let joined = join_writer_in_tx(&tx, &id, writer).await?;
        tx.commit().await.map_err(engine)?;
        return Ok(Started {
            session,
            outcome: StartOutcome::Created,
            joined,
        });
    }

    if request.exclusive
        && let Some(existing) =
            live_or_ended_on(&tx, request.project_id, request.session_name).await?
    {
        return Err(name_taken(request.session_name, &existing.id));
    }
    let (mut session, created) =
        find_or_create_in_tx(&tx, request.project_id, request.session_name, writer).await?;
    let outcome = if created {
        StartOutcome::Created
    } else if session.status == "active" {
        let at = now();
        tx.execute(
            "UPDATE sessions SET last_activity = ?1 WHERE id = ?2 AND last_activity < ?1",
            vec![t(&at), t(&session.id)],
        )
        .await
        .map_err(engine)?;
        StartOutcome::Resumed
    } else {
        if session.parent_id.is_some() {
            parent_for(&tx, request.project_id, request.session_name).await?;
        }
        let at = now();
        tx.execute(
            "UPDATE sessions SET status = 'active', last_activity = ?1 WHERE id = ?2",
            vec![t(&at), t(&session.id)],
        )
        .await
        .map_err(engine)?;
        session.status = "active".to_string();
        session.last_activity = at;
        StartOutcome::Reopened
    };
    let joined = join_writer_in_tx(&tx, &session.id, writer).await?;
    tx.commit().await.map_err(engine)?;
    Ok(Started {
        session,
        outcome,
        joined,
    })
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
    let tx = super::begin_write(db).await?;
    assert_project_active(&tx, project_id).await?;
    let (session, _) = find_or_create_in_tx(&tx, project_id, name, writer).await?;
    if session.status != "active" {
        return Err(ended(&session));
    }
    let joined = join_writer_in_tx(&tx, &session.id, writer).await?;
    tx.commit().await.map_err(engine)?;
    Ok((session, joined))
}

/// Upsert the participant: insert seeds its notify cursor at the session
/// floor; update refreshes `last_seen_at` (coalesced) and clears `left_at`.
/// Only a live session is joined.
pub async fn join(
    db: &Database,
    session_id: &str,
    key: &crate::author::ParticipantKey,
) -> Result<Joined> {
    let tx = super::begin_write(db).await?;
    writable_in_tx(&tx, session_id).await?;
    let joined = join_in_tx(&tx, session_id, key).await?;
    tx.commit().await.map_err(engine)?;
    Ok(joined)
}

/// `session_leave`: set the caller's `left_at`. Ends nothing.
pub async fn leave(
    db: &Database,
    session_id: &str,
    key: &crate::author::ParticipantKey,
) -> Result<()> {
    let tx = super::begin_write(db).await?;
    if get_on(&tx, session_id).await?.is_none() {
        return Err(Error::Session(
            SessionReason::Unknown,
            format!("session {session_id} not found"),
            None,
        ));
    }
    tx.execute(
        "UPDATE session_participants SET left_at = ?1
          WHERE session_id = ?2 AND member_id = ?3 AND agent = ?4",
        vec![t(&now()), t(session_id), t(&key.member), t(&key.agent)],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;
    Ok(())
}

/// The session and its live descendants, parent first: the gates an end or a
/// prune takes, in this set.
pub async fn family(db: &Database, session_id: &str) -> Result<Vec<String>> {
    let conn = super::connect(db)?;
    family_on(&conn, session_id).await
}

/// End a session for everyone, with its live children, one `ended` event
/// each authored by `writer`. The caller holds the exclusive gate of every id
/// [`family`] returned. Returns the ids it ended.
///
/// Ending is not owner-gated: a session has no owner. The store trusts its
/// caller, which must have authorized project access first (an active member
/// with access to the session's project, or the human admin); ending a parent
/// ends its children under that same authorization.
pub async fn end_session(
    db: &Database,
    _gates: &[tokio::sync::OwnedRwLockWriteGuard<()>],
    session_id: &str,
    writer: &crate::author::Writer,
    handoff: Option<&str>,
) -> Result<Vec<String>> {
    end_inner(db, session_id, writer, handoff).await
}

/// Every participant of a session, newest `last_seen_at` first.
pub async fn participants(db: &Database, session_id: &str) -> Result<Vec<Participant>> {
    let conn = super::connect(db)?;
    participants_on(&conn, session_id).await
}

async fn participants_on(conn: &turso::Connection, session_id: &str) -> Result<Vec<Participant>> {
    let mut rows = conn
        .query(
            "SELECT member_id, agent, joined_at, last_seen_at, left_at FROM session_participants
              WHERE session_id = ?1 ORDER BY last_seen_at DESC, member_id, agent",
            vec![t(session_id)],
        )
        .await
        .map_err(engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        let text = |index: usize| -> Result<Option<String>> {
            Ok(match row.get_value(index).map_err(engine)? {
                Value::Text(value) => Some(value),
                _ => None,
            })
        };
        out.push(Participant {
            member: text(0)?.unwrap_or_default(),
            agent: text(1)?.unwrap_or_default(),
            joined_at: text(2)?.unwrap_or_default(),
            last_seen_at: text(3)?.unwrap_or_default(),
            left_at: text(4)?,
        });
    }
    Ok(out)
}

/// How many top-level sessions are live (active, with a participant seen
/// inside the activity window and not left), in one project or across the
/// hub. Subagents are counted by [`live_subagent_count`].
pub async fn live_count(db: &Database, since: &str, project_id: Option<&str>) -> Result<i64> {
    live_kind_count(db, since, project_id, "IS NULL").await
}

/// How many subagent (child) sessions are live, counted as [`live_count`].
pub async fn live_subagent_count(
    db: &Database,
    since: &str,
    project_id: Option<&str>,
) -> Result<i64> {
    live_kind_count(db, since, project_id, "IS NOT NULL").await
}

async fn live_kind_count(
    db: &Database,
    since: &str,
    project_id: Option<&str>,
    parent: &str,
) -> Result<i64> {
    let mut sql = format!("SELECT COUNT(*) FROM sessions s WHERE {LIVE} AND s.parent_id {parent}");
    let mut params = vec![t(since)];
    if let Some(project_id) = project_id {
        params.push(t(project_id));
        sql.push_str(" AND s.project_id = ?2");
    }
    let conn = super::connect(db)?;
    crate::store::events::count_on(&conn, &sql, params).await
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
    /// Children that still exist, live or ended.
    pub children_count: i64,
    pub started_by: crate::author::AuthorView,
    /// The name the projects list shows for the session's project.
    pub project_display_name: Option<String>,
}

/// One session as a listing shows it, by id or alias; none when absent or
/// pruned.
pub async fn get_listed(db: &Database, session_id: &str) -> Result<Option<ListedSession>> {
    let Some(session) = lookup_id(db, session_id)
        .await?
        .filter(|session| session.deleted_at.is_none())
    else {
        return Ok(None);
    };
    let conn = super::connect(db)?;
    Ok(Some(listed(&conn, session).await?))
}

async fn listed(conn: &turso::Connection, session: Session) -> Result<ListedSession> {
    let participants = participants_on(conn, &session.id).await?;
    let children_count = crate::store::events::count_on(
        conn,
        "SELECT COUNT(*) FROM sessions WHERE parent_id = ?1 AND deleted_at IS NULL",
        vec![t(&session.id)],
    )
    .await?;
    let project_display_name = scalar(
        conn,
        "SELECT display_name FROM projects WHERE id = ?1",
        vec![t(&session.project_id)],
    )
    .await?;
    Ok(ListedSession {
        started_by: session.started_by.clone(),
        session,
        participants,
        children_count,
        project_display_name,
    })
}

/// List sessions with their participants.
pub async fn list_filtered(
    db: &Database,
    filter: &SessionFilter<'_>,
) -> Result<Vec<ListedSession>> {
    check_status(filter.status)?;
    let mut sql = String::from("WHERE s.deleted_at IS NULL");
    let mut params: Vec<Value> = Vec::new();
    fn push(sql: &mut String, params: &mut Vec<Value>, clause: &str, value: &str) {
        params.push(t(value));
        sql.push_str(&clause.replace("?N", &format!("?{}", params.len())));
    }
    let since = filter.since.unwrap_or("");
    if filter.live {
        sql.push_str(" AND s.status = 'active'");
    }
    if let Some(project) = filter.project_id {
        push(&mut sql, &mut params, " AND s.project_id = ?N", project);
    }
    if let Some(status) = filter.status {
        push(&mut sql, &mut params, " AND s.status = ?N", status);
    }
    if let Some(parent) = filter.parent {
        push(&mut sql, &mut params, " AND s.parent_id = ?N", parent);
    }
    match (filter.member, filter.live) {
        // A member's live sessions are the ones their own participation keeps
        // live, which is what the member row counts.
        (Some(member), true) => {
            params.push(t(member));
            let who = params.len();
            params.push(t(since));
            let seen = params.len();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM session_participants p
                    WHERE p.session_id = s.id AND p.member_id = ?{who}
                      AND p.left_at IS NULL AND p.last_seen_at >= ?{seen})"
            ));
        }
        (Some(member), false) => push(
            &mut sql,
            &mut params,
            " AND EXISTS (SELECT 1 FROM session_participants p
                WHERE p.session_id = s.id AND p.member_id = ?N)",
            member,
        ),
        (None, true) => push(
            &mut sql,
            &mut params,
            " AND EXISTS (SELECT 1 FROM session_participants p
                WHERE p.session_id = s.id AND p.left_at IS NULL AND p.last_seen_at >= ?N)",
            since,
        ),
        (None, false) => {}
    }
    if let Some(agent) = filter.agent {
        push(
            &mut sql,
            &mut params,
            " AND EXISTS (SELECT 1 FROM session_participants p
                WHERE p.session_id = s.id AND p.agent = ?N)",
            agent,
        );
    }
    let limit = filter
        .limit
        .unwrap_or(crate::limits::SESSION_LIST_LIMIT_MAX)
        .clamp(1, crate::limits::SESSION_LIST_LIMIT_MAX);
    params.push(Value::Integer(limit));
    sql.push_str(&format!(
        " ORDER BY s.last_activity DESC, s.id DESC LIMIT ?{}",
        params.len()
    ));
    let conn = super::connect(db)?;
    // The only table in the outer query is `sessions`, so its columns need no
    // alias; the filters' subqueries name their own.
    let mut rows = conn
        .query(&format!("SELECT {COLUMNS} FROM sessions s {sql}"), params)
        .await
        .map_err(engine)?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        sessions.push(session_from_row(&row)?);
    }
    drop(rows);
    let mut out = Vec::with_capacity(sessions.len());
    for session in sessions {
        out.push(listed(&conn, session).await?);
    }
    Ok(out)
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
