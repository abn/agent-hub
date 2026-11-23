//! `hub.lock`: one process per store, taken before the engine opens.
//!
//! The lock is an exclusive OS lock on `<data>/hub.lock`, so the kernel drops it
//! when the process dies and a stale record can never block a start. The lock
//! file stays empty. Who holds it is said in `<data>/hub.pid.json`, which the
//! holder writes whole (a temporary file renamed over it) once it has the lock.
//! A reader reads the record without the lock, then asks whether the lock is
//! held: a record is trusted only while it is, and only when its pid is still
//! the process that wrote it.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_hub::ops::restore::{HUB_LOCK, HUB_RECORD, lock_store_file};
use serde::{Deserialize, Serialize};

/// The lock file's name under the data directory.
pub const LOCK_FILE: &str = HUB_LOCK;

/// The record's name under the data directory.
pub const RECORD_FILE: &str = HUB_RECORD;

/// How long a start waits for a lock the previous process is still releasing.
pub const ACQUIRE_WAIT: Duration = Duration::from_secs(5);

/// How long a reader waits for the record of a holder that is still starting.
const RECORD_WAIT: Duration = Duration::from_millis(750);

const RETRY_EVERY: Duration = Duration::from_millis(50);

/// What the process holding the lock is doing with the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `agent-hub serve`: an HTTP hub. The only mode `stop` and `start` act on.
    #[default]
    Serve,
    /// Embedded `agent-hub mcp`: one stdio session over the store, no listener.
    Mcp,
}

/// What the lock file says about the process that holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub pid: u32,
    /// Whether the holder is a hub or an embedded MCP session.
    #[serde(default)]
    pub mode: Mode,
    /// The holder's start time as the system counts it (clock ticks since boot
    /// on Linux, the creation time on Windows). With the pid it names one
    /// process, so a reused pid does not pass for the holder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ticks: Option<u64>,
    /// The holder's pid namespace, so a pid recorded inside a container is not
    /// read as a process of the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_ns: Option<String>,
    /// The address clients reach the hub at. Absent until the bind succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The address the listener got. Absent until the bind succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    pub started_at: String,
    pub version: String,
    /// `systemd`, `launchd`, `task` or `process`.
    pub managed_by: String,
    /// Set by a process that stopped after folding the store's log.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clean_stop: bool,
}

impl Record {
    /// A short name for who runs the hub, as the refusal and `status` say it.
    pub fn manager_label(&self) -> &'static str {
        if self.mode == Mode::Mcp {
            return "embedded mcp session";
        }
        match self.managed_by.as_str() {
            "systemd" => "systemd user service",
            "launchd" => "launchd agent",
            "task" => "scheduled task",
            _ => "background process",
        }
    }
}

/// Who runs this process, from the environment the supervisor left it.
///
/// An explicit `AGENT_HUB_MANAGED_BY` wins, which is how `start` marks its child
/// and how a supervisor that is neither systemd nor launchd names itself.
pub fn managed_by_from_env() -> String {
    managed_by(&|key| std::env::var(key).ok())
}

fn managed_by(env: &dyn Fn(&str) -> Option<String>) -> String {
    let set = |key: &str| env(key).filter(|value| !value.trim().is_empty());
    if let Some(value) = set("AGENT_HUB_MANAGED_BY") {
        return value;
    }
    if set("INVOCATION_ID").is_some() {
        return "systemd".to_string();
    }
    // Terminal sessions on macOS carry XPC_SERVICE_NAME=0.
    if set("XPC_SERVICE_NAME").is_some_and(|name| name != "0") {
        return "launchd".to_string();
    }
    "process".to_string()
}

/// The refusal a second `serve` gives, naming the holder.
pub fn held_message(data_dir: &Path, holder: Option<&Record>) -> String {
    if let Some(record) = holder.filter(|record| record.mode == Mode::Mcp) {
        return format!(
            "an embedded agent-hub mcp session (pid {}) holds {}; end that session, or set \
             HUB_URL to reach a hub",
            record.pid,
            data_dir.display()
        );
    }
    match holder {
        Some(record) => format!(
            "a hub is already serving {} (pid {}, {}); use agent-hub stop, or set HUB_URL to \
             reach it",
            data_dir.display(),
            record.pid,
            record.manager_label()
        ),
        None => format!(
            "another process holds {} (a hub still starting, or a restore); use agent-hub \
             status to see which, or set HUB_URL to reach a hub",
            path_in(data_dir).display()
        ),
    }
}

