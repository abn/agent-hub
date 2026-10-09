//! Session brain handles and the per-session write lock table.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use agentfs_sdk::{AgentFS, AgentFSOptions};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{Error, Result};

/// How a value is stored in the AgentFS key-value store.
///
/// The store serialises values as JSON. Text stays text so a brain file is
/// readable, and anything that is not valid UTF-8 falls back to a JSON array
/// of bytes so arbitrary content still round-trips exactly.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum StoredValue {
    Text(String),
    Bytes(Vec<u8>),
}

impl StoredValue {
    fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::Text(text.to_string()),
            Err(_) => Self::Bytes(bytes.to_vec()),
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Text(text) => text.into_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

/// One entry in a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// The namespaced path, as a brain tool addresses it.
    pub path: String,
    /// What is stored there.
    #[serde(rename = "type")]
    pub kind: EntryKind,
    /// Bytes stored at the entry.
    pub size_bytes: i64,
}

/// What a listed entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A key-value entry.
    Key,
    /// A regular file.
    File,
    /// A directory.
    Dir,
}

/// Whether a write brings in an entry's newest verification, from the bytes
/// it replaces, when there are any, and the bytes it stores.
pub type Verifies = fn(Option<&[u8]>, &[u8]) -> bool;

/// Who made a write and what it was, as the write log records it.
#[derive(Debug, Clone, Copy)]
pub struct Stamp<'a> {
    /// The operation, such as `kb.put`.
    pub op: &'a str,
    /// The authenticated actor, never a value the caller supplied.
    pub actor: &'a str,
    /// Asked under the write lock, and the answer goes on the log row. `None`
    /// for a store that has no such notion.
    pub verifies: Option<Verifies>,
}

/// One row of the write log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRecord {
    /// The row id, which is also the log's order and its paging cursor.
    pub id: i64,
    pub op: String,
    pub path: String,
    pub actor: String,
    /// Unix seconds.
    pub at: i64,
    /// The version the write stored. A delete stores none.
    pub version: Option<String>,
    /// The bytes the write stored. A delete stores none.
    pub bytes: Option<i64>,
    /// Whether the write brought in the entry's newest verification, as its
    /// [`Stamp`] judged it when the write landed.
    pub verifies: bool,
}

/// One page and one version of it, as [`Brain::page_version`] read them.
#[derive(Debug, Clone, Default)]
pub struct PageVersionState {
    pub now: Option<Vec<u8>>,
    /// The page's log rows, oldest first.
    pub writes: Vec<WriteRecord>,
    pub mark: Option<ForgetMark>,
    /// The kept bytes of the version, when the page does not hold it now.
    pub kept: Option<Vec<u8>>,
}

/// Where one page's history was forgotten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetMark {
    /// The newest log row of the page when it was forgotten.
    pub through_id: i64,
    /// What the page held then, which a purge keeps.
    pub kept_version: Option<String>,
}

impl ForgetMark {
    /// Whether a log row of the page still leads to bytes: it is newer than
    /// the purge, or names what the page held when it was purged.
    pub fn readable(&self, id: i64, version: Option<&str>) -> bool {
        id > self.through_id
            || version.is_some_and(|version| Some(version) == self.kept_version.as_deref())
    }
}

/// What forgetting one page's history removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forgotten {
    /// Kept versions whose bytes were removed.
    pub versions: usize,
    /// The bytes those versions held.
    pub bytes: i64,
    /// The newest log row the page is now forgotten through, or `None` when
    /// the log names no write to the page.
    pub through_id: Option<i64>,
    /// Kept versions no log row of any page names, swept on the way: what a
    /// crash between keeping a write's bytes and logging it leaves behind.
    /// No history can read them.
    pub unreferenced: usize,
    pub unreferenced_bytes: i64,
}

/// What the write log says last about each path.
#[derive(Debug, Default)]
pub struct LastWrites {
    /// The newest write to every path the log names.
    pub by_path: HashMap<String, WriteRecord>,
    /// The newest write to each path that the caller counts as verifying.
    pub verifying: HashMap<String, WriteRecord>,
    /// The newest write of all.
    pub newest: Option<WriteRecord>,
}

/// Which rows of the write log a reader wants. Every field narrows.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteFilter<'a> {
    pub path: Option<&'a str>,
    /// A directory: the rows for entries under it.
    pub prefix: Option<&'a str>,
    pub actor: Option<&'a str>,
    pub op: Option<&'a str>,
}

impl WriteFilter<'_> {
    fn matches(&self, record: &WriteRecord) -> bool {
        self.path.is_none_or(|path| record.path == path)
            && self
                .prefix
                .is_none_or(|prefix| is_under(prefix, &record.path))
            && self.actor.is_none_or(|actor| record.actor == actor)
            && self.op.is_none_or(|op| record.op == op)
    }
}

