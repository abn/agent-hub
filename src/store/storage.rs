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
    pub kb_bytes: i64,
}

/// Total storage used, and a per-project breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct StorageUsage {
    pub total_bytes: i64,
    pub projects: Vec<ProjectUsage>,
}

/// Compute storage usage from artifact sizes, session brain file sizes, and
/// knowledge base file sizes.
pub async fn usage(db: &Database, data_dir: &Path) -> Result<StorageUsage> {
    let conn = super::connect(db)?;

    let mut by_project: Vec<ProjectUsage> = Vec::new();
    let ensure = |by_project: &mut Vec<ProjectUsage>, project_id: &str| {
        if !by_project.iter().any(|p| p.project_id == project_id) {
            by_project.push(ProjectUsage {
                project_id: project_id.to_string(),
                artifact_bytes: 0,
                session_bytes: 0,
                kb_bytes: 0,
            });
        }
    };

    // Every version keeps its blob on disk (only delete removes the tree), so
    // the report sums the version rows rather than just the current pointer.
    let mut artifacts = conn
        .query(
            "SELECT a.project_id, COALESCE(SUM(v.size_bytes), 0)
             FROM artifact_versions v JOIN artifacts a ON a.id = v.artifact_id
             GROUP BY a.project_id",
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

    // A knowledge base is never pruned, so it appears here and never in what
    // the human can reclaim. Only a project that has one is listed, so the
    // report still names the projects that hold something.
    let mut projects = conn
        .query("SELECT id FROM projects", ())
        .await
        .map_err(engine)?;
    while let Some(row) = projects.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let bytes = knowledge_bytes(data_dir, &project_id);
        if bytes == 0 {
            continue;
        }
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.kb_bytes = bytes;
        }
    }

    by_project.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    let total_bytes = by_project
        .iter()
        .map(|p| p.artifact_bytes + p.session_bytes + p.kb_bytes)
        .sum();
    Ok(StorageUsage {
        total_bytes,
        projects: by_project,
    })
}

/// The bytes a project's knowledge base holds, including what the engine
/// keeps in the write-ahead log beside it.
fn knowledge_bytes(data_dir: &Path, project_id: &str) -> i64 {
    let file = crate::brain::knowledge_dir(data_dir)
        .join(project_id)
        .join(format!("{}.db", crate::brain::KNOWLEDGE_FILE));
    let sidecar = file.with_extension("db-wal");
    [file, sidecar]
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .map(|meta| meta.len() as i64)
                .unwrap_or(0)
        })
        .sum()
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
