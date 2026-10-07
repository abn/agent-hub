//! Composition root: open the engine, apply migrations, serve.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::brain::BrainStore;
use crate::config::{self, Config};
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
    /// feed sends one; a live artifact write sends one naming the artifact and
    /// its revision. The stream carries no content either way, only the nudge
    /// to refetch.
    pub ticker: tokio::sync::broadcast::Sender<Tick>,
    /// The contentless nudge to the operator's notify target, off unless one
    /// is configured.
    pub notifier: crate::notify::Notifier,
    /// Share flag recorded during enrolment approvals.
    pub enrol_shares: Arc<std::sync::Mutex<std::collections::HashMap<String, bool>>>,
    /// How often the background store integrity sample runs. Zero disables it.
    pub integrity_sample: std::time::Duration,
    /// The data directory and hub store as they were when the store opened.
    /// The readiness probe compares against this so a removed or replaced path
    /// is unavailable even while the open file descriptor still answers.
    pub store_identity: StoreIdentity,
}

/// A freshness tick for the human stream.
///
/// A plain tick means "something changed, refetch what you show". A tick that
/// names a live artifact means the same thing with a topic, so a viewer of that
/// artifact can refetch it and leave every other screen alone. Either way the
/// stream carries no content.
#[derive(Clone, Debug, Default)]
pub struct Tick {
    /// The artifact whose live version just moved, when this tick is about one.
    pub live: Option<LiveTick>,
}

/// One live artifact write, as the stream reports it.
#[derive(Clone, Debug)]
pub struct LiveTick {
    pub artifact_id: String,
    pub version: i64,
    pub live_rev: i64,
}

/// The device and inode of a path, for detecting that it was removed or
/// replaced under a running process.
#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
pub struct PathIdentity {
    pub dev: u64,
    pub ino: u64,
}

#[cfg(unix)]
impl PathIdentity {
    /// Read the identity of `path` as it is now.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
}

/// The identity of the data directory and `hub.db` captured at open.
///
/// Empty on non-Unix, where the version leg is the only one available.
#[derive(Clone, Copy, Debug, Default)]
pub struct StoreIdentity {
    #[cfg(unix)]
    pub data_dir: Option<PathIdentity>,
    #[cfg(unix)]
    pub hub_db: Option<PathIdentity>,
}

impl StoreIdentity {
    /// Compare the captured identity against the paths as they are now.
    ///
    /// A path that no longer stats or whose device or inode moved is not the
    /// store this process opened: it was removed, unmounted or replaced.
    #[cfg(unix)]
    pub fn check(&self, data_dir: &Path, hub_db: &Path) -> std::result::Result<(), String> {
        if let Some(expected) = self.data_dir {
            compare_path("the data directory", data_dir, expected)?;
        }
        if let Some(expected) = self.hub_db {
            compare_path("the hub store", hub_db, expected)?;
        }
        Ok(())
    }

    /// No identity leg off Unix; the version leg carries readiness alone.
    #[cfg(not(unix))]
    pub fn check(&self, _data_dir: &Path, _hub_db: &Path) -> std::result::Result<(), String> {
        Ok(())
    }
}

#[cfg(unix)]
fn compare_path(
    label: &str,
    path: &Path,
    expected: PathIdentity,
) -> std::result::Result<(), String> {
    match PathIdentity::of(path) {
        Ok(now) if now.dev == expected.dev && now.ino == expected.ino => Ok(()),
        Ok(_) => Err(format!("{label} at {} was replaced", path.display())),
        Err(err) => Err(format!("{label} at {} is gone: {err}", path.display())),
    }
}

