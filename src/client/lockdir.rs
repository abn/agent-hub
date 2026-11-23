//! The lock-dir protocol shared with the OpenCode plugin and the launcher.
//!
//! `mkdir <target>.lock` is the lock; `<target>.lock/owner` holds
//! `<pid> <host> <epoch_ms>`. A stale lock is broken only while holding
//! `<target>.lock.break`, after re-reading the owner under it. Owner and temp
//! files are created exclusively with mode 0600. Not `flock`: Node has no
//! portable one and the launcher is POSIX sh.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Result;

/// How long to wait and when a lock counts as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockPolicy {
    /// How long to retry a busy lock, every 50 ms, before giving up.
    pub wait: Duration,
    /// The age past which a lock whose owner is gone may be broken.
    pub stale_after: Duration,
}

impl LockPolicy {
    /// `config.toml` writers: wait 5 s, stale after 30 s.
    pub const CONFIG: Self = Self {
        wait: Duration::from_secs(5),
        stale_after: Duration::from_secs(30),
    };
}

/// A held lock. Dropping it releases it.
#[derive(Debug)]
pub struct LockGuard {
    dir: PathBuf,
}

impl LockGuard {
    /// The lock directory held.
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

/// Take the lock on `target` (the lock is `<target>.lock`). A lock still busy
/// after the wait answers `config.toml is locked by pid <p> on <host>`, with
/// `unknown` when the owner file cannot be read.
pub fn acquire(target: &Path, policy: LockPolicy) -> Result<LockGuard> {
    let _ = (target, policy);
    Err(crate::error::Error::Unavailable(
        "lockdir::acquire is not built yet".to_string(),
    ))
}
