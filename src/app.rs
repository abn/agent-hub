//! Composition root: open the engine, apply migrations, serve.

use std::path::PathBuf;
use std::sync::Arc;

use crate::brain::BrainStore;
use crate::config::Config;
use crate::error::Result;
use crate::principal::Auth;
use crate::{http, mcp, net, store};

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
    /// Per-session brain files.
    pub brain: BrainStore,
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
        let brain = BrainStore::new(config.sessions_dir());
        let auth = Arc::new(Auth::from_config(&config));
        Ok(Self {
            config: Arc::new(config),
            data_dir,
            schema_version,
            db,
            brain,
            auth,
        })
    }
}

/// Run the hub until the process is stopped.
///
/// One process serves the REST API, the PWA, and the MCP streamable HTTP
/// endpoint on one listener, and runs the prune sweeper. The per-session write
/// lock only spans this process, so a second server over one data directory is
/// not a supported topology; the engine's exclusive file lock rejects it at
/// startup rather than letting the two corrupt data.
pub async fn run(config: Config) -> Result<()> {
    let state = AppState::open(config).await?;
    let bind = state.config.bind;
    let schema_version = state.schema_version;

    // Commit any prune whose undo window has passed, then keep sweeping.
    let sweeper = state.clone();
    tokio::spawn(async move {
        let interval = sweep_interval();
        loop {
            if let Err(err) = store::prune::sweep(&sweeper.db, &sweeper.data_dir).await {
                tracing::warn!(error = %err, "prune sweep failed");
            }
            tokio::time::sleep(interval).await;
        }
    });

    let router = http::router(state.clone()).merge(mcp::http_router(state.clone()));

    // An optional tailnet endpoint serves the same router on the device's
    // tailnet address, beside the plain listener.
    let tailnet = crate::config::Config::tailnet_from_env()?;
    if tailnet.enabled() {
        let tailnet_router = router.clone();
        tokio::spawn(async move {
            if let Err(err) = net::serve(&tailnet, tailnet_router).await {
                tracing::error!(error = %err, "tailnet endpoint stopped");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(bind = %bind, schema_version, "hub listening");

    axum::serve(listener, router).await?;
    Ok(())
}

/// How often the prune sweeper runs. Overridable so a test can watch it commit
/// without waiting the full undo window.
fn sweep_interval() -> std::time::Duration {
    let secs = std::env::var("HUB_SWEEP_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(store::prune::UNDO_WINDOW_SECS as u64);
    std::time::Duration::from_secs(secs)
}
