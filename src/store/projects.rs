//! Projects: the top-level grouping for a feed, sessions, and artifacts.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Value};

use crate::blob;
use crate::brain::BrainStore;
use crate::error::{Error, Result};

/// A project.
#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: String,
    pub display_name: String,
    pub owner_agent: Option<String>,
    pub created_at: String,
    /// Feed events newer than the human's last-seen cursor on this project.
    pub unseen_events: i64,
    /// Distinct agents with a live session here. Carried on the listing so a
    /// screen showing many projects learns it once: the rail used to ask for
    /// each project separately, on every navigation.
    pub agents_active: i64,
    /// Whether the project is confidential and hidden without a grant.
    pub confidential: bool,
    /// Status: 'active' or 'deleting'.
    pub status: String,
}

/// The fields of a project the human may change after creation.
///
/// A field left `None` is left alone, so a screen that edits one control does
/// not have to send the rest of the form back.
#[derive(Debug, Clone, Default)]
pub struct ProjectChanges<'a> {
    pub display_name: Option<&'a str>,
    pub confidential: Option<bool>,
}

/// List projects, oldest first.
pub async fn list(db: &Database, active_since: &str) -> Result<Vec<Project>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, owner_agent, created_at, confidential, status
             FROM projects WHERE status = 'active' ORDER BY created_at ASC",
            (),
        )
        .await
        .map_err(engine)?;
    let mut projects = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        projects.push(project_from_row(&row)?);
    }

    // One grouped count for the whole listing rather than one per project.
    let unseen = crate::store::events::unseen_counts(db).await?;
    let live = crate::store::sessions::agents_active_by_project(db, active_since).await?;
    for project in &mut projects {
        project.unseen_events = unseen
            .iter()
            .find(|row| row.project_id == project.id)
            .map_or(0, |row| row.events);
        project.agents_active = live
            .iter()
            .find(|(id, _)| id == &project.id)
            .map_or(0, |(_, count)| *count);
    }
    Ok(projects)
}

/// List projects visible to the given principal, oldest first.
pub async fn list_visible(
    db: &Database,
    active_since: &str,
    principal: &crate::principal::Principal,
) -> Result<Vec<Project>> {
    let all = list(db, active_since).await?;
    if principal.is_admin {
        return Ok(all);
    }
    let visible = crate::policy::visibility(db, principal).await?;
    let filtered = match visible.as_filter() {
        None => all,
        Some(allowed) => all
            .into_iter()
            .filter(|p| allowed.contains(&p.id))
            .collect(),
    };
    Ok(filtered)
}

