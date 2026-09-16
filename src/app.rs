//! Composition root: open the engine, apply migrations, serve.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::Config;
use crate::error::Result;
use crate::principal::Auth;
use crate::{http, store};

/// Shared state handed to every HTTP handler and the MCP server.
#[derive(Clone)]
pub struct AppState {
    /// Process configuration.
    pub config: Arc<Config>,
    /// Directory holding the hub store, session files, and artifact blobs.
    pub data_dir: PathBuf,
    /// Applied schema version, surfaced on the readiness probe.
    pub schema_version: i64,
    /// The hub store handle.
    pub db: turso::Database,
    /// Token resolver.
    pub auth: Arc<Auth>,
}

impl AppState {
    /// Create the data layout and open the store.
    pub async fn open(config: Config) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(config.sessions_dir())?;
        std::fs::create_dir_all(config.artifacts_dir())?;

        let db = store::open_engine(&config.hub_db_path()).await?;
        let schema_version = store::migrate(&db).await?;

        let data_dir = config.data_dir.clone();
        let auth = Arc::new(Auth::from_config(&config));
        Ok(Self {
            config: Arc::new(config),
            data_dir,
            schema_version,
            db,
            auth,
        })
    }
}

/// Run the hub HTTP surface until the process is stopped.
pub async fn run(config: Config) -> Result<()> {
    let state = AppState::open(config).await?;
    let bind = state.config.bind;

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(bind = %bind, schema_version = state.schema_version, "hub listening");

    axum::serve(listener, http::router(state)).await?;
    Ok(())
}