/// Whether a path is a directory or something inside it.
pub fn is_under(directory: &str, path: &str) -> bool {
    path.strip_prefix(directory.trim_end_matches('/'))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// One page of the write log.
#[derive(Debug, Clone, Default)]
pub struct WriteLogPage {
    pub rows: Vec<WriteRecord>,
    /// Rows the filter matches in the whole log, not on this page.
    pub total: usize,
    /// The cursor for the next page, when there is one.
    pub next_before: Option<i64>,
    /// Whether the filter matches rows this page does not carry.
    pub truncated: bool,
}

/// The two path namespaces a brain exposes.
enum Namespace<'a> {
    Kv(&'a str),
    Fs(&'a str),
}

/// The process-wide table of per-session-file write locks.
///
/// The single-writer invariant is per file, across every store instance, so
/// two handles to one session file serialise even when they come from
/// different stores. Keying by the file path as constructed from the store
/// root keeps distinct stores over distinct roots independent.
fn locks() -> &'static Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Factory for session brains.
///
/// Cheap to clone. Every clone, and every store, shares one lock table, so two
/// writers to one session file serialise.
#[derive(Clone)]
pub struct BrainStore {
    root: PathBuf,
}

impl BrainStore {
    /// Create a store rooted at the directory that holds the session files.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// A store for the sessions directory under a data directory.
    ///
    /// The prune sweep uses this so its file removal takes the same session
    /// lock as the store the rest of the hub writes through.
    pub fn for_data_dir(data_dir: &Path) -> Self {
        Self::new(data_dir.join("sessions"))
    }

    /// A store for the knowledge directory under a data directory.
    ///
    /// One file per project, opened under the same wrapper and the same
    /// per-file write lock as a session brain, so the knowledge base needs no
    /// type, locking or removal code of its own.
    pub fn for_knowledge(data_dir: &Path) -> Self {
        Self::new(knowledge_dir(data_dir))
    }

    /// The directory holding one brain file per session, under its project.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the brain file for a session, under its project.
    ///
    /// The path is stable for a given project and session, which is what lets
    /// a resume find the same state.
    pub fn brain_path(&self, project_id: &str, session_id: &str) -> Result<PathBuf> {
        validate_project_id(project_id)?;
        validate_session_id(session_id)?;
        Ok(self.root.join(project_id).join(format!("{session_id}.db")))
    }

    /// Open the brain for a session, locating or creating its file.
    pub async fn open(&self, project_id: &str, session_id: &str) -> Result<Brain> {
        self.open_live(project_id, session_id, async || Ok(()))
            .await
    }

    /// Open the brain for a session once `alive` confirms it is still there.
    ///
    /// `alive` runs under the session's write lock, which a prune also takes
    /// before it removes the file. A caller that checks liveness on its own
    /// checks it before the lock, so a sweep can land in between and the open
    /// recreates the file the sweep just removed; re-checking here is what
    /// orders the two.
    pub async fn open_live(
        &self,
        project_id: &str,
        session_id: &str,
        alive: impl AsyncFnOnce() -> Result<()>,
    ) -> Result<Brain> {
        std::fs::create_dir_all(self.root.join(project_id))?;
        let brain = self.open_under_lock(project_id, session_id, true, alive);
        Ok(brain
            .await?
            .expect("an open that may create always yields a brain"))
    }

    /// Open the brain for a session only when its file already exists.
    ///
    /// A read path uses this so it never creates a brain for a session whose
    /// file is absent; `None` means there is nothing stored yet.
    pub async fn open_existing(&self, project_id: &str, session_id: &str) -> Result<Option<Brain>> {
        self.open_existing_live(project_id, session_id, async || Ok(()))
            .await
    }

    /// Open the brain for a session only when its file already exists, checking
    /// liveness under the session lock.
    pub async fn open_existing_live(
        &self,
        project_id: &str,
        session_id: &str,
        alive: impl AsyncFnOnce() -> Result<()>,
    ) -> Result<Option<Brain>> {
        self.open_under_lock(project_id, session_id, false, alive)
            .await
    }

    /// The one place a brain file is opened, always under the session lock.
    ///
    /// Opening is not a read: the SDK creates its tables, so two first opens
    /// of one file would race on the engine's file lock, and an open of a
    /// missing file creates it. Both the liveness check and the existence
    /// test therefore run under the lock a prune takes to remove the file; a
    /// test made before the lock can be stale by the time the open runs, and
    /// the open would then bring a pruned brain back with no session row left
    /// for any later sweep to find.
    async fn open_under_lock(
        &self,
        project_id: &str,
        session_id: &str,
        create: bool,
        alive: impl AsyncFnOnce() -> Result<()>,
    ) -> Result<Option<Brain>> {
        let path = self.brain_path(project_id, session_id)?;
        let lock = lock_for(&path);
        let engine_path = path
            .to_str()
            .ok_or_else(|| Error::Config("session brain path is not valid UTF-8".to_string()))?
            .to_string();

        let guard = lock.lock().await;
        alive().await?;
        if !create && !path.exists() {
            return Ok(None);
        }
        let agent = AgentFS::open(AgentFSOptions {
            path: Some(engine_path),
            ..Default::default()
        })
        .await
        .map_err(engine_error)?;
        if session_id == KNOWLEDGE_FILE {
            create_versions_table(&agent).await?;
        }
        drop(guard);

        Ok(Some(Brain {
            agent,
            lock,
            path,
            session_id: session_id.to_string(),
        }))
    }

    /// Quarantine a session's brain file while holding its write lock.
    ///
    /// The active file is moved atomically to `.quarantine` so concurrent or
    /// subsequent opens cannot find it, while keeping data on disk until
    /// metadata deletion commits.
    pub async fn quarantine(&self, project_id: &str, session_id: &str) -> Result<bool> {
        let path = self.brain_path(project_id, session_id)?;
        let lock = lock_for(&path);
        let _guard = lock.lock().await;

        let q_path = quarantine_path(&path);
        if !path.exists() {
            return Ok(q_path.exists());
        }

        if path.is_dir() {
            let err = std::fs::remove_file(&path).unwrap_err();
            return Err(err.into());
        }

        std::fs::rename(&path, &q_path)?;
        for suffix in SIDECAR_SUFFIXES {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            let sidecar_path = PathBuf::from(sidecar);
            if sidecar_path.exists() {
                let q_sidecar = quarantine_path(&sidecar_path);
                if let Err(err) = std::fs::rename(&sidecar_path, &q_sidecar) {
                    tracing::warn!(
                        sidecar = %sidecar_path.display(),
                        error = %err,
                        "could not quarantine a brain sidecar"
                    );
                }
            }
        }
        Ok(true)
    }

    /// Remove a session's quarantined brain file while holding its write lock.
    pub async fn remove_quarantine(&self, project_id: &str, session_id: &str) -> Result<()> {
        let path = self.brain_path(project_id, session_id)?;
        let lock = lock_for(&path);
        let _guard = lock.lock().await;

        let q_path = quarantine_path(&path);
        let _ = remove_if_present(&q_path);
        for suffix in SIDECAR_SUFFIXES {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            let q_sidecar = quarantine_path(&PathBuf::from(sidecar));
            let _ = remove_if_present(&q_sidecar);
        }
        let _ = remove_if_present(&path);
        for suffix in SIDECAR_SUFFIXES {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            let _ = remove_if_present(&PathBuf::from(sidecar));
        }
        Ok(())
    }

    /// Remove any orphaned `.quarantine` files across the sessions tree.
    pub fn clean_all_quarantine(sessions_root: &Path) -> Result<usize> {
        let mut count = 0;
        if !sessions_root.exists() {
            return Ok(0);
        }
        if let Ok(projects) = std::fs::read_dir(sessions_root) {
            for project in projects.flatten() {
                if project.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && let Ok(files) = std::fs::read_dir(project.path())
                {
                    for file in files.flatten() {
                        if let Some(name) = file.file_name().to_str()
                            && name.ends_with(".quarantine")
                            && std::fs::remove_file(file.path()).is_ok()
                        {
                            count += 1;
                        }
                    }
                }
            }
        }
        Ok(count)
    }

    /// Remove a session's brain file while holding its write lock.
    ///
    /// Taking the lock is the point: a prune must not race an in-flight write
    /// to the same session file. Returns whether the brain file itself was
    /// actually removed, so a caller can tell a clean removal from a missing
    /// file.
    pub async fn remove(&self, project_id: &str, session_id: &str) -> Result<bool> {
        let path = self.brain_path(project_id, session_id)?;
        let lock = lock_for(&path);
        let _guard = lock.lock().await;
        let removed = remove_if_present(&path)?;
        // The engine's write-ahead log sits beside the brain file and outlives
        // the handle that wrote it. Prune is how the human reclaims the disk,
        // so the sidecar goes with the file it belongs to.
        for suffix in SIDECAR_SUFFIXES {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            // The brain file is already gone, so a sidecar that will not go
            // must not fail the removal: the prune would keep its tombstone
            // and be retried on every sweep for a file that no longer matters.
            if let Err(err) = remove_if_present(Path::new(&sidecar)) {
                tracing::warn!(
                    sidecar = %Path::new(&sidecar).display(),
                    error = %err,
                    "could not remove a brain sidecar"
                );
            }
        }
        Ok(removed)
    }
}

fn quarantine_path(path: &Path) -> PathBuf {
    let mut q = path.as_os_str().to_os_string();
    q.push(".quarantine");
    PathBuf::from(q)
}

/// What the engine can write beside a brain file, as a suffix on its path.
const SIDECAR_SUFFIXES: [&str; 1] = ["-wal"];

/// Remove a file, treating an already absent one as nothing to do.
fn remove_if_present(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// The write lock for a session file, creating it on first use.
///
/// Dead locks are dropped on lookup so the table does not accumulate one entry
/// per session ever seen.
fn lock_for(path: &Path) -> Arc<AsyncMutex<()>> {
    // Recover a poisoned table rather than propagating a panic: the critical
    // section is a map probe and an insert, so the table is consistent however
    // a panicking holder left it, and one poisoned mutex must not turn every
    // later brain open down until restart.
    let mut locks = locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, weak| weak.strong_count() > 0);
    if let Some(existing) = locks.get(path).and_then(|weak| weak.upgrade()) {
        return existing;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

/// A handle to one session's AgentFS brain.
///
/// Write operations take the session lock, so concurrent writers to one
/// session serialise while writers to distinct sessions proceed in parallel.
pub struct Brain {
    agent: AgentFS,
    lock: Arc<AsyncMutex<()>>,
    path: PathBuf,
    session_id: String,
}

impl Brain {
    /// Refuse a write once a prune has removed the file under this handle.
    ///
    /// The handle is opened under the session lock but outlives it, so a prune
    /// can remove the file before the write retakes the lock. The engine would
    /// keep writing into the unlinked file and the caller would index a row
    /// for a session that no longer exists. Called with the lock held, which
    /// the prune also holds to remove the file, so the answer cannot go stale.
    fn ensure_present(&self) -> Result<()> {
        if self.path.exists() {
            return Ok(());
        }
        // The wrapper serves session brains and project knowledge bases alike,
        // so it says what happened and leaves what to do next to the caller.
        Err(Error::Conflict(
            "the store was removed while this write was waiting; nothing was written".to_string(),
        ))
    }

    /// The session this brain belongs to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The path to the underlying brain file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The total bytes of this brain file and its sidecars on disk.
    pub fn file_bytes(&self) -> i64 {
        file_bytes(&self.path)
    }

    /// Record an audit row for a write operation into the tool_calls table.
    ///
    /// Takes the session write lock so concurrent writes to one file serialise
    /// their audit logging. A write that must be logged in the order it
    /// landed uses [`Brain::put_if_recorded`] instead, which holds one guard
    /// across both.
    pub async fn record_write(
        &self,
        name: &str,
        path: &str,
        actor: &str,
        bytes: Option<usize>,
        version: Option<&str>,
    ) -> Result<i64> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.record_locked(name, path, actor, bytes, version, false)
            .await
    }

    /// Append one row to the write log. The caller holds the write lock.
    async fn record_locked(
        &self,
        name: &str,
        path: &str,
        actor: &str,
        bytes: Option<usize>,
        version: Option<&str>,
        verifies: bool,
    ) -> Result<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Engine(e.to_string()))?
            .as_secs() as i64;
        let store = if self.session_id == KNOWLEDGE_FILE {
            "project"
        } else {
            "session"
        };
        let mut params = serde_json::json!({
            "path": path,
            "actor": actor,
            "store": store,
        });
        if let Some(b) = bytes {
            params["bytes"] = serde_json::json!(b);
        }
        if let Some(v) = version {
            params["version"] = serde_json::json!(v);
        }
        if verifies {
            params["verifies"] = serde_json::json!(true);
        }
        let result = version.map(|v| serde_json::json!({ "version": v }));

        self.agent
            .tools
            .record(name, now, now, Some(params), result, None)
            .await
            .map_err(engine_error)
    }

    /// Whether this file keeps every version of what it stores. A project
    /// knowledge base does; a session brain is working state and does not.
    fn keeps_versions(&self) -> bool {
        self.session_id == KNOWLEDGE_FILE
    }

    /// Keep one version's bytes, once however many writes store them. The
    /// caller holds the write lock.
    async fn keep_version(&self, bytes: &[u8]) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Engine(e.to_string()))?
            .as_secs() as i64;
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        conn.execute(
            "INSERT OR IGNORE INTO hub_page_versions (version, content, kept_at) VALUES (?1, ?2, ?3)",
            vec![
                turso::Value::Text(version(bytes)),
                turso::Value::Blob(bytes.to_vec()),
                turso::Value::Integer(now),
            ],
        )
        .await
        .map_err(|err| Error::Engine(err.to_string()))?;
        Ok(())
    }

    /// What one page holds and its history knows about one version, read
    /// under one hold of the lock: the page's bytes now, its log rows oldest
    /// first, where its history was forgotten, and the kept bytes of the
    /// version unless the page holds them now.
    pub async fn page_version(&self, path: &str, version: &str) -> Result<PageVersionState> {
        let namespace = parse_path(path)?;
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        let now = self.read(&namespace).await?;
        let mut writes = Vec::new();
        self.scan_log_locked(|record| {
            if record.path == path {
                writes.push(record);
            }
            std::ops::ControlFlow::Continue(())
        })
        .await?;
        writes.reverse();
        let current = now
            .as_deref()
            .is_some_and(|bytes| super::version(bytes) == version);
        let (mark, kept) = if self.keeps_versions() {
            let mark = self.forgotten_locked().await?.remove(path);
            let kept = if current {
                None
            } else {
                self.kept_version_locked(version).await?
            };
            (mark, kept)
        } else {
            (None, None)
        };
        Ok(PageVersionState {
            now,
            writes,
            mark,
            kept,
        })
    }

    /// The bytes of one kept version, or `None` when the file does not hold
    /// them. The caller holds the write lock.
    async fn kept_version_locked(&self, version: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        let mut rows = conn
            .query(
                "SELECT content FROM hub_page_versions WHERE version = ?1",
                vec![turso::Value::Text(version.to_string())],
            )
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(|err| Error::Engine(err.to_string()))?
        else {
            return Ok(None);
        };
        match row.get_value(0) {
            Ok(turso::Value::Blob(bytes)) => Ok(Some(bytes)),
            Ok(turso::Value::Text(text)) => Ok(Some(text.into_bytes())),
            _ => Ok(None),
        }
    }

    /// Which of `versions` this file holds the bytes of, in one query.
    pub async fn kept_versions(
        &self,
        versions: &[&str],
    ) -> Result<std::collections::HashSet<String>> {
        let mut wanted: Vec<&str> = versions.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        if !self.keeps_versions() || wanted.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.kept_among_locked(&wanted).await
    }

    /// Which of `versions` the file holds the bytes of. The caller holds the
    /// write lock and passes each version once.
    async fn kept_among_locked(
        &self,
        versions: &[&str],
    ) -> Result<std::collections::HashSet<String>> {
        let mut kept = std::collections::HashSet::new();
        if versions.is_empty() {
            return Ok(kept);
        }
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        let marks = (1..=versions.len())
            .map(|at| format!("?{at}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut rows = conn
            .query(
                &format!("SELECT version FROM hub_page_versions WHERE version IN ({marks})"),
                versions
                    .iter()
                    .map(|version| turso::Value::Text(version.to_string()))
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            if let Ok(turso::Value::Text(version)) = row.get_value(0) {
                kept.insert(version);
            }
        }
        Ok(kept)
    }

    /// Whether the file already holds one version's bytes. The caller holds
    /// the write lock.
    async fn is_kept_locked(&self, bytes: &[u8]) -> Result<bool> {
        let version = version(bytes);
        Ok(self
            .kept_among_locked(&[version.as_str()])
            .await?
            .contains(&version))
    }

    /// Where a page's history was forgotten, if it was.
    pub async fn forgotten_through(&self, path: &str) -> Result<Option<ForgetMark>> {
        if !self.keeps_versions() {
            return Ok(None);
        }
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        Ok(self.forgotten_locked().await?.remove(path))
    }

    /// Every forgotten page and where it was forgotten. The caller holds the
    /// write lock.
    async fn forgotten_locked(&self) -> Result<std::collections::HashMap<String, ForgetMark>> {
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        let mut rows = conn
            .query(
                "SELECT path, through_id, kept_version FROM hub_page_forgotten",
                (),
            )
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        let mut forgotten = std::collections::HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            if let (Ok(turso::Value::Text(path)), Ok(turso::Value::Integer(through_id))) =
                (row.get_value(0), row.get_value(1))
            {
                let kept_version = match row.get_value(2) {
                    Ok(turso::Value::Text(version)) => Some(version),
                    _ => None,
                };
                forgotten.insert(
                    path,
                    ForgetMark {
                        through_id,
                        kept_version,
                    },
                );
            }
        }
        Ok(forgotten)
    }

    /// The bytes the file's pages hold, its free pages left out.
    ///
    /// A delete gives its pages back to the file rather than to the disk, and
    /// the next write reuses them, so the size limit is held against what is
    /// used rather than how large the file has grown. The write-ahead log
    /// counts too, through the page count the connection sees.
    pub async fn used_bytes(&self) -> Result<i64> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.used_bytes_locked().await
    }

    /// [`Brain::used_bytes`] for a caller that holds the write lock.
    async fn used_bytes_locked(&self) -> Result<i64> {
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        let pragma = async |name: &str| -> Result<i64> {
            let mut rows = conn
                .query(&format!("PRAGMA {name}"), ())
                .await
                .map_err(|err| Error::Engine(err.to_string()))?;
            match rows
                .next()
                .await
                .map_err(|err| Error::Engine(err.to_string()))?
                .map(|row| row.get_value(0))
            {
                Some(Ok(turso::Value::Integer(value))) => Ok(value),
                _ => Err(Error::Engine(format!("PRAGMA {name} returned no number"))),
            }
        };
        let pages = pragma("page_count").await?;
        let free = pragma("freelist_count").await?;
        let size = pragma("page_size").await?;
        Ok(pages.saturating_sub(free).saturating_mul(size))
    }

    /// What the size limit is held against: the used pages of a knowledge
    /// base, whose history a purge hands back, and the file on disk for a
    /// session brain. The caller holds the write lock.
    async fn occupied_locked(&self) -> Result<i64> {
        if self.keeps_versions() {
            self.used_bytes_locked().await
        } else {
            Ok(self.file_bytes())
        }
    }

    /// What the size limit is held against, as [`Brain::occupied_locked`].
    pub async fn occupied_bytes(&self) -> Result<i64> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.occupied_locked().await
    }

    /// Forget the kept bytes of one page's history: every version its log
    /// rows name except the one the page holds now, and for a deleted page
    /// every one. The rows stay, so the history still says who wrote what and
    /// when; they no longer lead to bytes.
    ///
    /// Versions are kept once per content, so bytes another page's history
    /// still names stay for that page. The page is marked forgotten through
    /// its newest row, so nothing up to there reads back through this path
    /// even when the bytes stay for another.
    ///
    /// `audit` is given what the purge will remove before anything is, still
    /// under the lock, so a purge whose audit fails removes nothing and the
    /// record never misses one. The removals and the mark are one transaction.
    pub async fn forget_history(
        &self,
        path: &str,
        audit: impl AsyncFnOnce(&Forgotten) -> Result<()>,
    ) -> Result<Forgotten> {
        let namespace = parse_path(path)?;
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        let mut forgotten = Forgotten::default();
        if !self.keeps_versions() {
            return Ok(forgotten);
        }
        let current = self.read(&namespace).await?.map(|bytes| version(&bytes));
        let marks = self.forgotten_locked().await?;
        let mut through = None;
        let mut named = std::collections::BTreeSet::new();
        let mut elsewhere = std::collections::HashSet::new();
        let mut anywhere = std::collections::HashSet::new();
        self.scan_log_locked(|record| {
            if let Some(version) = record.version.clone() {
                anywhere.insert(version);
            }
            if let Some(version) = record.version {
                if record.path == path {
                    through = through.max(Some(record.id));
                    named.insert(version);
                } else if marks
                    .get(&record.path)
                    .is_none_or(|mark| mark.readable(record.id, Some(&version)))
                {
                    elsewhere.insert(version);
                }
            } else if record.path == path {
                through = through.max(Some(record.id));
            }
            std::ops::ControlFlow::Continue(())
        })
        .await?;
        let Some(through) = through else {
            return Ok(forgotten);
        };
        let going: Vec<&str> = named
            .iter()
            .map(String::as_str)
            .filter(|version| Some(*version) != current.as_deref())
            .filter(|version| !elsewhere.contains(*version))
            .collect();
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        // What goes is counted first, so the audit names it before it goes.
        let mut held = Vec::new();
        for version in &going {
            let mut rows = conn
                .query(
                    "SELECT length(content) FROM hub_page_versions WHERE version = ?1",
                    vec![turso::Value::Text(version.to_string())],
                )
                .await
                .map_err(|err| Error::Engine(err.to_string()))?;
            if let Some(row) = rows
                .next()
                .await
                .map_err(|err| Error::Engine(err.to_string()))?
            {
                if let Ok(turso::Value::Integer(length)) = row.get_value(0) {
                    forgotten.bytes += length;
                }
                forgotten.versions += 1;
                held.push(*version);
            }
        }
        // Bytes no row names are read by nothing and only hold space.
        let mut kept = conn
            .query("SELECT version, length(content) FROM hub_page_versions", ())
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        let mut unreferenced = Vec::new();
        while let Some(row) = kept
            .next()
            .await
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            if let Ok(turso::Value::Text(version)) = row.get_value(0)
                && !anywhere.contains(&version)
            {
                if let Ok(turso::Value::Integer(length)) = row.get_value(1) {
                    forgotten.unreferenced_bytes += length;
                }
                forgotten.unreferenced += 1;
                unreferenced.push(version);
            }
        }
        drop(kept);
        forgotten.through_id = Some(through);
        audit(&forgotten).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Engine(e.to_string()))?
            .as_secs() as i64;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        let removed = async {
            for version in held
                .iter()
                .copied()
                .chain(unreferenced.iter().map(String::as_str))
            {
                conn.execute(
                    "DELETE FROM hub_page_versions WHERE version = ?1",
                    vec![turso::Value::Text(version.to_string())],
                )
                .await
                .map_err(|err| Error::Engine(err.to_string()))?;
            }
            conn.execute(
                "INSERT INTO hub_page_forgotten (path, through_id, kept_version, forgotten_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(path) DO UPDATE SET through_id = excluded.through_id, \
             kept_version = excluded.kept_version, forgotten_at = excluded.forgotten_at",
                vec![
                    turso::Value::Text(path.to_string()),
                    turso::Value::Integer(through),
                    current.map_or(turso::Value::Null, turso::Value::Text),
                    turso::Value::Integer(now),
                ],
            )
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
            Ok::<(), Error>(())
        }
        .await;
        let committed = match removed {
            Ok(()) => conn
                .execute("COMMIT", ())
                .await
                .map(|_| ())
                .map_err(|err| Error::Engine(err.to_string())),
            Err(err) => Err(err),
        };
        match committed {
            Ok(()) => Ok(forgotten),
            Err(err) => {
                // A commit that fails leaves the transaction open; either way
                // nothing it did stays.
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(err)
            }
        }
    }

    /// Run `work` on a connection to this file while holding its write lock.
    ///
    /// An online backup copies the file through this, so no write to it
    /// interleaves with the copy. A file a prune removed after the open is
    /// refused, as a write would be.
    pub async fn with_locked_connection<T>(
        &self,
        work: impl AsyncFnOnce(&turso::Connection) -> Result<T>,
    ) -> Result<T> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        work(&conn).await
    }

    /// Fold the write-ahead log into the database file and truncate it, so the
    /// file's size reflects the data it holds rather than pages still in the
    /// log. Run when a session ends, so a finished brain is one consolidated
    /// file and its size means what a reader expects.
    pub async fn checkpoint(&self) -> Result<()> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        // The pragma returns a row, so it is run as a query and drained.
        let mut rows = conn
            .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        while rows
            .next()
            .await
            .map_err(|err| Error::Engine(err.to_string()))?
            .is_some()
        {}
        Ok(())
    }

    /// Read recent tool_calls audit records.
    pub async fn audit_recent(&self, limit: Option<i64>) -> Result<Vec<agentfs_sdk::ToolCall>> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.agent.tools.recent(limit).await.map_err(engine_error)
    }

    /// Visit every row of the write log, newest first, until `visit` breaks.
    ///
    /// The log has no index on the path it records, so every reader of it is
    /// a scan. The rows stream, so a scan holds one row at a time however long
    /// the log has grown.
    async fn scan_log(
        &self,
        visit: impl FnMut(WriteRecord) -> std::ops::ControlFlow<()>,
    ) -> Result<()> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.scan_log_locked(visit).await
    }

    /// [`Brain::scan_log`] for a caller that already holds the write lock.
    async fn scan_log_locked(
        &self,
        mut visit: impl FnMut(WriteRecord) -> std::ops::ControlFlow<()>,
    ) -> Result<()> {
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        let mut rows = conn
            .query(
                "SELECT id, name, parameters, result, started_at, completed_at \
                 FROM tool_calls ORDER BY id DESC",
                (),
            )
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            let integer = |index: usize| match row.get_value(index) {
                Ok(turso::Value::Integer(value)) => Some(value),
                _ => None,
            };
            let json = |index: usize| match row.get_value(index) {
                Ok(turso::Value::Text(text)) => {
                    serde_json::from_str::<serde_json::Value>(&text).ok()
                }
                _ => None,
            };
            let field = |value: &Option<serde_json::Value>, key: &str| {
                value
                    .as_ref()
                    .and_then(|value| value.get(key))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            let Some(id) = integer(0) else { continue };
            let op = match row.get_value(1) {
                Ok(turso::Value::Text(name)) => name,
                _ => continue,
            };
            let parameters = json(2);
            // A row that names no path is not a write to an entry, so it is
            // no part of any history.
            let Some(path) = field(&parameters, "path") else {
                continue;
            };
            let record = WriteRecord {
                id,
                op,
                path,
                actor: field(&parameters, "actor").unwrap_or_default(),
                at: integer(5).or_else(|| integer(4)).unwrap_or_default(),
                version: field(&json(3), "version"),
                bytes: parameters
                    .as_ref()
                    .and_then(|parameters| parameters.get("bytes"))
                    .and_then(serde_json::Value::as_i64),
                verifies: parameters
                    .as_ref()
                    .and_then(|parameters| parameters.get("verifies"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            };
            if visit(record).is_break() {
                break;
            }
        }
        Ok(())
    }

    /// One page of the write log, newest first, with the count the filter
    /// matches across the whole log.
    ///
    /// `before` is a row id from an earlier page's `next_before`. The count
    /// ignores it, so every page of one walk reports the same total. The scan
    /// is the whole log and the only bound on the log is the file's own size
    /// cap, so nothing falls out of a history by being old.
    pub async fn write_log(
        &self,
        filter: &WriteFilter<'_>,
        before: Option<i64>,
        limit: usize,
    ) -> Result<WriteLogPage> {
        let mut page = WriteLogPage::default();
        let mut more = false;
        self.scan_log(|record| {
            if filter.matches(&record) {
                page.total += 1;
                if before.is_none_or(|before| record.id < before) {
                    if page.rows.len() < limit {
                        page.rows.push(record);
                    } else {
                        more = true;
                    }
                }
            }
            std::ops::ControlFlow::Continue(())
        })
        .await?;
        if more {
            // With no rows asked for there is no row to continue from, and
            // the caller wanted the count.
            page.next_before = page.rows.last().map(|record| record.id);
            page.truncated = true;
        }
        Ok(page)
    }

    /// The newest write to one path, if the log holds one.
    pub async fn last_write(&self, path: &str) -> Result<Option<WriteRecord>> {
        let mut found = None;
        self.scan_log(|record| {
            if record.path == path {
                found = Some(record);
                return std::ops::ControlFlow::Break(());
            }
            std::ops::ControlFlow::Continue(())
        })
        .await?;
        Ok(found)
    }

    /// The newest write to every path the log names, the newest of all, and
    /// the newest write to each path that `verifying` accepts.
    pub async fn last_writes(
        &self,
        verifying: impl Fn(&WriteRecord) -> bool,
    ) -> Result<LastWrites> {
        let mut last = LastWrites::default();
        self.scan_log(|record| {
            if last.newest.is_none() {
                last.newest = Some(record.clone());
            }
            if verifying(&record) {
                last.verifying
                    .entry(record.path.clone())
                    .or_insert_with(|| record.clone());
            }
            last.by_path.entry(record.path.clone()).or_insert(record);
            std::ops::ControlFlow::Continue(())
        })
        .await?;
        Ok(last)
    }

    /// Copy this brain into a new file through the engine.
    ///
    /// The engine's own copy, never a filesystem one: it snapshots every table,
    /// the append-only tool call log included, and writes one self-contained
    /// file with no sidecar of its own. `alive` runs under the write lock, like
    /// [`BrainStore::open_live`], so a caller that checked the session is still
    /// there before the lock re-checks it here, where a prune cannot land in
    /// between and leave a copy of a file it is removing.
    pub async fn vacuum_into(
        &self,
        destination: &Path,
        alive: impl AsyncFnOnce() -> Result<()>,
    ) -> Result<()> {
        self.vacuum_and_then(destination, alive, async || Ok(()))
            .await
    }

    /// Copy this brain file into a new file, executing a hook under the source's
    /// lock after the copy completes.
    pub async fn vacuum_and_then<R>(
        &self,
        destination: &Path,
        alive: impl AsyncFnOnce() -> Result<()>,
        on_copied: impl AsyncFnOnce() -> Result<R>,
    ) -> Result<R> {
        let destination = destination.to_str().ok_or_else(|| {
            Error::Config("brain copy destination is not valid UTF-8".to_string())
        })?;
        // The engine reads the destination as a string literal and takes no
        // bound parameter there, so a quote in the path would end the
        // statement. Every path the store builds comes from a validated
        // project and session id and can hold none; anything else is refused
        // here rather than trusted.
        if destination.contains('\'') {
            return Err(Error::InvalidArgument(
                "a brain copy destination may not contain a quote".to_string(),
            ));
        }
        let _guard = self.lock.lock().await;
        alive().await?;
        self.ensure_present()?;
        let conn = self.agent.get_connection().await.map_err(engine_error)?;
        conn.execute(&format!("VACUUM INTO '{destination}'"), ())
            .await
            .map_err(|err| Error::Engine(err.to_string()))?;
        let res = on_copied().await?;
        Ok(res)
    }

    /// Read a value. Returns `None` when nothing is stored at the path.
    ///
    /// A directory under `/fs/` reads as `None`; only regular files have
    /// content.
    pub async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.read(&parse_path(path)?).await
    }

    /// Read one entry. Returns its kind and raw bytes.
    ///
    /// Refuses directories with [`Error::Conflict`]. Missing entries return
    /// [`Error::NotFound`].
    pub async fn entry(&self, path: &str) -> Result<(EntryKind, Vec<u8>)> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        match parse_path(path)? {
            Namespace::Kv(key) => {
                if key.is_empty() {
                    return Err(Error::Conflict(format!(
                        "brain path '{path}' is a directory"
                    )));
                }
                let value = self
                    .agent
                    .kv
                    .get::<StoredValue>(key)
                    .await
                    .map_err(engine_error)?
                    .ok_or_else(|| Error::NotFound(format!("no brain entry at '{path}'")))?;
                Ok((EntryKind::Key, value.into_bytes()))
            }
            Namespace::Fs(rest) => {
                let fs_path = fs_path(rest);
                if fs_path == "/" {
                    return Err(Error::Conflict(format!(
                        "brain path '{path}' is a directory"
                    )));
                }
                let stats = self
                    .agent
                    .fs
                    .stat(&fs_path)
                    .await
                    .map_err(engine_error)?
                    .ok_or_else(|| Error::NotFound(format!("no brain entry at '{path}'")))?;
                if stats.is_directory() {
                    return Err(Error::Conflict(format!(
                        "brain path '{path}' is a directory"
                    )));
                }
                if stats.is_file() {
                    let bytes = self
                        .agent
                        .fs
                        .read_file(&fs_path)
                        .await
                        .map_err(engine_error)?
                        .unwrap_or_default();
                    Ok((EntryKind::File, bytes))
                } else {
                    Err(Error::NotFound(format!("no brain entry at '{path}'")))
                }
            }
        }
    }

    /// Store bytes, creating the entry and any missing parent directories.
    ///
    /// Takes the session write lock. A value over the cap is refused before
    /// anything is written.
    pub async fn put(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.put_if(path, bytes, None).await.map(|_| ())
    }

    /// Store bytes only when what is there matches `expected`, and report the
    /// version of the bytes written.
    ///
    /// `expected` is a token from `version`, or `VERSION_ABSENT` to create an
    /// entry only while nothing is stored at the path; `None` is an
    /// unconditional write. The read, the comparison and the write all happen
    /// under the write lock, so two callers holding the same token cannot both
    /// win: doing this as a get followed by a put would let their writes
    /// interleave between the two calls.
    pub async fn put_if(&self, path: &str, bytes: &[u8], expected: Option<&str>) -> Result<String> {
        crate::limits::check_brain_value(bytes.len())?;
        let namespace = parse_path(path)?;
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        crate::limits::check_brain_file_projected(self.file_bytes(), bytes.len())?;
        self.check_expected(&namespace, path, expected).await?;
        self.write(&namespace, bytes).await?;
        Ok(version(bytes))
    }

    /// Store bytes like [`Brain::put_if`], and log the write under the same
    /// guard.
    ///
    /// The row and the write share one hold of the lock, so the log is in the
    /// order the writes landed and a write is never stored without its row.
    /// `indexed` runs under the guard too, after both: whatever mirrors the
    /// entry elsewhere sees the writes in that same order.
    pub async fn put_if_recorded(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<&str>,
        stamp: Stamp<'_>,
        alive: impl AsyncFnOnce() -> Result<()>,
        indexed: impl AsyncFnOnce(),
    ) -> Result<String> {
        // Asked to write whatever is there, it always stores, so it always
        // names a version.
        let stored = self
            .put_recorded(path, bytes, expected, stamp, false, alive, indexed)
            .await?;
        Ok(stored.unwrap_or_else(|| self::version(bytes)))
    }

    /// [`Brain::put_if_recorded`] that stores nothing when the entry already
    /// holds exactly these bytes, answering `None` then. The comparison is made
    /// under the same hold of the lock as the guard and the write, so a revert
    /// to what the page holds is decided against what it holds as it lands.
    pub async fn put_if_changed_recorded(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<&str>,
        stamp: Stamp<'_>,
        alive: impl AsyncFnOnce() -> Result<()>,
        indexed: impl AsyncFnOnce(),
    ) -> Result<Option<String>> {
        self.put_recorded(path, bytes, expected, stamp, true, alive, indexed)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn put_recorded(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<&str>,
        stamp: Stamp<'_>,
        unless_same: bool,
        alive: impl AsyncFnOnce() -> Result<()>,
        indexed: impl AsyncFnOnce(),
    ) -> Result<Option<String>> {
        crate::limits::check_brain_value(bytes.len())?;
        let namespace = parse_path(path)?;
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        alive().await?;
        let keeps = self.keeps_versions();
        // One read serves every question about what is being replaced.
        let before = match (expected, stamp.verifies, keeps || unless_same) {
            (None, None, false) => None,
            _ => self.read(&namespace).await?,
        };
        if unless_same && before.as_deref() == Some(bytes) {
            if let Some(expected) = expected {
                ensure_expected(path, expected, before.as_deref())?;
            }
            return Ok(None);
        }
        // A knowledge base keeps a copy of the bytes it stores and of the
        // bytes they replace, each unless it already holds them.
        let mut incoming = bytes.len();
        if keeps {
            if !self.is_kept_locked(bytes).await? {
                incoming += bytes.len();
            }
            if let Some(before) = before.as_deref()
                && !self.is_kept_locked(before).await?
            {
                incoming += before.len();
            }
        }
        crate::limits::check_brain_file_projected(self.occupied_locked().await?, incoming)?;
        if let Some(expected) = expected {
            ensure_expected(path, expected, before.as_deref())?;
        }
        let verifies = stamp
            .verifies
            .is_some_and(|judge| judge(before.as_deref(), bytes));
        self.write(&namespace, bytes).await?;
        if keeps {
            // Both sides are kept once the write has landed and before its log
            // row, so a write the store refuses (a directory at the path, a
            // file where a parent should be) keeps nothing, and no log row
            // ever names a version whose bytes the file lacks. The bytes
            // replaced are kept too: a page written before versions were kept
            // has its last content saved the first time it changes.
            if let Some(before) = before.as_deref() {
                self.keep_version(before).await?;
            }
            self.keep_version(bytes).await?;
        }
        let version = version(bytes);
        self.record_locked(
            stamp.op,
            path,
            stamp.actor,
            Some(bytes.len()),
            Some(&version),
            verifies,
        )
        .await?;
        indexed().await;
        Ok(Some(version))
    }

    /// Refuse when what is stored is not what the caller expects to replace.
    /// The caller holds the write lock.
    async fn check_expected(
        &self,
        namespace: &Namespace<'_>,
        path: &str,
        expected: Option<&str>,
    ) -> Result<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        ensure_expected(path, expected, self.read(namespace).await?.as_deref())
    }

    /// Read whatever is stored in one namespace.
    async fn read(&self, namespace: &Namespace<'_>) -> Result<Option<Vec<u8>>> {
        match namespace {
            Namespace::Kv(key) => {
                let key = require_key(key)?;
                let value = self
                    .agent
                    .kv
                    .get::<StoredValue>(key)
                    .await
                    .map_err(engine_error)?;
                Ok(value.map(StoredValue::into_bytes))
            }
            Namespace::Fs(path) => {
                let path = fs_path(path);
                match self.agent.fs.stat(&path).await.map_err(engine_error)? {
                    Some(stats) if stats.is_file() => {
                        self.agent.fs.read_file(&path).await.map_err(engine_error)
                    }
                    _ => Ok(None),
                }
            }
        }
    }

    /// Write bytes into one namespace. The caller holds the write lock.
    async fn write(&self, namespace: &Namespace<'_>, bytes: &[u8]) -> Result<()> {
        match namespace {
            Namespace::Kv(key) => {
                let key = require_key(key)?;
                let value = StoredValue::from_bytes(bytes);
                self.agent.kv.set(key, &value).await.map_err(engine_error)
            }
            Namespace::Fs(path) => self.put_file(&fs_path(path), bytes).await,
        }
    }

    /// List entries under a path prefix.
    ///
    /// For `/kv/<prefix>` this returns every key that starts with the prefix,
    /// as `/kv/<key>`. For `/fs/<dir>` it returns the immediate children of
    /// the directory, as `/fs/<dir>/<name>`. A prefix with nothing under it
    /// yields an empty list.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Entry>> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        match parse_path(prefix)? {
            Namespace::Kv(key_prefix) => {
                let mut keys = self.agent.kv.keys().await.map_err(engine_error)?;
                keys.retain(|key| key.starts_with(key_prefix));
                keys.sort();
                let mut entries = Vec::with_capacity(keys.len());
                for key in keys {
                    // The size is of the value a read hands back, not of the
                    // JSON the store holds it in, so it matches the bytes the
                    // caller would get.
                    let size_bytes = self
                        .agent
                        .kv
                        .get::<StoredValue>(&key)
                        .await
                        .map_err(engine_error)?
                        .map_or(0, |value| value.into_bytes().len() as i64);
                    entries.push(Entry {
                        path: format!("/kv/{key}"),
                        kind: EntryKind::Key,
                        size_bytes,
                    });
                }
                Ok(entries)
            }
            Namespace::Fs(path) => self.fs_children(path, true).await,
        }
    }

    /// The immediate children of an `/fs/<dir>`, as `list` names them, but
    /// with no byte total for a child directory: its `size_bytes` is 0.
    ///
    /// For a walk that needs only names, kinds and page sizes, which `list`
    /// would make pay a second directory read per child directory. A path
    /// outside `/fs` yields an empty list.
    pub async fn list_names(&self, prefix: &str) -> Result<Vec<Entry>> {
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        match parse_path(prefix)? {
            Namespace::Kv(_) => Ok(Vec::new()),
            Namespace::Fs(path) => self.fs_children(path, false).await,
        }
    }

    /// The children of one `/fs` directory, with directory byte totals when
    /// `dir_bytes` is set. Called with the lock held.
    async fn fs_children(&self, path: &str, dir_bytes: bool) -> Result<Vec<Entry>> {
        let dir = fs_path(path);
        let Some(stats) = self.agent.fs.stat(&dir).await.map_err(engine_error)? else {
            return Ok(Vec::new());
        };
        if !stats.is_directory() {
            return Ok(Vec::new());
        }
        let base = if dir == "/" {
            "/fs".to_string()
        } else {
            format!("/fs{dir}")
        };
        let mut entries = Vec::new();
        for child in self.children(stats.ino).await? {
            let (kind, size_bytes) = if !child.stats.is_directory() {
                (EntryKind::File, child.stats.size)
            } else if dir_bytes {
                (EntryKind::Dir, self.directory_bytes(child.stats.ino).await?)
            } else {
                (EntryKind::Dir, 0)
            };
            entries.push(Entry {
                path: format!("{base}/{}", child.name),
                kind,
                size_bytes,
            });
        }
        Ok(entries)
    }

    /// The bytes held directly under a directory.
    ///
    /// The sum over the immediate children, so a listing shows a directory
    /// total without a second walk. A child directory holds no bytes of its
    /// own, so a deeper tree is not counted here.
    async fn directory_bytes(&self, ino: i64) -> Result<i64> {
        Ok(self
            .children(ino)
            .await?
            .iter()
            .map(|child| child.stats.size)
            .sum())
    }

    /// The immediate children of a directory, by name, with their stats.
    async fn children(&self, ino: i64) -> Result<Vec<agentfs_sdk::filesystem::DirEntry>> {
        // The stats come back with the names in one query, so a listing does
        // not stat every entry it names.
        self.agent
            .fs
            .readdir_plus(ino)
            .await
            .map(Option::unwrap_or_default)
            .map_err(engine_error)
    }

    /// Delete an entry. Deleting something that is already absent is a no-op.
    ///
    /// Takes the session write lock. A non-empty directory is not removed.
    pub async fn delete(&self, path: &str) -> Result<()> {
        self.delete_if(path, None).await
    }

    /// Delete an entry only when what is there matches `expected`.
    ///
    /// `expected` is a token from `version`, or `VERSION_ABSENT`; `None` is an
    /// unconditional delete.
    pub async fn delete_if(&self, path: &str, expected: Option<&str>) -> Result<()> {
        let namespace = parse_path(path)?;
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        self.check_expected(&namespace, path, expected).await?;
        self.remove(&namespace).await.map(|_| ())
    }

    /// Delete an entry and log the delete under the same guard, reporting
    /// whether anything was there to delete.
    ///
    /// Nothing stored means nothing happened: no row is logged, `removed` does
    /// not run and the answer is `false`, whatever `expected` says, so a
    /// caller can report an absent entry the way a read does.
    pub async fn delete_if_recorded(
        &self,
        path: &str,
        expected: Option<&str>,
        stamp: Stamp<'_>,
        alive: impl AsyncFnOnce() -> Result<()>,
        removed: impl AsyncFnOnce(),
    ) -> Result<bool> {
        let namespace = parse_path(path)?;
        let _guard = self.lock.lock().await;
        self.ensure_present()?;
        alive().await?;
        if !self.exists(&namespace).await? {
            return Ok(false);
        }
        self.check_expected(&namespace, path, expected).await?;
        if self.keeps_versions()
            && let Some(going) = self.read(&namespace).await?
            && !self.is_kept_locked(&going).await?
        {
            // A delete keeps what it removes, at most one page, and is never
            // refused for it: a full knowledge base must still be able to
            // shed a page.
            self.keep_version(&going).await?;
        }
        if !self.remove(&namespace).await? {
            return Ok(false);
        }
        self.record_locked(stamp.op, path, stamp.actor, None, None, false)
            .await?;
        removed().await;
        Ok(true)
    }

    /// Whether a regular file is stored at a path.
    pub async fn is_file(&self, path: &str) -> Result<bool> {
        match parse_path(path)? {
            Namespace::Kv(_) => Ok(false),
            Namespace::Fs(rest) => Ok(self
                .agent
                .fs
                .stat(&fs_path(rest))
                .await
                .map_err(engine_error)?
                .is_some_and(|stats| stats.is_file())),
        }
    }

    /// Whether anything, a directory included, is stored in a namespace.
    async fn exists(&self, namespace: &Namespace<'_>) -> Result<bool> {
        match namespace {
            Namespace::Kv(_) => Ok(self.read(namespace).await?.is_some()),
            Namespace::Fs(path) => Ok(self
                .agent
                .fs
                .stat(&fs_path(path))
                .await
                .map_err(engine_error)?
                .is_some()),
        }
    }

    /// Remove what a namespace holds, reporting whether anything was there.
    /// The caller holds the write lock.
    async fn remove(&self, namespace: &Namespace<'_>) -> Result<bool> {
        match namespace {
            Namespace::Kv(key) => {
                let key = require_key(key)?;
                let held = self.exists(namespace).await?;
                self.agent.kv.delete(key).await.map_err(engine_error)?;
                Ok(held)
            }
            Namespace::Fs(path) => {
                let path = fs_path(path);
                let Some(stats) = self.agent.fs.stat(&path).await.map_err(engine_error)? else {
                    return Ok(false);
                };
                // The engine refuses this too, as a failure of its own. It is
                // the caller's mistake, so it is reported as one.
                if stats.is_directory() && !self.children(stats.ino).await?.is_empty() {
                    return Err(Error::InvalidArgument(format!(
                        "'/fs{path}' is a directory that still holds entries; delete those first"
                    )));
                }
                self.agent.fs.remove(&path).await.map_err(engine_error)?;
                Ok(true)
            }
        }
    }

    /// Write a file, creating parent directories as needed.
    async fn put_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.ensure_parent(path).await?;
        if let Some(stats) = self.agent.fs.stat(path).await.map_err(engine_error)? {
            if stats.is_directory() {
                return Err(Error::InvalidArgument(format!(
                    "brain path '{path}' is a directory"
                )));
            }
            self.agent
                .fs
                .truncate(path, 0)
                .await
                .map_err(engine_error)?;
        }
        self.agent
            .fs
            .pwrite(path, 0, bytes)
            .await
            .map_err(engine_error)
    }

    /// Create every missing ancestor directory of a file path.
    async fn ensure_parent(&self, path: &str) -> Result<()> {
        let parent = match path.rsplit_once('/') {
            None | Some(("", _)) => return Ok(()),
            Some((parent, _)) => parent,
        };
        let mut current = String::new();
        for component in parent.split('/').filter(|part| !part.is_empty()) {
            current.push('/');
            current.push_str(component);
            match self.agent.fs.stat(&current).await.map_err(engine_error)? {
                Some(stats) if stats.is_directory() => {}
                Some(_) => {
                    return Err(Error::InvalidArgument(format!(
                        "brain path '{current}' is not a directory"
                    )));
                }
                None => {
                    self.agent
                        .fs
                        .mkdir(&current, 0, 0)
                        .await
                        .map_err(engine_error)?;
                }
            }
        }
        Ok(())
    }
}

