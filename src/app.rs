//! Composition root: open the engine, apply migrations, serve.

use std::path::PathBuf;

use crate::config::Config;
use crate::error::Result;
use crate::{http, store};

/// Shared state handed to every HTTP handler.
#[derive(Clone)]
pub struct AppState {
    /// Directory holding the hub store, session files, and artifact blobs.
    pub data_dir: PathBuf,
    /// Applied schema version, surfaced on the readiness probe.
    pub schema_version: i64,
}

/// Run the hub until the process is stopped.
pub async fn run(config: Config) -> Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    std::fs::create_dir_all(config.sessions_dir())?;
    std::fs::create_dir_all(config.artifacts_dir())?;

    let db = store::open_engine(&config.hub_db_path()).await?;
    let schema_version = store::migrate(&db).await?;

    let state = AppState {
        data_dir: config.data_dir.clone(),
        schema_version,
    };

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, schema_version, "hub listening");

    axum::serve(listener, http::router(state)).await?;
    Ok(())
}
