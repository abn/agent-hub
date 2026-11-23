//! The lock-dir protocol shared with the OpenCode plugin and the launcher.
//!
//! `mkdir <target>.lock` is the lock; `<target>.lock/owner` holds
//! `<pid> <host> <epoch_ms>`. A stale lock is broken only while holding
//! `<target>.lock.break`, after re-reading the owner under it. The guard holds
//! its own `owner` file, and is reaped only when its owner is provably gone,
//! never by age alone. Owner and temp files are created exclusively with mode
//! 0600. Not `flock`: Node has no portable one and the launcher is POSIX sh.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

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

/// How long a break guard with no owner file, or held from another host, must
/// sit before it is reaped.
const GUARD_REAP_AFTER: Duration = Duration::from_secs(60);

/// The pause between two tries on a busy lock.
const RETRY_EVERY: Duration = Duration::from_millis(50);

/// A held lock. Dropping it releases it.
#[derive(Debug)]
pub struct LockGuard {
    dir: PathBuf,
    line: String,
}

impl LockGuard {
    /// The lock directory held.
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Only the holder whose line is in `owner` releases, so a lock broken
        // and retaken meanwhile is never removed by its old holder.
        if read_owner_line(&self.dir).as_deref() == Some(self.line.as_str()) {
            let _ = std::fs::remove_file(self.dir.join("owner"));
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

/// Where a breaker is, for a test that pauses it there.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The lock read as stale and the guard is not taken yet.
    StaleSeen,
    /// The guard is taken and its owner file written.
    GuardHeld,
    /// The lock was re-read as stale under the guard; nothing is renamed yet.
    BeforeRename,
}

/// The clock, process identity and pause points an acquire runs against. The
/// real ones are the default; a test substitutes its own.
#[doc(hidden)]
pub struct Env<'a> {
    pub pid: u32,
    pub host: String,
    /// Milliseconds since the epoch.
    pub now: &'a (dyn Fn() -> u64 + Sync),
    /// Whether a process on this host is running.
    pub alive: &'a (dyn Fn(u32) -> bool + Sync),
    pub pause: &'a (dyn Fn(Stage) + Sync),
}

fn real_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// This machine's name, for a test building its own environment.
#[doc(hidden)]
pub fn real_host() -> String {
    host_name()
}

/// Whether a process on this machine runs, for a test building its own
/// environment.
#[doc(hidden)]
pub fn real_alive(pid: u32) -> bool {
    pid_alive(pid)
}

/// Take the lock on `target` (the lock is `<target>.lock`). A lock still busy
/// after the wait answers `config.toml is locked by pid <p> on <host>`, with
/// `unknown` when the owner file cannot be read.
pub fn acquire(target: &Path, policy: LockPolicy) -> Result<LockGuard> {
    let env = Env {
        pid: std::process::id(),
        host: host_name(),
        now: &real_now,
        alive: &pid_alive,
        pause: &|_| {},
    };
    acquire_with(target, policy, &env)
}

/// [`acquire`] against a substituted environment.
#[doc(hidden)]
pub fn acquire_with(target: &Path, policy: LockPolicy, env: &Env<'_>) -> Result<LockGuard> {
    let dir = sibling(target, ".lock");
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_private_dir(parent)?;
    }
    let line = format!("{} {} {}", env.pid, env.host, (env.now)());
    let deadline = (env.now)().saturating_add(millis(policy.wait));

    let mut held = try_create(&dir, &line)?;
    while !held {
        if break_stale(&dir, policy, env)? && try_create(&dir, &line)? {
            break;
        }
        if (env.now)() >= deadline {
            let (pid, host) = read_owner(&dir).map_or_else(
                || ("unknown".to_string(), "unknown".to_string()),
                |owner| (owner.pid.to_string(), owner.host),
            );
            return Err(Error::Unavailable(format!(
                "config.toml is locked by pid {pid} on {host}"
            )));
        }
        std::thread::sleep(RETRY_EVERY);
        held = try_create(&dir, &line)?;
    }
    Ok(LockGuard { dir, line })
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// `<target><suffix>`, beside the target.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(Error::Io)
}

/// Make the lock directory and write its owner. False when it exists.
fn try_create(dir: &Path, line: &str) -> Result<bool> {
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(err) => return Err(Error::Io(err)),
    }
    let temp = dir.join("owner.tmp");
    let write = create_private(&temp, format!("{line}\n").as_bytes())
        .and_then(|()| std::fs::rename(&temp, dir.join("owner")));
    if let Err(err) = write {
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_dir(dir);
        return Err(Error::Io(err));
    }
    Ok(true)
}

/// Create a file exclusively, mode 0600, and write it before anyone reads it.
/// A leftover at the path is removed, never reused.
pub(crate) fn create_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// A parsed `owner` file: `<pid> <host> <epoch_ms>`.
struct Owner {
    pid: u32,
    host: String,
    at: u64,
}

fn read_owner_line(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("owner")).ok()?;
    Some(text.trim().to_string())
}

fn read_owner(dir: &Path) -> Option<Owner> {
    let line = read_owner_line(dir)?;
    let mut parts = line.split_whitespace();
    let pid = parts.next()?;
    let host = parts.next()?;
    let at = parts.next()?;
    if !pid.bytes().all(|b| b.is_ascii_digit()) || !at.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Owner {
        pid: pid.parse().ok()?,
        host: host.to_string(),
        at: at.parse().ok()?,
    })
}