/// Validate a project id before it becomes part of a directory name.
fn validate_project_id(project_id: &str) -> Result<()> {
    let valid = !project_id.is_empty()
        && project_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "project id '{project_id}' must be non-empty and contain only ASCII alphanumerics, hyphens, and underscores"
        )))
    }
}

/// Validate a session id before it becomes part of a file name.
fn validate_session_id(session_id: &str) -> Result<()> {
    if AgentFSOptions::validate_agent_id(session_id) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "session id '{session_id}' must be non-empty and contain only alphanumeric characters, hyphens, and underscores"
        )))
    }
}

/// Split a namespaced brain path into its namespace and the path inside it.
fn parse_path(path: &str) -> Result<Namespace<'_>> {
    if let Some(key) = path.strip_prefix("/kv/") {
        Ok(Namespace::Kv(key))
    } else if path == "/kv" {
        Ok(Namespace::Kv(""))
    } else if let Some(rest) = path.strip_prefix("/fs/") {
        Ok(Namespace::Fs(rest))
    } else if path == "/fs" {
        Ok(Namespace::Fs(""))
    } else {
        Err(Error::InvalidArgument(format!(
            "brain path '{path}' must start with /kv/ or /fs/"
        )))
    }
}

/// A key-value operation needs a key, unlike a listing which may take all keys.
fn require_key(key: &str) -> Result<&str> {
    if key.is_empty() {
        Err(Error::InvalidArgument(
            "brain key path must name a key after /kv/".to_string(),
        ))
    } else {
        Ok(key)
    }
}