fn capture_store_identity(data_dir: &Path, hub_db: &Path) -> StoreIdentity {
    #[cfg(unix)]
    {
        StoreIdentity {
            data_dir: PathIdentity::of(data_dir).ok(),
            hub_db: PathIdentity::of(hub_db).ok(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (data_dir, hub_db);
        StoreIdentity::default()
    }
}

/// Copy the hub store aside before a migration touches it.
///
/// The copy is the engine's `VACUUM INTO`, which writes a consistent snapshot
/// of everything committed regardless of what is still in the source's
/// write-ahead log. It runs before any writer exists in this process and the
/// engine's file lock keeps every other process out, so nothing is writing
/// while it runs. Only the newest three copies are kept.
async fn backup_before_migration(db: &turso::Database, data_dir: &Path, from: i64) -> Result<()> {
    let backups = data_dir.join("backups");
    config::private_dir(&backups)?;

    let target = backups.join(format!("pre-migration-v{from}-{}.db", backup_stamp()));
    // The destination is a string literal to the engine, so a quote in the
    // path is doubled rather than ending it.
    let target_sql = target.to_string_lossy().replace('\'', "''");
    let conn = store::connect(db)?;
    conn.execute(&format!("VACUUM INTO '{target_sql}'"), ())
        .await
        .map_err(store::engine)?;
    config::private_file(&target)?;
    tracing::info!(
        path = %target.display(),
        from,
        to = store::schema::SUPPORTED_MAX,
        "copied the store aside before migrating it"
    );

    prune_backups(&backups, 3);
    Ok(())
}

/// A filename-safe UTC stamp for a backup, distinct within a process.
fn backup_stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond(),
    )
}

/// Remove every pre-migration backup beyond the newest `keep`.
///
/// A copy is a write-ahead-log database, so its `.db-wal` and `.db-shm`
/// sidecars belong to it; a pruned backup goes as a set, never leaving a
/// sidecar of its own behind. A file that cannot be removed is a warning: it
/// costs disk, which the readiness probe watches, but it is not a reason the
/// hub does not start.
fn prune_backups(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_backup_store(&path) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        backups.push((modified, path));
    }
    if backups.len() <= keep {
        return;
    }
    backups.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, stem) in backups.into_iter().skip(keep) {
        for path in [stem.clone(), sidecar(&stem, "-wal"), sidecar(&stem, "-shm")] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "could not remove an old pre-migration backup"
                ),
            }
        }
    }
}

/// Whether a directory entry is a pre-migration backup's database file.
fn is_backup_store(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("pre-migration-") && name.ends_with(".db"))
}

