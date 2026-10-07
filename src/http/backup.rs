//! Online backup: the serving hub copies its own store, on the admin's word.
//!
//! The route takes no path from the caller. The hub writes a new directory
//! under the configured `backup_dir` and nowhere else, so the admin token
//! cannot be turned into a write anywhere the hub's account can reach.

use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::ops::{self, Serving};

/// What a finished backup reports.
#[derive(Debug, Serialize)]
pub struct BackupTaken {
    /// The new directory's name under the backup directory.
    pub directory: String,
    /// Where the backup is on the node.
    pub path: String,
    /// What the backup's manifest records.
    pub manifest: ManifestSummary,
    /// How long the backup took, in milliseconds.
    pub duration_ms: u64,
}

/// The manifest of a backup, without its per-file list.
#[derive(Debug, Serialize)]
pub struct ManifestSummary {
    pub schema_version: i64,
    pub created_at: String,
    pub files: usize,
    pub bytes: u64,
}

/// `POST /api/v1/backups`
///
/// Admin only. Takes a backup into a new timestamped directory under
/// `backup_dir` and answers once it is complete. Online backup is off while
/// `backup_dir` is unset, and one backup runs at a time.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<(StatusCode, Json<BackupTaken>), Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let Some(backup_dir) = state.config.backup_dir.clone() else {
        return Err(Problem::from_error(&Error::NotFound(
            "online backup is off on this hub; set HUB_BACKUP_DIR (or backup_dir under [hub]) \
             to a directory outside the data directory and restart it"
                .to_string(),
        )));
    };
    let running = state.backup_running.clone().try_lock_owned().map_err(|_| {
        Problem::from_error(&Error::Conflict(
            "a backup is already running on this hub; wait for it to finish".to_string(),
        ))
    })?;
    let target = ops::online_target(&backup_dir, &state.data_dir)
        .map_err(|err| Problem::from_error(&err))?;

    // The copy runs as a task of its own, so a caller that gives up waiting
    // does not cancel it halfway and leave a partial directory behind. The
    // single-flight guard goes with it and is released when the copy ends.
    let task = tokio::spawn(async move {
        let _running = running;
        let started = Instant::now();
        let serving = Serving {
            db: &state.db,
            brain: &state.brain,
            knowledge: &state.knowledge,
        };
        let outcome = ops::backup_serving(&state.data_dir, serving, &target).await;
        let elapsed = started.elapsed();
        match &outcome {
            Ok(report) => tracing::info!(
                path = %target.display(),
                files = report.files,
                bytes = report.bytes,
                duration_ms = elapsed.as_millis() as u64,
                "took an online backup"
            ),
            Err(err) => tracing::warn!(
                path = %target.display(),
                error = %err,
                "an online backup failed"
            ),
        }
        outcome.map(|report| (report, elapsed, target))
    });
    let (report, elapsed, target) = task
        .await
        .map_err(|err| {
            Problem::from_error(&Error::Engine(format!("the backup task failed: {err}")))
        })?
        .map_err(|err| Problem::from_error(&err))?;

    let directory = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((
        StatusCode::CREATED,
        Json(BackupTaken {
            directory,
            path: target.display().to_string(),
            manifest: ManifestSummary {
                schema_version: report.schema_version,
                created_at: report.created_at,
                files: report.files,
                bytes: report.bytes,
            },
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        }),
    ))
}