/// The expected version of a path nothing is stored at, so a caller can create
/// an entry without racing another creator.
pub const VERSION_ABSENT: &str = "absent";

/// The name a project's knowledge base file is opened under, in place of a
/// session id, so one project has exactly one knowledge base.
pub const KNOWLEDGE_FILE: &str = "kb";

/// The bytes one brain file occupies on disk.
///
/// The write-ahead log beside the file counts: it holds frames the engine has
/// not folded back yet, so a stat of the file alone under-reports a brain that
/// was just written to, sometimes by most of its size.
pub fn file_bytes(path: &Path) -> i64 {
    let mut wal = path.to_path_buf().into_os_string();
    wal.push("-wal");
    let mut shm = path.to_path_buf().into_os_string();
    shm.push("-shm");
    [path.to_path_buf(), PathBuf::from(wal), PathBuf::from(shm)]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len() as i64)
        .sum()
}

/// The directory holding one knowledge base file per project.
pub fn knowledge_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("kb")
}

/// Remove session brain files that no session row references, the
/// session-side twin of [`crate::blob::reconcile`].
///
/// A crash between a fork's file copy and its row write, or between a project
/// delete's metadata commit and its file removal, leaves `sessions/<project>/
/// <id>.db` that no row points at. Ids are ULIDs, so such a file is never
/// reused, is counted by no storage figure, and would otherwise grow the disk
/// silently. Called at startup, where the engine lock rules out another hub
/// over the same directory and no write is in flight, so every unreferenced
/// file is dead. `.tmp` (an in-flight fork) and `.quarantine` (a prune) are
/// left to the code that owns them.
pub async fn reconcile_sessions(db: &turso::Database, data_dir: &Path) -> Result<usize> {
    let conn = crate::store::connect(db)?;
    let mut rows = conn
        .query("SELECT project_id, id FROM sessions", ())
        .await
        .map_err(|err| Error::Engine(err.to_string()))?;
    let mut referenced = std::collections::HashSet::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| Error::Engine(err.to_string()))?
    {
        let project = match row
            .get_value(0)
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            turso::Value::Text(project) => project,
            _ => continue,
        };
        let id = match row
            .get_value(1)
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            turso::Value::Text(id) => id,
            _ => continue,
        };
        referenced.insert(format!("{project}/{id}.db"));
    }

    let sessions = data_dir.join("sessions");
    let mut removed = 0;
    for project in sessions_of(&sessions)? {
        let project_name = match project.file_name().and_then(|name| name.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        for entry in std::fs::read_dir(&project)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };
            if !is_session_file(&file_name) {
                continue;
            }
            let rel = format!("{project_name}/{file_name}");
            if referenced.contains(&rel) {
                continue;
            }
            std::fs::remove_file(entry.path())?;
            removed += 1;
            // The engine's write-ahead log sits beside the brain file. It is
            // not a candidate of its own (it is named by no row), so it goes
            // with the file it belongs to, as a prune removes it.
            let mut sidecar = std::ffi::OsString::from(&file_name);
            sidecar.push("-wal");
            let _ = std::fs::remove_file(project.join(sidecar));
        }
        if is_empty_dir(&project)? {
            let _ = std::fs::remove_dir(&project);
        }
    }
    Ok(removed)
}

