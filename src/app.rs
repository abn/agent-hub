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
    /// Held by the one online backup that may run at a time.
    pub backup_running: Arc<tokio::sync::Mutex<()>>,
    /// One gate per session: brain and artifact writes hold it shared, end and
    /// every prune path hold it exclusive.
    pub session_gates: SessionGates,
    /// Token hashes revoked or replaced. Every open stream registered under a
    /// hash closes when it is sent.
    pub revocations: tokio::sync::broadcast::Sender<String>,
}

/// Per-session gates, created on first use and dropped when unused.
///
/// Lock order: a session gate, then a brain file lock, then the store write
/// turn. Never take a gate while holding the write turn; a batch takes its
/// gates in session-id order.
#[derive(Clone, Default)]
pub struct SessionGates {
    gates: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, std::sync::Weak<tokio::sync::RwLock<()>>>,
        >,
    >,
}

impl SessionGates {
    fn gate(&self, session_id: &str) -> Arc<tokio::sync::RwLock<()>> {
        let mut gates = self
            .gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(gate) = gates.get(session_id).and_then(std::sync::Weak::upgrade) {
            return gate;
        }
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(tokio::sync::RwLock::new(()));
        gates.insert(session_id.to_string(), Arc::downgrade(&gate));
        gate
    }

    /// Hold the gate shared, for a brain or artifact write in this session.
    pub async fn shared(&self, session_id: &str) -> tokio::sync::OwnedRwLockReadGuard<()> {
        self.gate(session_id).read_owned().await
    }

    /// Hold the gate exclusive, for ending or pruning this session.
    pub async fn exclusive(&self, session_id: &str) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.gate(session_id).write_owned().await
    }

    /// Hold several gates exclusive, taken in session-id order.
    pub async fn exclusive_all(
        &self,
        session_ids: &[String],
    ) -> Vec<tokio::sync::OwnedRwLockWriteGuard<()>> {
        let mut ids: Vec<&String> = session_ids.iter().collect();
        ids.sort();
        ids.dedup();
        let mut held = Vec::with_capacity(ids.len());
        for id in ids {
            held.push(self.exclusive(id).await);
        }
        held
    }
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

/// Copy the whole store aside before a migration touches it.
///
/// The set is an offline backup (`hub.db`, every session and project brain,
/// the knowledge bases and the artifact blobs), written under
/// `backups/.staging-<stamp>` and renamed into place, so a set is complete or
/// absent. Rolling back restores it whole. Only the newest three sets are
/// kept, and older ones go only once the new one is in place.
async fn backup_before_migration(data_dir: &Path, from: i64) -> Result<()> {
    let backups = data_dir.join("backups");
    config::private_dir(&backups)?;
    let stamp = store::filename_stamp();
    let staging = backups.join(format!(".staging-{stamp}"));
    let target = backups.join(format!("pre-migration-v{from}-{stamp}"));
    if let Err(err) = crate::ops::backup::backup_aside(data_dir, &staging).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(store::migrate::stop_with(format!(
            "could not copy the store and session brains aside before migrating: {err}"
        )));
    }
    std::fs::rename(&staging, &target)?;
    store::migrate::sync_dir(&backups)?;
    tracing::info!(
        path = %target.display(),
        from,
        to = store::schema::SUPPORTED_MAX,
        "copied the store aside before migrating it"
    );

    prune_backups(&backups, 3);
    Ok(())
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
        if stem.is_dir() {
            if let Err(err) = std::fs::remove_dir_all(&stem) {
                tracing::warn!(
                    path = %stem.display(),
                    error = %err,
                    "could not remove an old pre-migration backup"
                );
            }
            continue;
        }
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