/// Why the lock was not taken.
#[derive(Debug)]
pub enum AcquireError {
    /// Another process holds it, and held it for the whole wait.
    Held(Option<Box<Record>>),
    Io(std::io::Error),
}

/// The held lock. Dropping it, or the process dying, releases it.
#[derive(Debug)]
pub struct HubLock {
    _file: File,
    data_dir: PathBuf,
    record: Record,
}

impl HubLock {
    /// Take the lock, retrying until `wait` has passed, and write the record.
    ///
    /// The retry covers a service manager that restarts the hub while the old
    /// process is still draining, and a restore that is swapping the directory
    /// (see [`lock_store_file`]). The caller creates the data directory. The
    /// record is the first thing written, before any other work, so the
    /// previous holder's record is replaced as soon as the lock changes hands.
    pub fn acquire(data_dir: &Path, wait: Duration, mode: Mode) -> Result<Self, AcquireError> {
        let path = path_in(data_dir);
        let file = match lock_store_file(&path, wait) {
            Ok(Some(file)) => file,
            Ok(None) => {
                let holder = current_record(data_dir).map(Box::new);
                return Err(AcquireError::Held(holder));
            }
            Err(err) => return Err(AcquireError::Io(err)),
        };
        // An earlier version wrote its record into the lock file itself.
        if file.metadata().map_err(AcquireError::Io)?.len() > 0 {
            file.set_len(0).map_err(AcquireError::Io)?;
        }

        let pid = std::process::id();
        let record = Record {
            pid,
            mode,
            start_ticks: process_start_ticks(pid),
            pid_ns: pid_namespace(),
            url: None,
            bind: None,
            started_at: now_rfc3339(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            managed_by: managed_by_from_env(),
            clean_stop: false,
        };
        let lock = Self {
            _file: file,
            data_dir: data_dir.to_path_buf(),
            record,
        };
        lock.write_record().map_err(AcquireError::Io)?;
        Ok(lock)
    }

    /// Record the address the listener got. Called once the bind succeeded.
    pub fn record_bound(&mut self, address: SocketAddr) {
        self.record.bind = Some(address.to_string());
        self.record.url = Some(format!("http://{}", display_address(address)));
        if let Err(err) = self.write_record() {
            tracing::warn!(error = %err, "could not record the bound address");
        }
    }

    /// Record that the store was folded and the process is stopping cleanly.
    pub fn mark_clean_stop(&mut self) {
        self.record.clean_stop = true;
        if let Err(err) = self.write_record() {
            tracing::warn!(error = %err, "could not record the clean stop");
        }
    }

    /// Write the whole record to a temporary file beside it and rename that
    /// over it: a reader sees the old record or the new one, never a part.
    fn write_record(&self) -> std::io::Result<()> {
        let mut bytes = serde_json::to_vec(&self.record)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        bytes.push(b'\n');
        let temp = self
            .data_dir
            .join(format!(".{RECORD_FILE}.{}.tmp", self.record.pid));
        // A temporary file a crash left behind is this pid's own, from an
        // earlier process that had the same number.
        let _ = std::fs::remove_file(&temp);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(&temp).and_then(|mut file| {
            file.write_all(&bytes)?;
            file.flush()
        });
        if let Err(err) = written.and_then(|()| std::fs::rename(&temp, record_path(&self.data_dir)))
        {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
        Ok(())
    }
}

/// When a live process started, as the system counts it. `None` for a process
/// that is gone, or on a platform that does not say.
///
/// Linux: field 22 of `/proc/<pid>/stat`, in clock ticks since boot. The
/// command name is in parentheses and may hold spaces and parentheses, so the
/// fields are counted from the last closing one. Windows: the process's
/// creation time.
pub fn process_start_ticks(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = text.get(text.rfind(')')? + 1..)?;
        // `after` starts at field 3 (the state), so field 22 is index 19.
        after.split_whitespace().nth(19)?.parse().ok()
    }
    #[cfg(windows)]
    {
        windows_start_time(pid)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = pid;
        None
    }
}

#[cfg(windows)]
fn windows_start_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    /// The exit code a process reports while it is still running.
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: the handle is checked before use and closed once; the calls only
    // write the values they are given.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let mut code = 0u32;
        let timed = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        let coded = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        if timed == 0 || coded == 0 || code != STILL_ACTIVE {
            return None;
        }
        Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// This process's pid namespace, as `pid:[4026531836]`.
pub fn pid_namespace() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link("/proc/self/ns/pid")
            .ok()
            .map(|link| link.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

impl Record {
    /// Whether two records name the same holder: the same process, started at
    /// the same moment. Ignores what the holder rewrites as it starts up.
    #[cfg(unix)]
    pub fn same_holder(&self, other: &Record) -> bool {
        self.pid == other.pid
            && self.started_at == other.started_at
            && self.start_ticks == other.start_ticks
            && self.mode == other.mode
    }

    /// Whether the pid in this record is a live process of this host's pid
    /// namespace and the same one that wrote the record.
    ///
    /// On Linux that compares the kernel's start time and the namespace, and on
    /// Windows the creation time. Elsewhere on Unix only the pid's existence is
    /// checked, which is the best a reader can do there.
    pub fn names_a_live_process(&self) -> bool {
        #[cfg(any(target_os = "linux", windows))]
        {
            let Some(recorded) = self.start_ticks else {
                return false;
            };
            if self.pid_ns != pid_namespace() {
                return false;
            }
            process_start_ticks(self.pid) == Some(recorded)
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            use rustix::process::{Pid, test_kill_process};
            i32::try_from(self.pid)
                .ok()
                .and_then(Pid::from_raw)
                .is_some_and(|pid| {
                    matches!(
                        test_kill_process(pid),
                        Ok(()) | Err(rustix::io::Errno::PERM)
                    )
                })
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }
}

/// The address a client should dial: a wildcard bind answers on loopback.
pub fn display_address(address: SocketAddr) -> SocketAddr {
    if address.ip().is_unspecified() {
        let loopback: std::net::IpAddr = match address {
            SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        };
        SocketAddr::new(loopback, address.port())
    } else {
        address
    }
}

/// Who holds the lock right now.
#[derive(Debug, PartialEq, Eq)]
pub enum Holder {
    /// Nobody: the lock was granted and given back, so any record is history.
    Free,
    /// A process holds it. The record is `None` until the holder has written
    /// its own: a missing or unreadable record, or one whose pid is no longer
    /// the process that wrote it (the previous holder's), reads as `None`.
    Held(Option<Record>),
}

/// Ask whether a process holds `data_dir`'s lock, without disturbing it.
///
/// The record is read first and the lock asked after, so a record that comes
/// back was on disk while the lock was held. A missing lock file means no hub
/// has ever served here.
pub fn inspect(data_dir: &Path) -> std::io::Result<Holder> {
    let record = current_record_now(data_dir);
    if !is_held(data_dir)? {
        return Ok(Holder::Free);
    }
    if record.is_some() {
        return Ok(Holder::Held(record));
    }
    // A holder writes its record as soon as it has the lock: give it a moment.
    Ok(Holder::Held(current_record(data_dir)))
}

/// Whether some process holds the lock: a shared lock that is refused.
pub fn is_held(data_dir: &Path) -> std::io::Result<bool> {
    let file = match OpenOptions::new().read(true).open(path_in(data_dir)) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// The record, if it names a live process that is the one that wrote it,
/// waiting a short while for one that is being written.
fn current_record(data_dir: &Path) -> Option<Record> {
    let deadline = Instant::now() + RECORD_WAIT;
    loop {
        if let Some(record) = current_record_now(data_dir) {
            return Some(record);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(RETRY_EVERY);
    }
}

fn current_record_now(data_dir: &Path) -> Option<Record> {
    last_record(data_dir).filter(Record::names_a_live_process)
}

/// The record the last holder left, whoever wrote it and whether or not it is
/// current.
pub fn last_record(data_dir: &Path) -> Option<Record> {
    let mut text = String::new();
    File::open(record_path(data_dir))
        .ok()?
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(text.trim()).ok()
}

/// The lock file's path, for a caller that names it.
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCK_FILE)
}

/// The record's path.
pub fn record_path(data_dir: &Path) -> PathBuf {
    data_dir.join(RECORD_FILE)
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("daemon-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch");
        root
    }

    #[test]
    fn a_second_acquire_is_refused_and_names_the_holder() {
        let dir = scratch("second");
        let first = HubLock::acquire(&dir, Duration::ZERO, Mode::Serve).expect("first lock");
        match HubLock::acquire(&dir, Duration::from_millis(100), Mode::Serve) {
            Err(AcquireError::Held(Some(record))) => assert_eq!(record.pid, std::process::id()),
            other => panic!("expected a refusal naming the holder, got {other:?}"),
        }
        drop(first);
        HubLock::acquire(&dir, Duration::ZERO, Mode::Serve).expect("the lock is free after a drop");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn inspect_sees_a_holder_and_a_free_lock() {
        let dir = scratch("inspect");
        assert_eq!(inspect(&dir).expect("no file"), Holder::Free);
        let mut lock = HubLock::acquire(&dir, Duration::ZERO, Mode::Serve).expect("lock");
        lock.record_bound("0.0.0.0:8080".parse().unwrap());
        match inspect(&dir).expect("held") {
            Holder::Held(Some(record)) => {
                assert_eq!(record.url.as_deref(), Some("http://127.0.0.1:8080"));
                assert_eq!(record.bind.as_deref(), Some("0.0.0.0:8080"));
            }
            other => panic!("expected a held lock with a record, got {other:?}"),
        }
        lock.mark_clean_stop();
        drop(lock);
        assert_eq!(inspect(&dir).expect("freed"), Holder::Free);
        assert!(last_record(&dir).expect("record stays").clean_stop);
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_lock_file_stays_empty_and_the_record_is_written_whole() {
        let dir = scratch("record-file");
        // An earlier version left its record in the lock file.
        std::fs::write(path_in(&dir), "{\"pid\":1}\n").expect("old lock file");
        let mut lock = HubLock::acquire(&dir, Duration::ZERO, Mode::Serve).expect("lock");
        assert_eq!(std::fs::metadata(path_in(&dir)).expect("lock").len(), 0);
        lock.record_bound("127.0.0.1:18080".parse().unwrap());
        lock.record.url = None;
        lock.record.bind = None;
        lock.write_record().expect("rewrite");
        let text = std::fs::read_to_string(record_path(&dir)).expect("read");
        assert_eq!(text.trim().lines().count(), 1, "one line: {text}");
        assert!(last_record(&dir).is_some(), "still parses: {text}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name != LOCK_FILE && name != RECORD_FILE)
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temporary file left: {leftovers:?}"
        );
        assert_eq!(std::fs::metadata(path_in(&dir)).expect("lock").len(), 0);
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_previous_holders_record_is_not_taken_for_the_current_one() {
        let dir = scratch("stale-record");
        // The lock is held, and the record on disk names a process that is
        // gone: what a new holder looks like before it has written its own.
        let held = lock_store_file(&path_in(&dir), Duration::ZERO)
            .expect("lock")
            .expect("free");
        let source = scratch("stale-record-source");
        let lock =
            HubLock::acquire(&source, Duration::ZERO, Mode::Serve).expect("a real record to copy");
        let stale = Record {
            pid: u32::MAX - 1,
            ..lock.record.clone()
        };
        std::fs::write(
            record_path(&dir),
            serde_json::to_string(&stale).expect("json"),
        )
        .expect("stale record");
        assert_eq!(inspect(&dir).expect("held"), Holder::Held(None));
        drop(held);
        assert_eq!(inspect(&dir).expect("free"), Holder::Free);
        drop(lock);
        std::fs::remove_dir_all(&dir).expect("clean up");
        std::fs::remove_dir_all(&source).expect("clean up");
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_granted_on_a_replaced_file_is_taken_again_on_the_new_one() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch("verify");
        let path = path_in(&dir);
        let old = lock_store_file(&path, Duration::ZERO)
            .expect("lock")
            .expect("free");
        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || lock_store_file(&path, Duration::from_secs(10)))
        };
        std::thread::sleep(Duration::from_millis(200));
        // What a restore does: a new file at the path, locked, then the old
        // lock let go while the new one is still held.
        std::fs::rename(&path, dir.join("aside.lock")).expect("move the old file aside");
        let new = lock_store_file(&path, Duration::ZERO)
            .expect("lock the new file")
            .expect("free");
        drop(old);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !waiter.is_finished(),
            "the waiter settled for the replaced file"
        );
        let new_ino = new.metadata().expect("meta").ino();
        drop(new);
        let got = waiter
            .join()
            .expect("join")
            .expect("lock")
            .expect("granted");
        assert_eq!(got.metadata().expect("meta").ino(), new_ino);
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_manager_comes_from_the_supervisors_environment() {
        let lookup = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(managed_by(&lookup(&[])), "process");
        assert_eq!(managed_by(&lookup(&[("INVOCATION_ID", "abc")])), "systemd");
        assert_eq!(managed_by(&lookup(&[("XPC_SERVICE_NAME", "0")])), "process");
        assert_eq!(
            managed_by(&lookup(&[("XPC_SERVICE_NAME", "dev.agync.agent-hub")])),
            "launchd"
        );
        assert_eq!(
            managed_by(&lookup(&[
                ("INVOCATION_ID", "abc"),
                ("AGENT_HUB_MANAGED_BY", "task")
            ])),
            "task"
        );
    }

    #[test]
    fn the_refusal_names_the_pid_and_the_way_out() {
        let record = Record {
            pid: 4121,
            mode: Mode::Serve,
            start_ticks: None,
            pid_ns: None,
            url: None,
            bind: None,
            started_at: String::new(),
            version: "1".into(),
            managed_by: "systemd".into(),
            clean_stop: false,
        };
        let message = held_message(Path::new("/data"), Some(&record));
        assert!(
            message.contains("pid 4121, systemd user service"),
            "{message}"
        );
        assert!(message.contains("agent-hub stop"), "{message}");
        assert!(message.contains("HUB_URL"), "{message}");
    }

    #[test]
    fn an_older_record_without_a_mode_reads_as_a_hub() {
        let record: Record = serde_json::from_str(
            r#"{"pid":7,"started_at":"t","version":"1","managed_by":"process"}"#,
        )
        .expect("parse");
        assert_eq!(record.mode, Mode::Serve);
        assert_eq!(record.start_ticks, None);
    }

    #[test]
    fn an_embedded_holder_is_named_as_a_session_not_a_hub() {
        let dir = scratch("mcp-mode");
        let lock = HubLock::acquire(&dir, Duration::ZERO, Mode::Mcp).expect("lock");
        drop(lock);
        let record = last_record(&dir).expect("record");
        assert_eq!(record.mode, Mode::Mcp);
        assert_eq!(record.manager_label(), "embedded mcp session");
        let message = held_message(&dir, Some(&record));
        assert!(
            message.contains("embedded agent-hub mcp session"),
            "{message}"
        );
        assert!(!message.contains("a hub is already serving"), "{message}");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_record_names_a_live_process_only_with_its_own_start_time() {
        let dir = scratch("live");
        let lock = HubLock::acquire(&dir, Duration::ZERO, Mode::Serve).expect("lock");
        drop(lock);
        let mut record = last_record(&dir).expect("record");
        assert!(record.start_ticks.is_some() && record.pid_ns.is_some());
        assert!(record.names_a_live_process(), "this process, as it started");

        let mut reused = record.clone();
        reused.start_ticks = reused.start_ticks.map(|ticks| ticks + 1);
        assert!(!reused.names_a_live_process(), "another start time");

        let mut foreign = record.clone();
        foreign.pid_ns = Some("pid:[1]".to_string());
        assert!(!foreign.names_a_live_process(), "another pid namespace");

        record.start_ticks = None;
        assert!(!record.names_a_live_process(), "no start time to compare");

        let gone = Record {
            pid: u32::MAX - 1,
            ..last_record(&dir).expect("record")
        };
        assert!(!gone.names_a_live_process(), "no such process");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[cfg(unix)]
    #[test]
    fn the_holder_is_the_same_across_what_startup_rewrites() {
        let dir = scratch("same-holder");
        let mut lock = HubLock::acquire(&dir, Duration::ZERO, Mode::Serve).expect("lock");
        let before = last_record(&dir).expect("record");
        lock.record_bound("127.0.0.1:9".parse().unwrap());
        lock.mark_clean_stop();
        let after = last_record(&dir).expect("record");
        assert_ne!(before, after);
        assert!(before.same_holder(&after));
        let mut other = after.clone();
        other.pid += 1;
        assert!(!before.same_holder(&other));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