/// The path of one of a store file's sidecars.
fn sidecar(stem: &Path, suffix: &str) -> PathBuf {
    let mut name = stem.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

impl AppState {
    /// Create the data layout and open the store.
    pub async fn open(config: Config) -> Result<Self> {
        // The whole store is private to the operator's account: brains,
        // knowledge pages and artifact blobs all live here. H11.
        config::private_dir(&config.data_dir)?;
        config::private_dir(&config.sessions_dir())?;
        config::private_dir(&config.knowledge_dir())?;
        config::private_dir(&config.artifacts_dir())?;

        let db = store::open_engine(&config.hub_db_path()).await?;

        // A migration changes the store in place. Copy it aside first, and only
        // when there is something to migrate: a second start on an up-to-date
        // store must not leave a backup behind.
        let from = store::schema_version(&db).await?;
        if from < store::schema::SUPPORTED_MAX {
            backup_before_migration(&db, &config.data_dir, from).await?;
        }
        let schema_version = store::migrate(&db).await?;

        // Seed the id high-water mark this process mints against, from the
        // persisted table AND from the newest id the data already holds: the
        // table is written only at a clean shutdown, so a crash after the last
        // mint would otherwise leave it behind. When the wall clock has stepped
        // behind the mark, say so; ids remain ordered because the mint is
        // clamped forward.
        let mut high_water = store::read_id_high_water(&db).await?.unwrap_or(0);
        if let Some(newest) = store::read_newest_event_ms(&db).await? {
            high_water = high_water.max(newest);
        }
        if high_water > 0 {
            store::set_id_high_water(high_water);
            let now = store::millis_now();
            if high_water > now {
                tracing::warn!(
                    high_water_ms = high_water,
                    now_ms = now,
                    behind_ms = high_water - now,
                    "the wall clock is behind the last minted id; new ids are clamped forward"
                );
            }
        }

        // An agent a store from before already holds under a reserved id would
        // sign as the hub itself, so its token goes before anything is served.
        // A store that cannot be made safe this way is not opened. The revoke
        // writes an audit event, so it runs after the id high-water mark is
        // seeded and its id sorts after every id the store holds.
        for agent_id in store::identity::retire_reserved_agents(&db).await? {
            tracing::warn!(
                agent_id = %agent_id,
                "revoked the token of an agent whose id the hub now reserves; \
                 create it again under another id and give it the new token"
            );
        }

        // Captured once the store exists on disk, before any path can move
        // under the running process.
        let store_identity = capture_store_identity(&config.data_dir, &config.hub_db_path());

        // The engine creates the database and its sidecars under whatever umask
        // is in force, so they are hardened after the fact. A sidecar that does
        // not exist yet is not an error.
        let db_path = config.hub_db_path();
        config::private_file(&db_path)?;
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = db_path.clone().into_os_string();
            sidecar.push(suffix);
            config::private_file(Path::new(&sidecar))?;
        }

        // The engine lock is held from here, so no other hub is mid-update over
        // this directory and none is in flight in this one: any content still
        // waiting for a version number or unreferenced by committed metadata
        // was left by an interrupted write or crash. This is housekeeping, so
        // a file it cannot remove is a warning and never a reason the hub does
        // not start.
        match blob::reconcile(&db, &config.data_dir).await {
            Ok(0) => {}
            Ok(reaped) => tracing::info!(
                reaped,
                "reconciled unreferenced artifact content left by an interrupted write"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                "could not reconcile unreferenced artifact content"
            ),
        }

        match store::prune::recover(&db, &config.data_dir).await {
            Ok(0) => {}
            Ok(recovered) => tracing::info!(
                recovered,
                "recovered intermediate prune states from an interrupted sweep"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                "could not recover intermediate prune states"
            ),
        }

        match store::projects::recover(&db, &config.data_dir).await {
            Ok(0) => {}
            Ok(recovered) => tracing::info!(
                recovered,
                "recovered deleting project states from an interrupted delete"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                "could not recover deleting project states"
            ),
        }

        // The session-side twin of the artifact reconcile: a fork or a delete
        // that crashed between its file and its row leaves a brain file no
        // session names. Runs after the prune and project recovery, so a file
        // those two are about to reclaim is not mistaken for an orphan.
        match crate::brain::reconcile_sessions(&db, &config.data_dir).await {
            Ok(0) => {}
            Ok(reaped) => tracing::info!(
                reaped,
                "reconciled unreferenced session brain files left by an interrupted write"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                "could not reconcile unreferenced session brain files"
            ),
        }

        // Fold the log of every finished session into its file once, so a brain
        // that ended while this was not yet automatic is consolidated too. Best
        // effort: a brain that cannot be consolidated is left as it is.
        match store::sessions::ended_unpruned(&db).await {
            Ok(sessions) => {
                let store = BrainStore::new(config.sessions_dir());
                for (project_id, session_id) in sessions {
                    match store.open_existing(&project_id, &session_id).await {
                        Ok(Some(brain)) => {
                            if let Err(err) = brain.checkpoint().await {
                                tracing::warn!(session_id = %session_id, error = %err, "could not checkpoint a finished brain");
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(session_id = %session_id, error = %err, "could not open a finished brain")
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "could not list finished sessions to checkpoint")
            }
        }

        let data_dir = config.data_dir.clone();
        let brain = BrainStore::new(config.sessions_dir());
        let knowledge = BrainStore::new(config.knowledge_dir());
        let auth = Arc::new(Auth::from_config(&config));
        let activity = Arc::new(store::sessions::Activity::new());
        let host = store::storage::host_name(config.node_name.as_deref());
        let integrity_sample = Config::integrity_sample_from_env()?;
        let (ticker, _) = tokio::sync::broadcast::channel(16);
        let notifier = match &config.notify {
            Some(target) => {
                tracing::info!(target = %target.redacted(), interval_secs = target.interval.as_secs(), "notify target configured");
                crate::notify::Notifier::start(target.clone())
            }
            None => crate::notify::Notifier::off(),
        };
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
            notifier,
            enrol_shares: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            integrity_sample,
            store_identity,
        })
    }

    /// Record the share flag for an approved enrolment.
    pub fn set_enrol_share(&self, agent_id: &str, share: bool) {
        if share {
            self.enrol_shares
                .lock()
                .unwrap()
                .insert(agent_id.to_string(), true);
        }
    }

    /// Read the share flag for an enrolment.
    pub fn get_enrol_share(&self, agent_id: &str) -> bool {
        self.enrol_shares
            .lock()
            .unwrap()
            .get(agent_id)
            .copied()
            .unwrap_or(false)
    }

    /// Nudge every stream subscriber to refetch. A send with no subscribers is
    /// not an error; the next subscriber gets the state on its next write.
    pub fn notify(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.ticker.send(Tick::default());
    }

    /// Nudge stream subscribers after a write that left something waiting on
    /// the human: a question, an approval, or an enrolment request. The
    /// operator's notify target, when one is configured, is told too.
    pub fn notify_waiting(&self) {
        self.notify();
        self.notifier.waiting();
    }

    /// Nudge subscribers that one artifact's live version moved.
    ///
    /// The generation is bumped with it: a live write changes artifact bytes on
    /// disk, so the storage and knowledge memos must recompute rather than
    /// serve a figure that is a version behind.
    pub fn notify_live(&self, artifact_id: &str, version: i64, live_rev: i64) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.ticker.send(Tick {
            live: Some(LiveTick {
                artifact_id: artifact_id.to_string(),
                version,
                live_rev,
            }),
        });
    }

    /// Commit every prune whose undo window has passed.
    ///
    /// A committed prune removes the session's own events, so the storage
    /// report's event weights are dropped with them, and the screens are told
    /// to refetch.
    pub async fn sweep_prunes(&self) -> Result<u64> {
        let committed = store::prune::sweep(&self.db, &self.data_dir).await?;
        if committed > 0 {
            self.stats.forget_events();
            self.notify();
        }
        // A live version whose session has gone quiet stops being live. Its
        // row and blob stay, so nothing is destroyed; the artifact's own page
        // reads `current_ver`, which a live write never moves.
        store::artifacts::clear_idle_live(&self.db, &self.config.active_since()).await?;
        Ok(committed)
    }

    /// Resolve every open item whose agent-set deadline has passed, and wake
    /// the waiters when any did.
    ///
    /// The serving hub runs this on a short interval, and a read that shows
    /// whether an item is open runs it first, so an item past its deadline
    /// never reads as open between two sweeps.
    pub async fn settle_deadlines(&self) -> Result<usize> {
        self.settle_deadlines_at(time::OffsetDateTime::now_utc())
            .await
    }

    /// Settle deadlines ahead of a read that shows whether an item is open.
    ///
    /// A failure is logged and the read goes on: the background sweep resolves
    /// the item on its next pass, and a decision on it is refused in its own
    /// transaction meanwhile.
    pub async fn settle_before_read(&self) {
        if let Err(err) = self.settle_deadlines().await {
            tracing::warn!(error = %err, "could not resolve items at their deadline");
        }
    }

    /// Start the background sweep that resolves items at their deadline.
    ///
    /// Every way of serving the store runs it, over HTTP and embedded on stdio,
    /// so an agent waiting on an item is woken by its expiry whichever way it
    /// reached the hub. The cadence is its own and shorter than the prune
    /// sweeper's: a deadline is a promise to the waiting agent, so it is kept
    /// within seconds rather than within a prune window. The caller aborts the
    /// task before it checkpoints the store.
    pub fn spawn_deadline_sweep(&self) -> tokio::task::JoinHandle<()> {
        let settler = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(deadline_interval());
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                match settler.settle_deadlines().await {
                    Ok(0) => {}
                    Ok(settled) => tracing::info!(settled, "resolved items at their deadline"),
                    Err(err) => tracing::warn!(error = %err, "deadline sweep failed"),
                }
            }
        })
    }

    /// As [`Self::settle_deadlines`], as of `now` rather than the wall clock.
    pub async fn settle_deadlines_at(&self, now: time::OffsetDateTime) -> Result<usize> {
        let settled = store::questions::expire_due(&self.db, now).await?;
        if settled > 0 {
            self.notify();
        }
        Ok(settled)
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
    let sweeper_task = tokio::spawn(async move {
        let interval = sweep_interval();
        // The integrity sample is a separate cadence (its own hub setting),
        // so this tick count decides when its turn falls due. The loop is the
        // single place that runs either background writer.
        let integrity_every = integrity_ticks(&sweeper.integrity_sample, interval);
        let mut tick: u64 = 0;
        loop {
            match sweeper.sweep_prunes().await {
                Ok(_) => {}
                Err(err) => tracing::warn!(error = %err, "prune sweep failed"),
            }
            // An abandoned enrolment stops blocking its source once it is old
            // enough, and its request and approval leave with it.
            match crate::store::identity::expire_pending(
                &sweeper.db,
                sweeper.config.enrol_pending_ttl,
            )
            .await
            {
                Ok(0) => {}
                Ok(expired) => tracing::info!(expired, "expired abandoned enrolment requests"),
                Err(err) => tracing::warn!(error = %err, "could not expire pending enrolments"),
            }
            if let Some(every) = integrity_every
                && tick.is_multiple_of(every)
            {
                // Bounded so a large or damaged store cannot pin the loop; a
                // timeout or an error records a failing sample and returns.
                if let Err(err) = sample_integrity_within(&sweeper.db, INTEGRITY_SAMPLE_CAP).await {
                    tracing::warn!(error = %err, "the integrity sample failed");
                }
            }
            tick = tick.wrapping_add(1);
            tokio::time::sleep(interval).await;
        }
    });

    let deadline_task = state.spawn_deadline_sweep();

    let router = http::router(state.clone())
        .merge(mcp::http_router(state.clone()))
        // Applied after the merge, so `/mcp` is counted too: `Router::layer`
        // does not reach a router merged in later.
        .layer(axum::middleware::from_fn(count_http));

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
    let tailnet_task = if tailnet.enabled() {
        let tailnet_router = router.clone();
        Some(tokio::spawn(async move {
            // The embedded endpoint owns its own reconnect loop and takes no
            // shutdown handle, so it is dropped with the process rather than
            // drained: see the graceful shutdown below.
            if let Err(err) = net::serve(&tailnet, tailnet_router).await {
                tracing::error!(error = %err, "tailnet endpoint stopped");
            }
        }))
    } else {
        None
    };

    let listener = tokio::net::TcpListener::bind(bind).await?;
    // The address the listener got, which is not the configured one when the
    // operator asked for port 0.
    let address = listener.local_addr()?;
    tracing::info!(bind = %address, schema_version, "hub listening");

    // One line unconditionally, at whatever log level, so an operator who set
    // nothing still sees the hub start and can read the address a port-0 bind
    // was actually given.
    let admin = if state.config.admin_token.is_some() {
        "admin token configured"
    } else {
        "no admin token; the control surface will reject every request"
    };
    eprintln!(
        "agent-hub serve: listening on http://{address}, data dir {}, {admin}",
        state.config.data_dir.display()
    );

    // SIGTERM or SIGINT stops the accept loop and lets in-flight requests
    // finish. The drain window is applied only AFTER the signal, so a hub that
    // is simply serving is never stopped by it.
    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel();
    let shutdown = async move {
        shutdown_signal().await;
        let _ = stopping_tx.send(());
    };
    let server = std::future::IntoFuture::into_future(
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown),
    );
    let drain = async {
        let _ = stopping_rx.await;
        tokio::time::sleep(SHUTDOWN_DRAIN).await;
        tracing::warn!(
            seconds = SHUTDOWN_DRAIN.as_secs(),
            "the drain window elapsed with connections still open; closing"
        );
    };
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result.map_err(crate::error::Error::from)?,
        _ = drain => {}
    }

    // Stop the background writers before checkpointing, so the fold sees a
    // store no one else is touching.
    sweeper_task.abort();
    deadline_task.abort();
    // The tailnet endpoint is a separate serve with its own reconnect-and-rebuild
    // loop and no shutdown handle, so it is dropped here rather than drained.
    // Stdio MCP is a different path entirely and never reaches this code.
    if let Some(task) = tailnet_task {
        task.abort();
    }

    // Fold the log and record the id high-water mark, so the next start opens
    // a store whose log is already folded and whose ids continue from here.
    if let Err(err) = store::persist_id_high_water(&state.db).await {
        tracing::warn!(error = %err, "could not persist the id high-water mark");
    }
    if let Err(err) = store::checkpoint_hub(&state.db).await {
        tracing::warn!(error = %err, "could not checkpoint the hub store");
    }
    eprintln!("agent-hub serve: stopped");
    Ok(())
}