/// Whether a directory entry is a pre-migration backup: a set (a
/// directory) or, from before sets, a single database file.
fn is_backup_store(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("pre-migration-") && (name.ends_with(".db") || path.is_dir())
        })
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
        let data_dir = config.data_dir.clone();
        let hub_db = config.hub_db_path();
        // The whole store is private to the operator's account: brains,
        // knowledge pages and artifact blobs all live here. H11. A fresh
        // directory is laid out at once; an existing store is probed and
        // planned first, so a start that stops changes nothing in it.
        let harden = || -> Result<()> {
            config::private_dir(&config.data_dir)?;
            config::private_dir(&config.sessions_dir())?;
            config::private_dir(&config.knowledge_dir())?;
            config::private_dir(&config.artifacts_dir())?;
            Ok(())
        };
        let existing = hub_db.is_file();
        if !existing {
            harden()?;
        }

        let mut db = store::open_engine(&hub_db).await?;
        let from = store::schema_version(&db).await?;
        let upgrading = existing && from > 0 && from < store::schema::SUPPORTED_MAX;
        if upgrading {
            let backups = data_dir.join("backups");
            let had_backups = backups.exists();
            let plan = store::migrate::preflight_in(&db, &data_dir, &backups).await;
            if !had_backups {
                let _ = std::fs::remove_dir(&backups);
            }
            let plan = plan?;
            if !plan.is_clean() {
                return Err(store::migrate::stop_error(&plan));
            }
        }
        harden()?;

        // A migration changes the store in place. Copy the whole store aside
        // first, and only when there is something to migrate: a second start
        // on an up-to-date store must not leave a backup behind, and a fresh
        // one has nothing to keep.
        if upgrading {
            drop(db);
            backup_before_migration(&data_dir, from).await?;
            db = store::open_engine(&hub_db).await?;
        }
        let schema_version = store::migrate_in(&db, Some(&data_dir)).await?;
        // Merged sessions' brains are folded in before anything is served.
        store::migrate::run_merges(&db, &data_dir, None).await?;

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

        // A member a store from before already holds under a reserved id would
        // sign as the hub itself, so its token goes before anything is served.
        // A store that cannot be made safe this way is not opened. The revoke
        // writes an audit event, so it runs after the id high-water mark is
        // seeded and its id sorts after every id the store holds.
        for member in store::identity::retire_reserved_members(&db).await? {
            tracing::warn!(
                member = %member,
                "revoked the token of a member whose id the hub now reserves; \
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

        let brain = BrainStore::new(config.sessions_dir());
        let knowledge = BrainStore::new(config.knowledge_dir());
        let auth = Arc::new(Auth::from_config(&config));
        let activity = Arc::new(store::sessions::Activity::new());
        let host = store::storage::host_name(config.node_name.as_deref());
        let integrity_sample = Config::integrity_sample_from_env()?;
        let (ticker, _) = tokio::sync::broadcast::channel(16);
        let (revocations, _) = tokio::sync::broadcast::channel(64);
        let notifier = match &config.notify {
            Some(target) => {
                tracing::info!(target = %target.redacted(), interval_secs = target.interval.as_secs(), "notify target configured");
                crate::notify::Notifier::start(target.clone())
            }
            None => crate::notify::Notifier::off(),
        };
        let state = Self {
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
            backup_running: Arc::new(tokio::sync::Mutex::new(())),
            session_gates: SessionGates::default(),
            revocations,
        };
        // A brain write that crashed between its file and its index left its
        // session marked; its search rows are rebuilt from the file.
        let reindexed = crate::brain::shared::reindex_pending(&state).await?;
        if reindexed > 0 {
            tracing::info!(
                reindexed,
                "rebuilt the search rows of interrupted brain writes"
            );
        }
        Ok(state)
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

    /// Tell every open stream a token held that the token is gone. The caller
    /// sends after its store change committed, so a stream that checks again
    /// finds the refusal. No receiver is not a failure: nothing was listening.
    pub fn announce_revoked(&self, hashes: Vec<String>) {
        for hash in hashes {
            let _ = self.revocations.send(hash);
        }
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
        let committed =
            store::prune::sweep_gated(&self.db, &self.data_dir, &self.session_gates).await?;
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
    run_with(config, |_| {}).await.map(|_| ())
}

/// How a hub that served and stopped without error left its store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped {
    /// True only when every request drained inside the window, the background
    /// writers had finished, and the store's checkpoint returned success: the
    /// state a next start can trust without a check.
    pub checkpointed: bool,
}

/// As [`run`], telling `on_bound` the address the listener got as soon as the
/// bind succeeds, before the first request is served.
///
/// The lock holder's record carries that address so `status` and `start` can name it.
/// The stop signals are watched before the store opens, so by the time the
/// address is published the hub already answers a stop.
pub async fn run_with(
    config: Config,
    on_bound: impl FnOnce(std::net::SocketAddr),
) -> Result<Stopped> {
    let stop_signal = watch_stop_signals();
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
                Ok((0, _)) => {}
                Ok((expired, hashes)) => {
                    sweeper.announce_revoked(hashes);
                    tracing::info!(expired, "expired abandoned enrolment requests")
                }
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
    on_bound(address);

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
        stop_signal.await;
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
    let mut drained = true;
    tokio::select! {
        result = &mut server => result.map_err(crate::error::Error::from)?,
        _ = drain => drained = false,
    }

    // Stop the background writers before checkpointing, so the fold sees a
    // store no one else is touching. An abort only takes effect at the task's
    // next await point, so each handle is awaited until the task is gone.
    sweeper_task.abort();
    deadline_task.abort();
    let _ = sweeper_task.await;
    let _ = deadline_task.await;
    // The tailnet endpoint is a separate serve with its own reconnect-and-rebuild
    // loop and no shutdown handle, so it is dropped here rather than drained.
    // Stdio MCP is a different path entirely and never reaches this code.
    if let Some(task) = tailnet_task {
        task.abort();
        let _ = task.await;
    }

    let checkpointed = fold_on_stop(&state.db, drained).await;
    tracing::info!(checkpointed, "hub stopped");
    eprintln!("agent-hub serve: stopped");
    Ok(Stopped { checkpointed })
}

/// Fold the log and record the id high-water mark, so the next start opens a
/// store whose log is already folded and whose ids continue from here.
///
/// True only when `drained` (every request finished inside the window) and
/// both steps succeeded, including a checkpoint that reported no busy log: the
/// state a stopping hub may record as a clean stop.
#[doc(hidden)]
pub async fn fold_on_stop(db: &turso::Database, drained: bool) -> bool {
    let mut checkpointed = drained;
    if let Err(err) = store::persist_id_high_water(db).await {
        tracing::warn!(error = %err, "could not persist the id high-water mark");
        checkpointed = false;
    }
    if let Err(err) = store::checkpoint_hub(db).await {
        tracing::warn!(error = %err, "could not checkpoint the hub store");
        checkpointed = false;
    }
    checkpointed
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

/// Start watching for the signals that stop the hub, and return the future
/// that resolves when one arrives.
///
/// SIGTERM is what a supervised container or service manager sends; SIGINT is
/// Ctrl-C. Both drain the same way. The handlers are installed here, when the
/// function is called, not when the future is first polled, so a signal that
/// lands while the store is still opening is held and acted on once it is.
fn watch_stop_signals() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    let signals = {
        use tokio::signal::unix::{SignalKind, signal};
        let watch = |kind: SignalKind, name: &str| match signal(kind) {
            Ok(stream) => Some(stream),
            Err(err) => {
                tracing::warn!(error = %err, "could not watch for {name}");
                None
            }
        };
        (
            watch(SignalKind::terminate(), "SIGTERM"),
            watch(SignalKind::interrupt(), "SIGINT"),
        )
    };
    #[cfg(not(unix))]
    let event_stop = windows_stop_event();
    async move {
        #[cfg(unix)]
        {
            let (mut terminate, mut interrupt) = signals;
            let wait = |stream: &mut Option<tokio::signal::unix::Signal>| {
                let stream = stream.take();
                async move {
                    match stream {
                        Some(mut stream) => {
                            stream.recv().await;
                        }
                        None => std::future::pending().await,
                    }
                }
            };
            tokio::select! {
                _ = wait(&mut terminate) => tracing::info!("SIGTERM received, draining"),
                _ = wait(&mut interrupt) => tracing::info!("SIGINT received, draining"),
            }
        }
        #[cfg(not(unix))]
        {
            // The watch lives as long as this future: when the future ends or
            // is dropped, the waiting thread is told to quit and the event is
            // closed, so a later serve in this process can create it again.
            let (receiver, _watch) = match event_stop {
                Some((receiver, watch)) => (Some(receiver), Some(watch)),
                None => (None, None),
            };
            let event = async {
                match receiver {
                    // A closed channel is a failed wait, already logged, not a
                    // request to stop.
                    Some(stop) => {
                        if stop.await.is_err() {
                            std::future::pending::<()>().await;
                        }
                    }
                    None => std::future::pending().await,
                }
            };
            let interrupt = async {
                if let Err(err) = tokio::signal::ctrl_c().await {
                    tracing::warn!(error = %err, "could not watch for Ctrl-C");
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = interrupt => tracing::info!("interrupt received, draining"),
                () = event => tracing::info!("stop event received, draining"),
            }
        }
    }
}

/// What [`windows_stop_event`] hands back: the channel that fires when the
/// event is set, and the watch that keeps the waiting thread alive.
#[cfg(not(unix))]
type EventStop = Option<(tokio::sync::oneshot::Receiver<()>, Box<dyn Send>)>;

/// Create this process's stop event and watch it from a thread of its own.
///
/// A hub started detached or by the Windows service has no console to send
/// Ctrl-C to, and `TerminateProcess` would skip the checkpoint, so
/// `agent-hub stop` sets the named event instead. The thread is not a runtime
/// task, so a dropped runtime does not wait for it; dropping the returned watch
/// tells it to quit and waits until it has.
#[cfg(windows)]
fn windows_stop_event() -> EventStop {
    let event = match stop_event::StopEvent::create(std::process::id()) {
        Ok(event) => event,
        Err(err) => {
            tracing::warn!(error = %err, "could not create the stop event; only Ctrl-C will stop the hub");
            return None;
        }
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    match event.watch(move || {
        let _ = sender.send(());
    }) {
        Ok(watch) => Some((receiver, Box::new(watch))),
        Err(err) => {
            tracing::warn!(error = %err, "could not watch the stop event; only Ctrl-C will stop the hub");
            None
        }
    }
}

#[cfg(all(not(unix), not(windows)))]
fn windows_stop_event() -> EventStop {
    None
}

/// The named event that stops a hub where there is no SIGTERM.
///
/// `serve` creates `Local\agent-hub-stop-<pid>` (and `Global\` beside it, for a
/// stop that comes from another logon session than a boot-started hub's) and
/// waits on it next to Ctrl-C. `agent-hub stop` and the Windows supervisor open
/// it by the pid they hold and set it, which drains the hub and checkpoints the
/// store the way SIGTERM does. The names are plain strings so they can be
/// checked on any platform; the event itself exists on Windows only.
pub mod stop_event {
    /// The event a process of this pid waits on, in the caller's session.
    pub fn local_name(pid: u32) -> String {
        format!("Local\\agent-hub-stop-{pid}")
    }

    /// The same event in the global namespace.
    pub fn global_name(pid: u32) -> String {
        format!("Global\\agent-hub-stop-{pid}")
    }

    /// What a wait on the stop event came to.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Waited {
        /// Someone set the event: stop.
        Set,
        TimedOut,
        /// The owner of the wait called it off.
        Cancelled,
        /// The wait itself failed. It is not a request to stop.
        Failed,
    }

    const WAIT_TIMEOUT: u32 = 0x0000_0102;

    /// Read the result of `WaitForMultipleObjects` over `stops` stop handles,
    /// followed by a cancel handle when `cancel` is true. Anything that is not
    /// one of those handles being set, or the timeout, is a failure: an event
    /// is never abandoned, so `WAIT_ABANDONED_*` and `WAIT_FAILED` both are.
    pub fn classify(code: u32, stops: usize, cancel: bool) -> Waited {
        let stops = stops as u32;
        if code < stops {
            Waited::Set
        } else if cancel && code == stops {
            Waited::Cancelled
        } else if code == WAIT_TIMEOUT {
            Waited::TimedOut
        } else {
            Waited::Failed
        }
    }

    #[cfg(windows)]
    pub use imp::{StopEvent, Watch};

    #[cfg(windows)]
    mod imp {
        use std::io;
        use std::thread::JoinHandle;
        use std::time::Duration;

        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE,
        };
        use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
        use windows_sys::Win32::System::Threading::{
            CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
            WaitForMultipleObjects,
        };

        use super::{Waited, classify};

        fn wide(name: &str) -> Vec<u16> {
            name.encode_utf16().chain(std::iter::once(0)).collect()
        }

        fn millis(timeout: Option<Duration>) -> u32 {
            match timeout {
                Some(limit) => u32::try_from(limit.as_millis())
                    .unwrap_or(INFINITE - 1)
                    .min(INFINITE - 1),
                None => INFINITE,
            }
        }

        /// One or two handles to the same logical stop event. The handles are
        /// kept as integers so the value can move to the thread that waits.
        pub struct StopEvent {
            handles: Vec<usize>,
        }

        impl StopEvent {
            /// Create the event for `pid`. The session-local one is required,
            /// the global one is taken when the system allows it.
            pub fn create(pid: u32) -> io::Result<Self> {
                let mut handles = vec![create_one(&super::local_name(pid))?];
                if let Ok(global) = create_one(&super::global_name(pid)) {
                    handles.push(global);
                }
                Ok(Self { handles })
            }

            /// Open the event of the process `pid`, or `None` when it has none
            /// (it is not a hub, or it is gone).
            pub fn open(pid: u32) -> io::Result<Option<Self>> {
                let mut handles = Vec::new();
                let mut refused = None;
                for name in [super::local_name(pid), super::global_name(pid)] {
                    match open_one(&name) {
                        Ok(handle) => handles.push(handle),
                        Err(err) if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {}
                        Err(err) => refused = Some(err),
                    }
                }
                match (handles.is_empty(), refused) {
                    (false, _) => Ok(Some(Self { handles })),
                    (true, Some(err)) => Err(err),
                    (true, None) => Ok(None),
                }
            }

            /// Set the event: the process waiting on it starts to stop.
            pub fn signal(&self) -> io::Result<()> {
                for handle in &self.handles {
                    // SAFETY: the handle is open for as long as `self` is.
                    if unsafe { SetEvent(*handle as HANDLE) } == 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            }

            fn wait_with(&self, cancel: Option<usize>, timeout: Option<Duration>) -> Waited {
                let mut raw: Vec<HANDLE> = self.handles.iter().map(|h| *h as HANDLE).collect();
                if let Some(cancel) = cancel {
                    raw.push(cancel as HANDLE);
                }
                // SAFETY: `raw` holds open handles and its length is the count.
                let code = unsafe {
                    WaitForMultipleObjects(raw.len() as u32, raw.as_ptr(), 0, millis(timeout))
                };
                classify(code, self.handles.len(), cancel.is_some())
            }

            /// Wait until the event is set (`Ok(true)`) or `timeout` has passed
            /// (`Ok(false)`; `None` waits for ever). A wait that fails is an
            /// error, never a stop.
            pub fn wait(&self, timeout: Option<Duration>) -> io::Result<bool> {
                match self.wait_with(None, timeout) {
                    Waited::Set => Ok(true),
                    Waited::TimedOut | Waited::Cancelled => Ok(false),
                    Waited::Failed => Err(io::Error::last_os_error()),
                }
            }

            /// Wait for the event on a thread of its own and call `on_set` once
            /// it is set. A failed wait is logged and calls nothing. Dropping
            /// the returned [`Watch`] ends the thread, which closes the event.
            pub fn watch(self, on_set: impl FnOnce() + Send + 'static) -> io::Result<Watch> {
                // SAFETY: an unnamed manual-reset event; the result is checked.
                let cancel = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
                if cancel.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let cancel = cancel as usize;
                let thread = std::thread::Builder::new()
                    .name("stop-event".to_string())
                    .spawn(move || match self.wait_with(Some(cancel), None) {
                        Waited::Set => on_set(),
                        Waited::Cancelled | Waited::TimedOut => {}
                        Waited::Failed => tracing::warn!(
                            error = %io::Error::last_os_error(),
                            "could not wait on the stop event; only Ctrl-C will stop the hub"
                        ),
                    });
                match thread {
                    Ok(thread) => Ok(Watch {
                        cancel,
                        thread: Some(thread),
                    }),
                    Err(err) => {
                        // SAFETY: created above and closed once.
                        unsafe { CloseHandle(cancel as HANDLE) };
                        Err(err)
                    }
                }
            }
        }

        impl Drop for StopEvent {
            fn drop(&mut self) {
                for handle in &self.handles {
                    // SAFETY: each handle was opened by this value and is closed once.
                    unsafe { CloseHandle(*handle as HANDLE) };
                }
            }
        }

        /// The thread waiting on a stop event. Dropping it calls the wait off
        /// and joins the thread, so the event is closed when the drop returns.
        pub struct Watch {
            cancel: usize,
            thread: Option<JoinHandle<()>>,
        }

        impl Drop for Watch {
            fn drop(&mut self) {
                // SAFETY: the cancel event is open until the end of this drop.
                unsafe { SetEvent(self.cancel as HANDLE) };
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                // SAFETY: created in `watch` and closed once, after the thread
                // that waited on it has ended.
                unsafe { CloseHandle(self.cancel as HANDLE) };
            }
        }

        fn create_one(name: &str) -> io::Result<usize> {
            let wide = wide(name);
            // SAFETY: `wide` is a NUL-terminated string that outlives the call.
            let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: reads the thread's last error right after the call.
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                // Something else made an event of this name first: it is not ours.
                // SAFETY: the handle was just returned and is closed once.
                unsafe { CloseHandle(handle) };
                return Err(io::Error::from_raw_os_error(ERROR_ALREADY_EXISTS as i32));
            }
            Ok(handle as usize)
        }

        fn open_one(name: &str) -> io::Result<usize> {
            let wide = wide(name);
            // SAFETY: `wide` is a NUL-terminated string that outlives the call.
            let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, wide.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(handle as usize)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_names_carry_the_pid_in_both_namespaces() {
            assert_eq!(local_name(4242), "Local\\agent-hub-stop-4242");
            assert_eq!(global_name(4242), "Global\\agent-hub-stop-4242");
        }

        #[test]
        fn only_a_set_stop_handle_is_a_stop() {
            // Two stop handles (local and global) and a cancel handle.
            assert_eq!(classify(0, 2, true), Waited::Set);
            assert_eq!(classify(1, 2, true), Waited::Set);
            assert_eq!(classify(2, 2, true), Waited::Cancelled);
            assert_eq!(classify(0x102, 2, true), Waited::TimedOut);
            // WAIT_FAILED and an abandoned wait are failures, never a stop.
            assert_eq!(classify(0xFFFF_FFFF, 2, true), Waited::Failed);
            assert_eq!(classify(0x80, 2, true), Waited::Failed);
            // Without a cancel handle, the index after the stops is no cancel.
            assert_eq!(classify(1, 1, false), Waited::Failed);
            assert_eq!(classify(0, 1, false), Waited::Set);
        }
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

#[cfg(test)]
mod gate_tests {
    use super::SessionGates;

    #[tokio::test]
    async fn an_exclusive_hold_waits_for_shared_writers_of_its_session_only() {
        let gates = SessionGates::default();
        let writer = gates.shared("s1").await;
        let other =
            tokio::time::timeout(std::time::Duration::from_millis(50), gates.exclusive("s2")).await;
        assert!(other.is_ok(), "another session's gate is free");
        let blocked =
            tokio::time::timeout(std::time::Duration::from_millis(50), gates.exclusive("s1")).await;
        assert!(blocked.is_err(), "end waits for the write in flight");
        drop(writer);
        let held = gates
            .exclusive_all(&["s1".to_string(), "s1".to_string()])
            .await;
        assert_eq!(held.len(), 1);
    }
}