/// Whether a sessions-directory name is a session brain file.
///
/// `.tmp` and `.quarantine` belong to a fork and a prune and are not swept
/// here.
fn is_session_file(name: &str) -> bool {
    name.ends_with(".db") && !name.ends_with(".tmp") && !name.ends_with(".quarantine")
}

/// The directories directly under the sessions root. A missing directory
/// yields none.
fn sessions_of(path: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

fn is_empty_dir(path: &Path) -> Result<bool> {
    match std::fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(err) => Err(err.into()),
    }
}

/// Refuse when what is stored is not what the caller expects to replace.
fn ensure_expected(path: &str, expected: &str, current: Option<&[u8]>) -> Result<()> {
    let current = current.map_or_else(|| VERSION_ABSENT.to_string(), version);
    if current != expected {
        return Err(Error::Conflict(format!(
            "the entry at '{path}' changed since it was read current_version={current}"
        )));
    }
    Ok(())
}

/// The version token for stored bytes.
///
/// The token is over the bytes a read hands back, never the stored form, so a
/// caller can reproduce it from what it read and it survives any later change
/// to how the store encodes a value.
pub fn version(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    let mut token = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        let _ = write!(token, "{byte:02x}");
    }
    token
}

/// The canonical brain path for an entry, as the store addresses it.
///
/// The filesystem resolves `.`, `..`, empty components and a trailing slash
/// before it reaches an inode, so several client paths name one entry. Anything
/// that keys on a path, the search corpus above all, canonicalises first so one
/// entry never becomes two keys.
pub fn canonical_path(path: &str) -> Result<String> {
    Ok(match parse_path(path)? {
        // A key-value key is opaque: the store uses it verbatim.
        Namespace::Kv(key) => format!("/kv/{}", require_key(key)?),
        Namespace::Fs(rest) => match fs_path(rest).as_str() {
            "/" => "/fs".to_string(),
            resolved => format!("/fs{resolved}"),
        },
    })
}