/// How long a stopping hub waits for in-flight requests to finish.
const SHUTDOWN_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Count one answered request by method and status class.
///
/// The layer wraps the whole router, so it counts every route including the
/// merged `/mcp` service and a request the bearer gate refused. It reads the
/// response status after the handler ran, so a 404 or a 500 is counted as
/// itself rather than as the request that reached it.
async fn count_http(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().as_str().to_owned();
    let response = next.run(request).await;
    crate::metrics::record_http(&method, response.status().as_u16() / 100);
    response
}

/// Resolve when the process is asked to stop.
///
/// SIGTERM is what a supervised container or service manager sends; SIGINT is
/// Ctrl-C. Both drain the same way.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(err) => {
                tracing::warn!(error = %err, "could not watch for SIGTERM");
                return std::future::pending().await;
            }
        };
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(signal) => signal,
            Err(err) => {
                tracing::warn!(error = %err, "could not watch for SIGINT");
                return std::future::pending().await;
            }
        };
        tokio::select! {
            _ = terminate.recv() => tracing::info!("SIGTERM received, draining"),
            _ = interrupt.recv() => tracing::info!("SIGINT received, draining"),
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "could not watch for Ctrl-C");
            return std::future::pending().await;
        }
        tracing::info!("interrupt received, draining");
    }
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