/// The display names of the projects named, by id, in one read.
///
/// A response that carries project ids calls this once for all of them rather
/// than once per row. An id with no project row is absent from the answer, and
/// no ids means no query.
pub(crate) async fn display_names(
    conn: &turso::Connection,
    ids: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    let mut wanted: Vec<&str> = ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let mut names = std::collections::HashMap::new();
    if wanted.is_empty() {
        return Ok(names);
    }
    let holes: Vec<String> = (1..=wanted.len()).map(|at| format!("?{at}")).collect();
    let mut rows = conn
        .query(
            &format!(
                "SELECT id, display_name FROM projects WHERE id IN ({})",
                holes.join(", ")
            ),
            wanted
                .iter()
                .map(|id| Value::Text(id.to_string()))
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = rows.next().await.map_err(engine)? {
        if let (Value::Text(id), Value::Text(name)) = (
            row.get_value(0).map_err(engine)?,
            row.get_value(1).map_err(engine)?,
        ) {
            names.insert(id, name);
        }
    }
    Ok(names)
}

/// Create a project. The id is a slug, immutable after creation.
pub async fn create(db: &Database, id: &str, display_name: &str) -> Result<Project> {
    create_with_confidential(db, id, display_name, false).await
}

/// Create a project with an explicit confidential posture.
pub async fn create_with_confidential(
    db: &Database,
    id: &str,
    display_name: &str,
    confidential: bool,
) -> Result<Project> {
    validate_id(id)?;
    validate_display_name(display_name)?;
    let created_at = crate::store::now_rfc3339();

    // Inside the immediate transaction so a concurrent create is a conflict
    // rather than a raw engine error.
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let mut rows = tx
        .query(
            "SELECT 1 FROM projects WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    if rows.next().await.map_err(engine)?.is_some() {
        return Err(Error::Conflict(format!("project {id} already exists")));
    }
    insert_owned(&tx, id, display_name, None, &created_at, confidential).await?;
    tx.commit().await.map_err(engine)?;

    Ok(Project {
        id: id.to_string(),
        display_name: display_name.to_string(),
        owner_agent: None,
        created_at,
        unseen_events: 0,
        agents_active: 0,
        confidential,
        status: "active".to_string(),
    })
}

/// Change what the human may change about a project.
///
/// The id is not among them: it is the slug every MCP call and every other
/// table names, so it is read-only after creation. An agent's personal space is
/// settable like any other project; only deleting it is refused, because that
/// is the agent's lifecycle rather than a setting.
pub async fn update(db: &Database, id: &str, changes: ProjectChanges<'_>) -> Result<Project> {
    if let Some(display_name) = changes.display_name {
        validate_display_name(display_name)?;
    }

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let mut rows = tx
        .query(
            "SELECT 1 FROM projects WHERE id = ?1 AND status = 'active'",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    if rows.next().await.map_err(engine)?.is_none() {
        return Err(Error::NotFound(format!("project {id} not found")));
    }
    drop(rows);

    // Only what was named is written, so an untouched field cannot be blanked
    // by a screen that edits one control.
    let mut sets = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if let Some(display_name) = changes.display_name {
        params.push(Value::Text(display_name.to_string()));
        sets.push(format!("display_name = ?{}", params.len()));
    }
    if let Some(confidential) = changes.confidential {
        params.push(Value::Integer(if confidential { 1 } else { 0 }));
        sets.push(format!("confidential = ?{}", params.len()));
    }
    if !sets.is_empty() {
        params.push(Value::Text(id.to_string()));
        let sql = format!(
            "UPDATE projects SET {} WHERE id = ?{}",
            sets.join(", "),
            params.len()
        );
        tx.execute(&sql, params).await.map_err(engine)?;
    }
    tx.commit().await.map_err(engine)?;

    get(db, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("project {id} not found")))
}

/// Insert a project inside a caller's transaction.
///
/// `owner_agent` is set when the project is an agent's personal space, so the
/// agent row and its space can be written as one unit.
pub(crate) async fn insert_owned(
    tx: &turso::transaction::Transaction<'_>,
    id: &str,
    display_name: &str,
    owner_agent: Option<&str>,
    created_at: &str,
    confidential: bool,
) -> Result<()> {
    tx.execute(
        "INSERT INTO projects(id, display_name, owner_agent, created_at, retention, settings, confidential, status)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, 'active')",
        vec![
            Value::Text(id.to_string()),
            Value::Text(display_name.to_string()),
            owner_agent.map_or(Value::Null, |agent| Value::Text(agent.to_string())),
            Value::Text(created_at.to_string()),
            Value::Integer(if confidential { 1 } else { 0 }),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// The counts a project header and its tab row show.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectStats {
    pub project_id: String,
    /// Feed events in the project, the hub's own audit trail excluded.
    pub events: i64,
    pub artifacts: i64,
    /// Live sessions, pruned ones excluded.
    pub sessions: i64,
    /// Pages in the project knowledge base.
    pub kb_pages: i64,
    /// Agents with a session in this project touched inside the active window.
    pub agents_active: i64,
    /// Distinct non-null threads across events in this project.
    pub threads: i64,
    /// Distinct non-null actors across events in this project.
    pub agents_written: i64,
    /// Total bytes on disk for this project (artifacts, session brains, knowledge base).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_bytes: Option<i64>,
    /// Total bytes on disk for this project (alias).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_on_disk_bytes: Option<i64>,
}

async fn disk_bytes_for_project(
    conn: &turso::Connection,
    data_dir: &Path,
    id: &str,
) -> Result<i64> {
    let mut artifacts = conn
        .query(
            "SELECT COALESCE(SUM(v.size_bytes), 0)
             FROM artifact_versions v JOIN artifacts a ON a.id = v.artifact_id
             WHERE a.project_id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    let artifact_bytes = match artifacts.next().await.map_err(engine)? {
        Some(row) => match row.get_value(0).map_err(engine)? {
            Value::Integer(b) => b,
            _ => 0,
        },
        None => 0,
    };
    drop(artifacts);

    let mut sessions = conn
        .query(
            "SELECT brain_path FROM sessions WHERE project_id = ?1 AND deleted_at IS NULL",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut session_bytes = 0i64;
    while let Some(row) = sessions.next().await.map_err(engine)? {
        if let Value::Text(brain_path) = row.get_value(0).map_err(engine)? {
            session_bytes += crate::brain::file_bytes(&data_dir.join(&brain_path));
        }
    }
    drop(sessions);

    let kb_file = crate::brain::knowledge_dir(data_dir)
        .join(id)
        .join(format!("{}.db", crate::brain::KNOWLEDGE_FILE));
    let kb_bytes = crate::brain::file_bytes(&kb_file);

    Ok(artifact_bytes + session_bytes + kb_bytes)
}

/// Count what a project holds.
///
/// Five indexed counts, no walk and nothing per row: a project tab row costs
/// one request whatever the project holds.
pub async fn stats(
    db: &Database,
    data_dir: Option<&Path>,
    id: &str,
    active_since: &str,
) -> Result<ProjectStats> {
    let conn = super::connect(db)?;
    let project = Value::Text(id.to_string());
    let count =
        async |sql: &str| crate::store::events::count_on(&conn, sql, vec![project.clone()]).await;
    let disk_bytes = match data_dir {
        Some(dir) => disk_bytes_for_project(&conn, dir, id).await.ok(),
        None => None,
    };
    Ok(ProjectStats {
        project_id: id.to_string(),
        events: crate::store::events::count_for_project(db, id).await?,
        artifacts: count("SELECT COUNT(*) FROM artifacts WHERE project_id = ?1").await?,
        sessions: count(
            "SELECT COUNT(*) FROM sessions WHERE project_id = ?1 AND deleted_at IS NULL",
        )
        .await?,
        // The corpus is the cheap and correct source: every page is indexed on
        // write and its row goes on delete, so this needs no walk of the file.
        kb_pages: count("SELECT COUNT(*) FROM search_docs WHERE project_id = ?1 AND type = 'kb'")
            .await?,
        agents_active: crate::store::sessions::agents_active(db, active_since, Some(id)).await?,
        threads: count(
            "SELECT COUNT(DISTINCT thread_id) FROM events WHERE project_id = ?1 AND thread_id IS NOT NULL",
        )
        .await?,
        agents_written: count(
            "SELECT COUNT(DISTINCT actor) FROM events WHERE project_id = ?1 AND actor IS NOT NULL",
        )
        .await?,
        disk_bytes,
        files_on_disk_bytes: disk_bytes,
    })
}

pub async fn get(db: &Database, id: &str) -> Result<Option<Project>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, owner_agent, created_at, confidential, status
             FROM projects WHERE id = ?1 AND status = 'active'",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => {
            let mut project = project_from_row(&row)?;
            project.unseen_events = crate::store::events::unseen_count(db, id).await?;
            Ok(Some(project))
        }
        None => Ok(None),
    }
}

/// Delete a project and every row and file scoped to it.
///
/// Refuses an agent's personal space: that project belongs to the agent and is
/// removed only with it. The deletion sets status to 'deleting' in an immediate
/// transaction to block late writes, moves project directories to generation-scoped
/// quarantined locations, deletes metadata rows in an immediate transaction,
/// and then removes the quarantined files.
pub async fn delete(db: &Database, data_dir: &Path, id: &str) -> Result<()> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let mut rows = tx
        .query(
            "SELECT owner_agent, status FROM projects WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    let (owner_agent, status) = match rows.next().await.map_err(engine)? {
        Some(row) => {
            let owner: Option<String> = match row.get_value(0).map_err(engine)? {
                Value::Text(s) => Some(s),
                _ => None,
            };
            let status: String = match row.get_value(1).map_err(engine)? {
                Value::Text(s) => s,
                _ => "active".to_string(),
            };
            (owner, status)
        }
        None => return Err(Error::NotFound(format!("project {id} not found"))),
    };
    if owner_agent.is_some() {
        return Err(Error::Conflict(format!(
            "project {id} is an agent's personal space"
        )));
    }
    if status != "active" {
        return Err(Error::NotFound(format!("project {id} not found")));
    }

    tx.execute(
        "UPDATE projects SET status = 'deleting' WHERE id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;

    finish_delete(db, data_dir, id).await
}

pub(crate) async fn finish_delete(db: &Database, data_dir: &Path, id: &str) -> Result<()> {
    let session_ids = session_ids(db, id).await?;

    let del_id = crate::store::next_id();
    let q_art = data_dir
        .join("artifacts")
        .join(format!(".deleted-{id}-{del_id}"));
    let art_dir = data_dir.join("artifacts").join(id);
    if art_dir.exists() {
        let _ = std::fs::rename(&art_dir, &q_art);
    }

    let q_sess = data_dir
        .join("sessions")
        .join(format!(".deleted-{id}-{del_id}"));
    let sess_dir = data_dir.join("sessions").join(id);
    if sess_dir.exists() {
        let _ = std::fs::rename(&sess_dir, &q_sess);
    }

    let q_kb = crate::brain::knowledge_dir(data_dir).join(format!(".deleted-{id}-{del_id}"));
    let kb_dir = crate::brain::knowledge_dir(data_dir).join(id);
    if kb_dir.exists() {
        let _ = std::fs::rename(&kb_dir, &q_kb);
    }

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    // Inbox and search rows hang off events, so they go first.
    tx.execute(
        "DELETE FROM inbox WHERE event_id IN (SELECT id FROM events WHERE project_id = ?1)",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM events WHERE project_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM artifact_versions WHERE artifact_id IN (SELECT id FROM artifacts WHERE project_id = ?1)",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM comments WHERE artifact_id IN (SELECT id FROM artifacts WHERE project_id = ?1)",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM artifacts WHERE project_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM sessions WHERE project_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM grants WHERE project_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM idempotency WHERE project_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM search_docs WHERE project_id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    crate::store::events::forget_cursor_in_tx(&tx, id).await?;
    tx.execute(
        "DELETE FROM projects WHERE id = ?1",
        vec![Value::Text(id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;

    if q_art.exists() {
        let _ = std::fs::remove_dir_all(&q_art);
    }
    if q_sess.exists() {
        let _ = std::fs::remove_dir_all(&q_sess);
    }
    if q_kb.exists() {
        let _ = std::fs::remove_dir_all(&q_kb);
    }

    if let Err(err) = blob::remove_tree(data_dir, &format!("artifacts/{id}")) {
        tracing::warn!(project = id, error = %err, "artifact tree removal failed");
    }
    let brains = BrainStore::for_data_dir(data_dir);
    for session_id in session_ids {
        if let Err(err) = brains.remove(id, &session_id).await {
            tracing::warn!(session_id, error = %err, "brain removal failed");
        }
    }
    if let Err(err) = BrainStore::for_knowledge(data_dir)
        .remove(id, crate::brain::KNOWLEDGE_FILE)
        .await
    {
        tracing::warn!(project = id, error = %err, "knowledge base removal failed");
    }
    let held = crate::brain::knowledge_dir(data_dir).join(id);
    if let Err(err) = std::fs::remove_dir(&held)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(project = id, error = %err, "knowledge base directory removal failed");
    }
    Ok(())
}

/// Recover any projects left in 'deleting' status and clean up orphaned generation-scoped directories.
pub async fn recover(db: &Database, data_dir: &Path) -> Result<usize> {
    let mut recovered = 0;
    clean_deleted_dirs(&data_dir.join("artifacts"))?;
    clean_deleted_dirs(&data_dir.join("sessions"))?;
    clean_deleted_dirs(&crate::brain::knowledge_dir(data_dir))?;

    let conn = super::connect(db)?;
    let mut rows = conn
        .query("SELECT id FROM projects WHERE status = 'deleting'", ())
        .await
        .map_err(engine)?;
    let mut deleting_ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        if let Value::Text(id) = row.get_value(0).map_err(engine)? {
            deleting_ids.push(id);
        }
    }
    drop(rows);
    drop(conn);

    for id in deleting_ids {
        if let Err(err) = finish_delete(db, data_dir, &id).await {
            tracing::warn!(project = id, error = %err, "could not complete recovery of deleting project");
        } else {
            recovered += 1;
        }
    }

    Ok(recovered)
}

fn clean_deleted_dirs(parent: &Path) -> Result<()> {
    if !parent.exists() {
        return Ok(());
    }
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with(".deleted-")
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
}

/// Every session id for a project, including a pruned session whose brain file
/// has not been swept yet.
async fn session_ids(db: &Database, project_id: &str) -> Result<Vec<String>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id FROM sessions WHERE project_id = ?1",
            vec![Value::Text(project_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        if let Value::Text(id) = row.get_value(0).map_err(engine)? {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn project_from_row(row: &turso::Row) -> Result<Project> {
    let text = |index: usize| -> Result<String> {
        match row.get_value(index).map_err(engine)? {
            Value::Text(value) => Ok(value),
            other => Err(Error::Engine(format!(
                "expected text in a project column, found {other:?}"
            ))),
        }
    };
    let owner_agent = match row.get_value(2).map_err(engine)? {
        Value::Text(value) => Some(value),
        _ => None,
    };
    let confidential = match row.get_value(4).map_err(engine)? {
        Value::Integer(value) => value != 0,
        _ => false,
    };
    let status = match row.get_value(5).map_err(engine)? {
        Value::Text(value) => value,
        _ => "active".to_string(),
    };
    Ok(Project {
        id: text(0)?,
        display_name: text(1)?,
        owner_agent,
        created_at: text(3)?,
        // Both filled by the caller, which counts every project it returns in
        // one query rather than one query per row.
        unseen_events: 0,
        agents_active: 0,
        confidential,
        status,
    })
}

/// Set a project's confidential posture.
pub async fn set_confidential(db: &Database, id: &str, confidential: bool) -> Result<()> {
    let conn = super::connect(db)?;
    conn.execute(
        "UPDATE projects SET confidential = ?1 WHERE id = ?2",
        vec![
            Value::Integer(if confidential { 1 } else { 0 }),
            Value::Text(id.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

fn validate_display_name(display_name: &str) -> Result<()> {
    if display_name.trim().is_empty() {
        return Err(Error::InvalidArgument(
            "a project display name is required".to_string(),
        ));
    }
    if display_name.chars().count() > 200 {
        return Err(Error::InvalidArgument(
            "the project display name is too long".to_string(),
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "project id '{id}' must be non-empty and contain only ASCII alphanumerics, hyphens, and underscores"
        )))
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
