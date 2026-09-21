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
}

/// The fields of a project the human may change after creation.
///
/// A field left `None` is left alone, so a screen that edits one control does
/// not have to send the rest of the form back.
#[derive(Debug, Clone, Default)]
pub struct ProjectChanges<'a> {
    pub display_name: Option<&'a str>,
}

/// List projects, oldest first.
pub async fn list(db: &Database) -> Result<Vec<Project>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, owner_agent, created_at
             FROM projects ORDER BY created_at ASC",
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
    for project in &mut projects {
        project.unseen_events = unseen
            .iter()
            .find(|row| row.project_id == project.id)
            .map_or(0, |row| row.events);
    }
    Ok(projects)
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
    insert_owned(&tx, id, display_name, None, &created_at).await?;
    tx.commit().await.map_err(engine)?;

    Ok(Project {
        id: id.to_string(),
        display_name: display_name.to_string(),
        owner_agent: None,
        created_at,
        unseen_events: 0,
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
            "SELECT 1 FROM projects WHERE id = ?1",
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
) -> Result<()> {
    tx.execute(
        "INSERT INTO projects(id, display_name, owner_agent, created_at, retention, settings)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(display_name.to_string()),
            owner_agent.map_or(Value::Null, |agent| Value::Text(agent.to_string())),
            Value::Text(created_at.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

/// Fetch one project.
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
}

/// Count what a project holds.
///
/// Five indexed counts, no walk and nothing per row: a project tab row costs
/// one request whatever the project holds.
pub async fn stats(db: &Database, id: &str, active_since: &str) -> Result<ProjectStats> {
    let conn = super::connect(db)?;
    let project = Value::Text(id.to_string());
    let count =
        async |sql: &str| crate::store::events::count_on(&conn, sql, vec![project.clone()]).await;
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
    })
}

pub async fn get(db: &Database, id: &str) -> Result<Option<Project>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, owner_agent, created_at
             FROM projects WHERE id = ?1",
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
/// removed only with it. The rows are deleted in one transaction first, so
/// nothing is visible but partially gone; the files are then removed best
/// effort, since an orphaned file is invisible while an orphaned row is not.
pub async fn delete(db: &Database, data_dir: &Path, id: &str) -> Result<()> {
    let project = get(db, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("project {id} not found")))?;
    if project.owner_agent.is_some() {
        return Err(Error::Conflict(format!(
            "project {id} is an agent's personal space"
        )));
    }

    let session_ids = session_ids(db, id).await?;

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

    // Every artifact version lives under one directory, so remove the tree
    // rather than only the current version's file. A file failure here leaves
    // an orphan, not a dangling row.
    if let Err(err) = blob::remove_tree(data_dir, &format!("artifacts/{id}")) {
        tracing::warn!(project = id, error = %err, "artifact tree removal failed");
    }
    let brains = BrainStore::for_data_dir(data_dir);
    for session_id in session_ids {
        if let Err(err) = brains.remove(id, &session_id).await {
            tracing::warn!(session_id, error = %err, "brain removal failed");
        }
    }
    // A knowledge base has no prune path of its own: deleting the project it
    // belongs to is the only way it goes, and it goes with everything else.
    if let Err(err) = BrainStore::for_knowledge(data_dir)
        .remove(id, crate::brain::KNOWLEDGE_FILE)
        .await
    {
        tracing::warn!(project = id, error = %err, "knowledge base removal failed");
    }
    // Only the directory, and only once it is empty: removing it with whatever
    // it still holds would hide a file the removal above failed to take.
    let held = crate::brain::knowledge_dir(data_dir).join(id);
    if let Err(err) = std::fs::remove_dir(&held)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(project = id, error = %err, "knowledge base directory removal failed");
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
    Ok(Project {
        id: text(0)?,
        display_name: text(1)?,
        owner_agent,
        created_at: text(3)?,
        // Filled by the caller, which counts every project it returns in one
        // query rather than one query per row.
        unseen_events: 0,
    })
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
