//! Composition root: open the engine, apply migrations, serve.

use std::path::PathBuf;
use std::sync::Arc;

use crate::brain::BrainStore;
use crate::config::Config;
use crate::error::Result;
use crate::principal::Auth;
use crate::{blob, http, mcp, net, store};

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
    /// Per-project knowledge base files.
    pub knowledge: BrainStore,
    /// Token resolver.
    pub auth: Arc<Auth>,
    /// When each session was last touched, so an agent at work stays counted
    /// as active without a store write per tool call.
    pub activity: Arc<store::sessions::Activity>,
    /// The memo over the numbers that cost a syscall or a file walk.
    pub stats: Arc<store::storage::StatsCache>,
    /// Bumped by every write, so the memo can tell a stale entry from a fresh
    /// one without knowing what changed.
    pub generation: Arc<std::sync::atomic::AtomicU64>,
    /// The name this node shows the human.
    pub host: String,
    /// Freshness ticks for the human stream. A write that changes the inbox or
    /// feed sends one; the stream carries no data, only the nudge to refetch.
    pub ticker: tokio::sync::broadcast::Sender<()>,
}

impl AppState {
    /// Create the data layout and open the store.
    pub async fn open(config: Config) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(config.sessions_dir())?;
        std::fs::create_dir_all(config.knowledge_dir())?;
        std::fs::create_dir_all(config.artifacts_dir())?;

        let db = store::open_engine(&config.hub_db_path()).await?;
        let schema_version = store::migrate(&db).await?;

        // The engine lock is held from here, so no other hub is mid-update over
        // this directory and none is in flight in this one: any content still
        // waiting for a version number was left by an interrupted update. This
        // is housekeeping, so a file it cannot remove is a warning and never a
        // reason the hub does not start.
        match blob::reap_pending(&config.data_dir) {
            Ok(0) => {}
            Ok(reaped) => tracing::info!(
                reaped,
                "removed artifact content left by an interrupted update"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                "could not clear artifact content left by an interrupted update"
            ),
        }

        let data_dir = config.data_dir.clone();
        let brain = BrainStore::new(config.sessions_dir());
        let knowledge = BrainStore::new(config.knowledge_dir());
        let auth = Arc::new(Auth::from_config(&config));
        let activity = Arc::new(store::sessions::Activity::new());
        let host = store::storage::host_name(config.node_name.as_deref());
        let (ticker, _) = tokio::sync::broadcast::channel(16);
        Ok(Self {
            config: Arc::new(config),
            data_dir,
            schema_version,
            db,
            brain,
            knowledge,
            auth,
            activity,
            stats: Arc::new(store::storage::StatsCache::new()),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            host,
            ticker,
        })
    }

    /// Nudge every stream subscriber to refetch. A send with no subscribers is
    /// not an error; the next subscriber gets the state on its next write.
    pub fn notify(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.ticker.send(());
    }

    /// The generation a cached number must have been computed at to be served.
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
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
            match store::prune::sweep(&sweeper.db, &sweeper.data_dir).await {
                Ok(committed) if committed > 0 => sweeper.notify(),
                Ok(_) => {}
                Err(err) => tracing::warn!(error = %err, "prune sweep failed"),
            }
            tokio::time::sleep(interval).await;
        }
    });

    let router = http::router(state.clone()).merge(mcp::http_router(state.clone()));

    // An optional tailnet endpoint serves the same router on the device's
    // tailnet address, beside the plain listener.
    let tailnet = state.config.tailnet_from_env()?;
    if tailnet.enabled() && state.config.admin_token.is_none() {
        // The tailnet endpoint is reachable by any peer on the tailnet, so it
        // needs the same admin token the loopback bind is allowed to omit.
        return Err(crate::error::Error::Config(
            "HUB_ADMIN_TOKEN is required when the tailnet endpoint is enabled".to_string(),
        ));
    }
    if tailnet.enabled() {
        let tailnet_router = router.clone();
        tokio::spawn(async move {
            if let Err(err) = net::serve(&tailnet, tailnet_router).await {
                tracing::error!(error = %err, "tailnet endpoint stopped");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(bind).await?;
    // The address the listener got, which is not the configured one when the
    // operator asked for port 0.
    tracing::info!(bind = %listener.local_addr()?, schema_version, "hub listening");

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
