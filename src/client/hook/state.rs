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
//! there is no pid to go stale. `session-end` deletes that file only while it
//! holds the lock itself, and a watcher that finds its locked file no longer
//! the one at the path does not run, so the lock always lives on one inode.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::input::Harness;

/// A subagent's hub session, found by the harness's id for the subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Child {
    /// The session name, `<parent>/<child>`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// A `session_leave` the hub has not taken yet. It holds no token: a retry
/// resolves the hub and the member again from `cwd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leave {
    pub project_id: String,
    pub name: String,
    /// Where the hook that wanted the leave ran, which a retry resolves from.
    pub cwd: String,
    /// The harness session the child was mapped under, for a subagent's leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    /// The harness's id for the subagent, for a subagent's leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// Epoch milliseconds of the last attempt.
    #[serde(default)]
    pub tried_ms: u64,
}

/// The leaves of one harness that are still owed, across its sessions.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leaves {
    #[serde(default)]
    pub pending: Vec<Leave>,
}

/// What one harness session remembers between hook runs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The member the hub says this session acts as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The subagents that have started and not stopped, by the harness's
    /// `agent_id`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub children: BTreeMap<String, Child>,
    /// The last line a hook printed about the connection, so a state that does
    /// not change is said once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
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

/// The variables that name the home directory, in order. `USERPROFILE` is the
/// Windows fallback only: on Unix it is not a home directory, so it is not read.
pub(crate) const HOME_VARS: &[&str] = if cfg!(windows) {
    &["HOME", "USERPROFILE"]
} else {
    &["HOME"]
};

/// The home directory from the first of `HOME_VARS` that is set and not empty.
pub fn home_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    home_from(env, HOME_VARS)
}

fn home_from(env: &dyn Fn(&str) -> Option<String>, vars: &[&str]) -> Option<String> {
    vars.iter()
        .find_map(|key| env(key).filter(|home| !home.is_empty()))
}

/// The directory of one harness's state files.
pub fn state_dir(harness: Harness, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_STATE_HOME")
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir(env).map(|home| Path::new(&home).join(".local").join("state")))
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
        self.read_as()
    }

    /// The file as last written, read as `T`, or empty.
    pub fn read_as<T: serde::de::DeserializeOwned + Default>(&self) -> T {
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
        self.update_as(change)
    }

    /// [`Store::update`] for a file that holds a `T`.
    pub fn update_as<T: serde::Serialize + serde::de::DeserializeOwned + Default>(
        &self,
        change: impl FnOnce(&mut T),
    ) -> T {
        let held = self.lock_state();
        let mut state = self.read_as::<T>();
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

    fn write(&self, state: &impl Serialize) {
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

    /// Delete the watcher lock file when no watcher holds it. The lock is taken
    /// first and the file removed while it is held, so a lock another process
    /// holds is never deleted.
    pub fn remove_watch_lock_if_free(&self) {
        let Ok(file) = OpenOptions::new().write(true).open(&self.watch_lock) else {
            return;
        };
        if file.try_lock().is_ok() {
            std::fs::remove_file(&self.watch_lock).ok();
        }
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
        // A cleanup that removed the file while this process waited to open it
        // leaves a lock on a file nobody else will find.
        same_file(&file, &self.watch_lock).then_some(WatcherLock { _file: file })
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

#[cfg(test)]
mod tests {
    use super::{HOME_VARS, Harness, home_dir, home_from, state_dir};
    use std::path::PathBuf;

    const WINDOWS_VARS: &[&str] = &["HOME", "USERPROFILE"];

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn home_prefers_home_over_userprofile() {
        let env = env_of(&[("HOME", "/users/a"), ("USERPROFILE", "C:\\Users\\a")]);
        assert_eq!(home_dir(&env).as_deref(), Some("/users/a"));
        assert_eq!(home_from(&env, WINDOWS_VARS).as_deref(), Some("/users/a"));
    }

    #[test]
    fn windows_falls_back_to_userprofile_when_home_is_unset_or_empty() {
        let unset = env_of(&[("USERPROFILE", "C:\\Users\\a")]);
        assert_eq!(
            home_from(&unset, WINDOWS_VARS).as_deref(),
            Some("C:\\Users\\a")
        );
        let empty = env_of(&[("HOME", ""), ("USERPROFILE", "C:\\Users\\a")]);
        assert_eq!(
            home_from(&empty, WINDOWS_VARS).as_deref(),
            Some("C:\\Users\\a")
        );
    }

    #[test]
    fn home_is_none_when_nothing_is_set() {
        assert_eq!(home_dir(&env_of(&[])), None);
        assert_eq!(
            home_from(&env_of(&[("HOME", ""), ("USERPROFILE", "")]), WINDOWS_VARS),
            None
        );
    }

    #[test]
    fn userprofile_is_read_only_where_it_is_a_home() {
        let env = env_of(&[("USERPROFILE", "/profile")]);
        assert_eq!(
            home_dir(&env).is_some(),
            cfg!(windows),
            "vars: {HOME_VARS:?}"
        );
        assert_eq!(HOME_VARS.contains(&"USERPROFILE"), cfg!(windows));
    }

    #[test]
    fn state_dir_follows_the_platform_home() {
        let env = env_of(&[("USERPROFILE", "/profile")]);
        let dir = state_dir(Harness::Claude, &env);
        let base: PathBuf = if cfg!(windows) {
            ["/profile", ".local", "state"].iter().collect()
        } else {
            [".local", "state"].iter().collect()
        };
        assert_eq!(dir, base.join("agent-hub").join("hooks").join("claude"));
    }
}