/// The age of a directory by its mtime, in ms, or none when it is gone.
fn dir_age(dir: &Path, env: &Env<'_>) -> Option<u64> {
    let modified = std::fs::metadata(dir).ok()?.modified().ok()?;
    let at = modified.duration_since(UNIX_EPOCH).ok()?;
    let at = u64::try_from(at.as_millis()).unwrap_or(u64::MAX);
    Some((env.now)().saturating_sub(at))
}

#[derive(Debug, PartialEq, Eq)]
enum LockState {
    Gone,
    Held,
    Stale,
}

fn lock_state(dir: &Path, policy: LockPolicy, env: &Env<'_>) -> LockState {
    let owner = read_owner(dir);
    let age = match &owner {
        Some(owner) => (env.now)().saturating_sub(owner.at),
        None => match dir_age(dir, env) {
            Some(age) => age,
            None => return LockState::Gone,
        },
    };
    if age <= millis(policy.stale_after) {
        return LockState::Held;
    }
    if let Some(owner) = &owner
        && owner.host == env.host
        && (env.alive)(owner.pid)
    {
        return LockState::Held;
    }
    LockState::Stale
}

/// Whether a break guard's owner is provably gone.
fn guard_reapable(guard: &Path, env: &Env<'_>) -> bool {
    match read_owner(guard) {
        Some(owner) if owner.host == env.host => !(env.alive)(owner.pid),
        Some(owner) => (env.now)().saturating_sub(owner.at) > millis(GUARD_REAP_AFTER),
        None => dir_age(guard, env).is_some_and(|age| age > millis(GUARD_REAP_AFTER)),
    }
}

fn random_suffix() -> String {
    use rand::RngCore;
    format!("{:08x}", rand::rng().next_u32())
}

/// Break a stale lock. Only the holder of `<lock>.break` renames or removes
/// `<lock>`, and it decides staleness again once it holds the guard. True when
/// the lock is gone and the caller may take it.
fn break_stale(dir: &Path, policy: LockPolicy, env: &Env<'_>) -> Result<bool> {
    match lock_state(dir, policy, env) {
        LockState::Gone => return Ok(true),
        LockState::Held => return Ok(false),
        LockState::Stale => {}
    }
    (env.pause)(Stage::StaleSeen);

    let guard = sibling(dir, ".break");
    let own_line = format!("{} {} {}", env.pid, env.host, (env.now)());
    if !take_guard(&guard, &own_line, env)? {
        return Ok(false);
    }
    (env.pause)(Stage::GuardHeld);

    let outcome: Result<bool> = (|| {
        match lock_state(dir, policy, env) {
            LockState::Gone => return Ok(true),
            LockState::Held => return Ok(false),
            LockState::Stale => {}
        }
        (env.pause)(Stage::BeforeRename);
        // The guard may have been reaped and retaken while this breaker was
        // paused: only its own owner line lets it touch the lock.
        if read_owner_line(&guard).as_deref() != Some(own_line.as_str()) {
            return Ok(false);
        }
        let aside = sibling(dir, &format!(".stale-{}-{}", env.pid, random_suffix()));
        if std::fs::rename(dir, &aside).is_err() {
            return Ok(false);
        }
        let _ = std::fs::remove_dir_all(&aside);
        Ok(true)
    })();

    if read_owner_line(&guard).as_deref() == Some(own_line.as_str()) {
        let _ = std::fs::remove_file(guard.join("owner"));
        let _ = std::fs::remove_dir(&guard);
    }
    outcome
}

/// Make the guard directory and write its owner. A guard held by a live owner
/// is left alone; one whose owner is provably gone is reaped once.
fn take_guard(guard: &Path, own_line: &str, env: &Env<'_>) -> Result<bool> {
    for attempt in 0..2 {
        match std::fs::create_dir(guard) {
            Ok(()) => {
                // The owner file is exclusive: a guard that cannot name its
                // owner is not held.
                if let Err(err) =
                    create_private(&guard.join("owner"), format!("{own_line}\n").as_bytes())
                {
                    let _ = std::fs::remove_dir_all(guard);
                    return Err(Error::Io(err));
                }
                return Ok(true);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if attempt == 1 || !guard_reapable(guard, env) {
                    return Ok(false);
                }
                let aside = sibling(guard, &format!(".stale-{}-{}", env.pid, random_suffix()));
                if std::fs::rename(guard, &aside).is_err() {
                    return Ok(false);
                }
                let _ = std::fs::remove_dir_all(&aside);
            }
            Err(err) => return Err(Error::Io(err)),
        }
    }
    Ok(false)
}

/// This machine's name, as recorded in an owner file.
fn host_name() -> String {
    host_name_impl()
        .filter(|name| !name.is_empty() && !name.contains(char::is_whitespace))
        .unwrap_or_else(|| "localhost".to_string())
}

#[cfg(unix)]
fn host_name_impl() -> Option<String> {
    rustix::system::uname()
        .nodename()
        .to_str()
        .ok()
        .map(str::to_string)
}

#[cfg(not(unix))]
fn host_name_impl() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Whether a process on this machine is running. A process another account
/// owns counts as running.
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    use rustix::io::Errno;
    use rustix::process::{Pid, test_kill_process};
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return false;
    };
    match test_kill_process(pid) {
        Ok(()) => true,
        Err(errno) => errno == Errno::PERM,
    }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: OpenProcess only reads its arguments; a null result is checked.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return std::io::Error::last_os_error().raw_os_error()
            != Some(ERROR_INVALID_PARAMETER as i32);
    }
    // SAFETY: the handle was just returned by OpenProcess and is owned here.
    drop(unsafe { OwnedHandle::from_raw_handle(raw as _) });
    true
}
