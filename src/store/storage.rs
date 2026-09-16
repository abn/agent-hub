//! Storage usage: what is on the data volume, by project.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Value};

use crate::error::{Error, Result};

/// Storage used by one project.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectUsage {
    pub project_id: String,
    pub artifact_bytes: i64,
    pub session_bytes: i64,
}

/// Total storage used, and a per-project breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct StorageUsage {
    pub total_bytes: i64,
    pub projects: Vec<ProjectUsage>,
}

/// Compute storage usage from artifact sizes and session brain file sizes.
pub async fn usage(db: &Database, data_dir: &Path) -> Result<StorageUsage> {
    let conn = db.connect().map_err(engine)?;

    let mut by_project: Vec<ProjectUsage> = Vec::new();
    let ensure = |by_project: &mut Vec<ProjectUsage>, project_id: &str| {
        if !by_project.iter().any(|p| p.project_id == project_id) {
            by_project.push(ProjectUsage {
                project_id: project_id.to_string(),
                artifact_bytes: 0,
                session_bytes: 0,
            });
        }
    };

    let mut artifacts = conn
        .query(
            "SELECT project_id, COALESCE(SUM(size_bytes), 0) FROM artifacts GROUP BY project_id",
            (),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = artifacts.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let bytes = integer(row.get_value(1).map_err(engine)?);
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.artifact_bytes = bytes;
        }
    }

    let mut sessions = conn
        .query(
            "SELECT project_id, brain_path FROM sessions WHERE deleted_at IS NULL",
            (),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = sessions.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let brain_path = text(row.get_value(1).map_err(engine)?);
        let bytes = std::fs::metadata(data_dir.join(&brain_path))
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.session_bytes += bytes;
        }
    }

    by_project.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    let total_bytes = by_project
        .iter()
        .map(|p| p.artifact_bytes + p.session_bytes)
        .sum();
    Ok(StorageUsage {
        total_bytes,
        projects: by_project,
    })
}

fn text(value: Value) -> String {
    match value {
        Value::Text(value) => value,
        _ => String::new(),
    }
}

fn integer(value: Value) -> i64 {
    match value {
        Value::Integer(value) => value,
        _ => 0,
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
