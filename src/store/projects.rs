//! Projects: the top-level grouping for a feed, sessions, and artifacts.

use serde::Serialize;
use turso::{Database, Value};

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
    tx.execute(
        "INSERT INTO projects(id, display_name, owner_agent, created_at, retention, settings)
         VALUES (?1, ?2, NULL, ?3, NULL, NULL)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(display_name.to_string()),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    tx.commit().await.map_err(engine)?;

    Ok(Project {
        id: id.to_string(),
        display_name: display_name.to_string(),
        owner_agent: None,
        created_at,
    })
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
