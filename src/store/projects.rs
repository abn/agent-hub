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
}

/// List projects, oldest first.
pub async fn list(db: &Database) -> Result<Vec<Project>> {
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, owner_agent, created_at FROM projects ORDER BY created_at ASC",
            (),
        )
        .await
        .map_err(engine)?;
    let mut projects = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        projects.push(project_from_row(&row)?);
    }
    Ok(projects)
}

/// Create a project. The id is a slug, immutable after creation.
pub async fn create(db: &Database, id: &str, display_name: &str) -> Result<Project> {
    validate_id(id)?;
    validate_display_name(display_name)?;
    let created_at = crate::store::now_rfc3339();

    // Inside the immediate transaction so a concurrent create is a conflict
    // rather than a raw engine error.
    let mut conn = db.connect().map_err(engine)?;
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
    })
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
pub async fn get(db: &Database, id: &str) -> Result<Option<Project>> {
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT id, display_name, owner_agent, created_at FROM projects WHERE id = ?1",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(project_from_row(&row)?)),
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

    let mut conn = db.connect().map_err(engine)?;
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
    Ok(())
}

/// Every session id for a project, including a pruned session whose brain file
/// has not been swept yet.
async fn session_ids(db: &Database, project_id: &str) -> Result<Vec<String>> {
    let conn = db.connect().map_err(engine)?;
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