/// Resolve a filesystem path fragment to the absolute AgentFS path it names.
///
/// This mirrors what the filesystem does internally: `.` and empty components
/// drop out, `..` pops a component but never climbs above the root, and a
/// trailing slash is ignored.
fn fs_path(rest: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            name => components.push(name),
        }
    }
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}

/// Create the table a knowledge base keeps its page versions in.
///
/// It lives in the knowledge base file itself, beside the AgentFS tables and
/// written through the same handle and lock, so the file stays the one place
/// a project's knowledge is held. A version is keyed by its token, the hash of
/// its bytes, so a page that goes back to earlier bytes stores nothing new.
async fn create_versions_table(agent: &AgentFS) -> Result<()> {
    let conn = agent.get_connection().await.map_err(engine_error)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS hub_page_versions (
            version TEXT PRIMARY KEY,
            content BLOB NOT NULL,
            kept_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .map_err(|err| Error::Engine(err.to_string()))?;
    // A page whose history the operator forgot: its log rows up to and
    // including `through_id` no longer name readable bytes, except those of
    // `kept_version`, what the page held when it was forgotten.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS hub_page_forgotten (
            path TEXT PRIMARY KEY,
            through_id INTEGER NOT NULL,
            kept_version TEXT,
            forgotten_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .map_err(|err| Error::Engine(err.to_string()))?;
    Ok(())
}

/// The SDK error carries the detail; the hub reports it as an engine failure.
fn engine_error(err: agentfs_sdk::error::Error) -> Error {
    Error::Engine(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic while the lock table is held must not turn every later brain
    /// open down: the table is recovered, as the id generator already does.
    #[test]
    fn a_poisoned_lock_table_is_recovered() {
        let poisoner = std::thread::spawn(|| {
            let _guard = locks().lock().expect("first lock");
            panic!("poison the lock table on purpose");
        });
        assert!(poisoner.join().is_err(), "the holder panicked");
        assert!(
            locks().lock().is_err(),
            "the table is poisoned, so recovery is what makes the call safe"
        );

        // No panic, and a usable lock comes back.
        let lock = lock_for(Path::new("/nonexistent/brain-poison-test.db"));
        assert_eq!(Arc::strong_count(&lock), 1);

        // The recovery leaves the table usable for subsequent opens.
        let again = lock_for(Path::new("/nonexistent/brain-poison-test.db"));
        assert_eq!(Arc::strong_count(&again), 2);
    }
}