/// How often the deadline sweep runs: every five seconds, or the prune
/// sweeper's interval when a test has set that shorter.
fn deadline_interval() -> std::time::Duration {
    sweep_interval().min(std::time::Duration::from_secs(5))
}

/// The wall-clock cap on one integrity sample. A store too large or damaged to
/// answer inside this is recorded as a failing sample rather than allowed to
/// hold the sweeper loop.
const INTEGRITY_SAMPLE_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// How often the integrity sample falls due, as a whole number of sweeper
/// ticks. `None` disables it (`integrity_sample_secs = 0`). A cadence shorter
/// than one tick runs every tick rather than never.
fn integrity_ticks(sample: &std::time::Duration, tick: std::time::Duration) -> Option<u64> {
    let secs = sample.as_secs();
    if secs == 0 {
        return None;
    }
    Some((secs / tick.as_secs().max(1)).max(1))
}

/// Run one background integrity sample and record its result.
///
/// `PRAGMA quick_check` is bounded and cheaper than `integrity_check`; it skips
/// cross-index verification only. It runs on its own connection, a read
/// snapshot like the readiness probe, and never inside a request. The wall-clock
/// cap bounds the wait, and any timeout or error - or a check that reports
/// anything besides `ok` - is recorded as a failing sample rather than
/// propagated to the caller or allowed to block the loop.
async fn sample_integrity_within(db: &turso::Database, cap: std::time::Duration) -> Result<()> {
    let probe = tokio::time::timeout(cap, integrity_quick_check(db)).await;
    match probe {
        Ok(Ok(true)) => {
            crate::metrics::record_integrity(true);
            Ok(())
        }
        Ok(Ok(false)) => {
            crate::metrics::record_integrity(false);
            Err(crate::error::Error::Engine(
                "quick_check reported a problem".to_string(),
            ))
        }
        Ok(Err(err)) => {
            crate::metrics::record_integrity(false);
            Err(err)
        }
        Err(_) => {
            crate::metrics::record_integrity(false);
            Err(crate::error::Error::Engine(format!(
                "quick_check did not answer within {cap:?}"
            )))
        }
    }
}

/// Run `PRAGMA quick_check` on a fresh connection and report whether it said
/// `ok`. A row that reports anything else is a failure; the check is stepped to
/// completion so a multi-row answer is fully read.
async fn integrity_quick_check(db: &turso::Database) -> Result<bool> {
    let conn = store::connect(db)?;
    let mut rows = conn
        .query("PRAGMA quick_check", ())
        .await
        .map_err(store::engine)?;
    let mut ok = true;
    while let Some(row) = rows.next().await.map_err(store::engine)? {
        match row.get_value(0).map_err(store::engine)? {
            turso::Value::Text(line) if line == "ok" => {}
            _ => ok = false,
        }
    }
    Ok(ok)
}
