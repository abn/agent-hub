//! Per-session state under `$XDG_STATE_HOME/agent-hub/hooks/<harness>/`, never
//! the system temporary directory. It caches the resolved names and the
//! pacing times, so every field is optional and a missing or unreadable file
//! reads as empty.
//!
//! Every change is a read-modify-write under an exclusive lock on a sibling
//! `.lock` file and lands through a staging file and a rename, so two hook
//! processes of one session never interleave. The lock is tried in a loop until
//! the invocation's deadline and never waited on, so a stuck holder cannot hold
//! a hook past its time. A watcher holds a kernel lock on its own `.watch.lock`
//! file for its whole life, which the kernel releases when the process dies, so
//! there is no pid to go stale. That file is never deleted: its lock lives on
//! one stable inode.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::input::Harness;

/// What one harness session remembers between hook runs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default)]
    pub prompts: u64,
    /// Epoch milliseconds of the last peek a prompt reserved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_peek_ms: Option<u64>,
    /// Epoch milliseconds of the last wake, which the cooldown reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wake_ms: Option<u64>,
    /// Epoch milliseconds of the last start or configuration that failed, so a
    /// hub that is down is not retried on every prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_failed_ms: Option<u64>,
}

/// The directory of one harness's state files.
pub fn state_dir(harness: Harness, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_STATE_HOME")
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| Path::new(&home).join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("agent-hub").join("hooks").join(harness.name())
}

fn file_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.json")
}

/// Distinguishes the staging files one process writes.
static STAGING: AtomicU64 = AtomicU64::new(0);

/// One session's state file and the locks beside it.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    lock: PathBuf,
    watch_lock: PathBuf,
    /// The state lock is not waited for past this.
    deadline: Instant,
}

/// How often a held state lock is tried again.
const LOCK_RETRY: Duration = Duration::from_millis(5);

/// The exclusive hold on a session's state, released when dropped.
#[derive(Debug)]
struct StateLock {
    _file: File,
}

impl Store {
    pub fn new(dir: &Path, harness_session_id: &str, deadline: Instant) -> Self {
        let path = dir.join(file_name(harness_session_id));
        Self {
            lock: path.with_extension("lock"),
            watch_lock: path.with_extension("watch.lock"),
            path,
            deadline,
        }
    }

    /// The state as last written, or empty. A write is a rename, so a read sees
    /// a whole file and needs no lock.
    pub fn read(&self) -> State {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Whether the state file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Take the state lock, trying until the deadline. `None` when the
    /// directory or the lock file cannot be had, or the lock stays held past
    /// the deadline, in which case a change is dropped, as a failed write is.
    fn lock_state(&self) -> Option<StateLock> {
        std::fs::create_dir_all(self.path.parent()?).ok()?;
        loop {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&self.lock)
                .ok()?;
            loop {
                match file.try_lock() {
                    Ok(()) => break,
                    Err(TryLockError::WouldBlock) if Instant::now() < self.deadline => {
                        std::thread::sleep(LOCK_RETRY);
                    }
                    Err(_) => return None,
                }
            }
            // A holder that removed the lock file while this one waited leaves
            // a lock on a file nobody else will find. Try again on the new one.
            if same_file(&file, &self.lock) {
                return Some(StateLock { _file: file });
            }
            if Instant::now() >= self.deadline {
                return None;
            }
        }
    }

    /// Change the state and write it back. A write that fails is dropped: the
    /// state caches pacing times, and the hook works without it.
    pub fn update(&self, change: impl FnOnce(&mut State)) -> State {
        let held = self.lock_state();
        let mut state = self.read();
        change(&mut state);
        if held.is_some() {
            self.write(&state);
        }
        state
    }

    /// Change the state only if it still exists. A watcher uses this so that
    /// a session that ended is not written back to life.
    pub fn update_existing(&self, change: impl FnOnce(&mut State)) -> Option<State> {
        let _held = self.lock_state()?;
        if !self.path.exists() {
            return None;
        }
        let mut state = self.read();
        change(&mut state);
        self.write(&state);
        Some(state)
    }

    /// Claim the next peek: `true`, and the time recorded, when at least `gap_ms`
    /// passed since the last claim. The check and the record are one step under
    /// the lock, so two prompts that arrive together cannot both peek. Without
    /// the lock nothing is claimed, so nothing is peeked.
    pub fn reserve_peek(&self, now_ms: u64, gap_ms: u64) -> bool {
        let Some(_held) = self.lock_state() else {
            return false;
        };
        let mut state = self.read();
        if state
            .last_peek_ms
            .is_some_and(|at| now_ms.saturating_sub(at) < gap_ms)
        {
            return false;
        }
        state.last_peek_ms = Some(now_ms);
        self.write(&state);
        true
    }

    fn write(&self, state: &State) {
        let Some(dir) = self.path.parent() else {
            return;
        };
        let Ok(text) = serde_json::to_string(state) else {
            return;
        };
        let staging = dir.join(format!(
            "{}.{}.{}.tmp",
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id(),
            STAGING.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::write(&staging, text).is_err() || std::fs::rename(&staging, &self.path).is_err()
        {
            std::fs::remove_file(&staging).ok();
        }
    }

    /// Remove the state, which tells a watcher of the session to stop, and the
    /// state lock file beside it. The watcher's lock file stays. `false`, and
    /// nothing removed, when the state lock could not be taken: the watcher
    /// then stops at its own time.
    pub fn remove(&self) -> bool {
        let Some(_held) = self.lock_state() else {
            return false;
        };
        std::fs::remove_file(&self.path).ok();
        std::fs::remove_file(&self.lock).ok();
        true
    }

    /// Take the watcher lock for this process, which lasts as long as the
    /// returned value. `None` when another watcher of the session holds it, and
    /// also when the lock cannot be had for any other reason: a watcher that
    /// cannot be sure it is the only one does not run.
    pub fn lock_watcher(&self) -> Option<WatcherLock> {
        std::fs::create_dir_all(self.watch_lock.parent()?).ok()?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.watch_lock)
            .ok()?;
        file.try_lock().ok()?;
        Some(WatcherLock { _file: file })
    }
}

/// The watcher lock, released by the kernel when this value or the process goes.
#[derive(Debug)]
pub struct WatcherLock {
    _file: File,
}

#[cfg(unix)]
fn same_file(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(named)) => held.dev() == named.dev() && held.ino() == named.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_file: &File, path: &Path) -> bool {
    path.exists()
}

/// The current time in epoch milliseconds.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
