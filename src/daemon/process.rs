//! The plain-process backend: a hub with no service manager behind it.
//!
//! `start` spawns `agent-hub serve` detached and waits for it, `stop` signals
//! the pid the lock holder's record names, and `logs` reads the log file. A service
//! backend replaces all of it when one is installed (see [`super::backend`]).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use agent_hub::config::LifecycleSettings;

use super::lock::{self, Holder, Mode, Record};
use super::logfile::{self, Follower};
use super::{Backend, LogsOptions, Managed, StopOptions};

/// How long `start` waits for the child to bind and answer `/readyz`.
const START_WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(50);
/// How many log lines `start` prints when the child dies first.
const FAILURE_LOG_LINES: usize = 20;
/// How long one connect may take.
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);
/// The whole of one probe, connect to last byte. A peer that trickles bytes
/// cannot hold a caller past it.
const PROBE_DEADLINE: Duration = Duration::from_secs(3);
/// The most a probe reads. `/readyz` answers in a few hundred bytes.
const PROBE_MAX_BYTES: usize = 64 * 1024;

/// Where the startup file lives for these settings: see
/// [`logfile::startup_path`].
pub fn startup_file(settings: &LifecycleSettings) -> PathBuf {
    logfile::startup_path(&settings.log_file)
}

/// A hub started and stopped by this binary, with no service manager.
pub struct Process;

impl Backend for Process {
    fn managed(&self, _settings: &LifecycleSettings, holder: Option<&Record>) -> Managed {
        match holder {
            Some(record) if record.mode == Mode::Mcp => Managed {
                kind: "process",
                label: "an embedded agent-hub mcp session".to_string(),
                enabled: false,
                starts_at: "manual",
            },
            Some(record) if record.managed_by != "process" => Managed {
                kind: kind_of(&record.managed_by),
                label: record.manager_label().to_string(),
                enabled: false,
                starts_at: "manual",
            },
            Some(_) => Managed {
                kind: "process",
                label: "agent-hub start (a background process)".to_string(),
                enabled: false,
                starts_at: "manual",
            },
            None => Managed {
                kind: "none",
                label: "nothing (no service installed)".to_string(),
                enabled: false,
                starts_at: "manual",
            },
        }
    }

    fn restart(&self, settings: &LifecycleSettings, options: &StopOptions) -> ExitCode {
        if let Some(code) = unsupported_here("restart") {
            return code;
        }
        let stopped = self.stop(settings, options);
        if stopped != ExitCode::SUCCESS {
            return stopped;
        }
        self.start(settings)
    }

    fn start(&self, settings: &LifecycleSettings) -> ExitCode {
        if let Some(code) = unsupported_here("start") {
            return code;
        }
        let data_dir = &settings.data_dir;
        if let Some(code) = wait_for_a_holder(data_dir) {
            return code;
        }

        if let Err(message) = logfile::check_log_path(&settings.log_file, data_dir) {
            return fail(message);
        }
        if let Err(message) = check_port(settings) {
            return fail(message);
        }

        // The hub writes its log through its own rotating writer. What it prints
        // before that, and what it dies with, goes to a separate small file that
        // nothing rotates.
        let startup = startup_file(settings);
        let out = match logfile::create_startup_file(&startup) {
            Ok(file) => file,
            Err(err) => {
                return fail(format!(
                    "could not open the startup file {}: {err}",
                    startup.display()
                ));
            }
        };
        let err_out = match out.try_clone() {
            Ok(file) => file,
            Err(err) => return fail(format!("could not open the startup file: {err}")),
        };
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => return fail(format!("could not find the agent-hub binary: {err}")),
        };

        let mut command = Command::new(exe);
        command
            .arg("serve")
            .env("AGENT_HUB_MANAGED_BY", "process")
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err_out);
        if !settings.data_dir_set {
            command.env("HUB_DATA_DIR", data_dir);
        }
        if !settings.log_file_set {
            command.env("HUB_LOG_FILE", &settings.log_file);
        }
        detach(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => return fail(format!("could not start the hub: {err}")),
        };

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("agent-hub start: the hub exited at once ({status})");
                    print_failure_output(&startup, &settings.log_file);
                    return ExitCode::FAILURE;
                }
                Ok(None) => {}
                Err(err) => return fail(format!("could not watch the hub start: {err}")),
            }
            if let Ok(Holder::Held(Some(record))) = lock::inspect(data_dir)
                && record.pid == child.id()
                && let Some(url) = &record.url
            {
                if ready(&record) {
                    println!("started (pid {}) at {url}/", record.pid);
                    return ExitCode::SUCCESS;
                }
                if started.elapsed() >= START_WAIT {
                    return fail(format!(
                        "the hub (pid {}) is listening at {url} but /readyz is not ready after {}s; \
                         see agent-hub logs",
                        record.pid,
                        START_WAIT.as_secs()
                    ));
                }
            } else if started.elapsed() >= START_WAIT {
                return fail(format!(
                    "the hub (pid {}) did not report an address within {}s; see agent-hub logs",
                    child.id(),
                    START_WAIT.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    fn stop(&self, settings: &LifecycleSettings, options: &StopOptions) -> ExitCode {
        if let Some(code) = unsupported_here("stop") {
            return code;
        }
        let data_dir = &settings.data_dir;
        let record = match lock::inspect(data_dir) {
            Ok(Holder::Free) => {
                println!("not running");
                return ExitCode::SUCCESS;
            }
            Ok(Holder::Held(Some(record))) if record.mode == Mode::Mcp => {
                return refuse_embedded(&record, "stop");
            }
            Ok(Holder::Held(Some(record))) => record,
            Ok(Holder::Held(None)) => {
                return fail(format!(
                    "a process holds {} but no current record names it (a hub still starting, \
                     or a restore); nothing was signalled. Try again",
                    lock::path_in(data_dir).display()
                ));
            }
            Err(err) => return fail(format!("could not read {}: {err}", data_dir.display())),
        };

        print!("Stopping the hub (pid {})... ", record.pid);
        let _ = std::io::stdout().flush();
        let began = Instant::now();
        if let Err(message) = signal_holder(data_dir, &record, options.force) {
            println!();
            return fail(message);
        }

        loop {
            match lock::inspect(data_dir) {
                Ok(Holder::Free) => break,
                Ok(Holder::Held(_)) => {}
                Err(err) => {
                    println!();
                    return fail(format!("could not read {}: {err}", data_dir.display()));
                }
            }
            if began.elapsed() >= options.timeout {
                println!();
                return fail(format!(
                    "the hub (pid {}) did not stop within {}s; agent-hub stop --force kills it \
                     without a checkpoint",
                    record.pid,
                    options.timeout.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }

        let seconds = began.elapsed().as_secs_f64();
        let clean = lock::last_record(data_dir)
            .is_some_and(|last| last.pid == record.pid && last.clean_stop);
        if clean {
            println!("stopped after {seconds:.1}s, store checkpointed.");
        } else if options.force {
            println!(
                "killed after {seconds:.1}s, no checkpoint; agent-hub check verifies the store."
            );
        } else {
            println!(
                "stopped after {seconds:.1}s, without a clean-shutdown record; \
                 agent-hub check verifies the store."
            );
        }
        ExitCode::SUCCESS
    }

    fn logs(&self, settings: &LifecycleSettings, options: &LogsOptions) -> ExitCode {
        let path = &settings.log_file;
        // Follow from the end the file has now, then print the tail, so a line
        // written between the two is shown at most twice and never lost.
        let mut follower = options.follow.then(|| Follower::at_end(path));
        match logfile::last_lines(path, options.lines) {
            Ok(lines) => {
                let mut out = std::io::stdout().lock();
                for line in lines {
                    if writeln!(out, "{line}").is_err() {
                        return ExitCode::SUCCESS;
                    }
                }
                let _ = out.flush();
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if follower.is_none() {
                    return fail(format!("no log yet at {}", path.display()));
                }
                eprintln!("agent-hub logs: waiting for {}", path.display());
            }
            Err(err) => return fail(format!("could not read {}: {err}", path.display())),
        }
        let Some(follower) = follower.as_mut() else {
            return ExitCode::SUCCESS;
        };
        loop {
            match follower.poll() {
                Ok(bytes) if !bytes.is_empty() => {
                    let mut out = std::io::stdout().lock();
                    if out.write_all(&bytes).and_then(|()| out.flush()).is_err() {
                        return ExitCode::SUCCESS;
                    }
                }
                Ok(_) => {}
                Err(err) => return fail(format!("could not read {}: {err}", path.display())),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

fn kind_of(managed_by: &str) -> &'static str {
    match managed_by {
        "systemd" => "systemd",
        "launchd" => "launchd",
        "task" => "task",
        _ => "process",
    }
}

fn fail(message: String) -> ExitCode {
    eprintln!("agent-hub: {message}");
    ExitCode::FAILURE
}

/// What `start` does when a process already holds the lock. `None` means the
/// lock is free and a hub should be started.
///
/// A ready hub is success. A holder with no current record yet (a hub still
/// starting, or a restore swapping the directory), or a hub that does not
/// answer yet, is waited for: `start` succeeds only on a serve record whose
/// `/readyz` answers. A lock freed while waiting means it is ours to start.
fn wait_for_a_holder(data_dir: &Path) -> Option<ExitCode> {
    let began = Instant::now();
    let mut said = false;
    loop {
        match lock::inspect(data_dir) {
            Ok(Holder::Free) => return None,
            Ok(Holder::Held(Some(record))) if record.mode == Mode::Mcp => {
                return Some(refuse_embedded(&record, "start"));
            }
            Ok(Holder::Held(Some(record))) if ready(&record) => {
                println!("already running (pid {})", record.pid);
                return Some(ExitCode::SUCCESS);
            }
            Ok(Holder::Held(holder)) => {
                if began.elapsed() >= START_WAIT {
                    let who = match holder {
                        Some(record) => format!("the hub (pid {})", record.pid),
                        None => "a process with no current record".to_string(),
                    };
                    return Some(fail(format!(
                        "{who} holds {} and no ready hub answered within {}s; see agent-hub \
                         status and agent-hub logs",
                        lock::path_in(data_dir).display(),
                        START_WAIT.as_secs()
                    )));
                }
                if !said {
                    eprintln!("agent-hub start: a hub is starting; waiting for it to answer");
                    said = true;
                }
            }
            Err(err) => {
                return Some(fail(format!(
                    "could not read {}: {err}",
                    data_dir.display()
                )));
            }
        }
        std::thread::sleep(POLL);
    }
}

/// What a process that died at once said: its startup file first, which holds
/// the refusal or panic it exited with, and its own log when that was empty.
fn print_failure_output(startup: &Path, log: &Path) {
    for path in [startup, log] {
        if let Ok(lines) = logfile::last_lines(path, FAILURE_LOG_LINES)
            && !lines.is_empty()
        {
            eprintln!("the last {} lines of {}:", lines.len(), path.display());
            for line in lines {
                eprintln!("  {line}");
            }
            return;
        }
    }
    eprintln!("the hub said nothing; see {}", log.display());
}

/// The refusal for a verb that acts on a hub when an embedded MCP session holds
/// the store: it has no listener to start and no drain to ask for.
fn refuse_embedded(record: &Record, verb: &str) -> ExitCode {
    eprintln!(
        "agent-hub {verb}: an embedded agent-hub mcp session (pid {}) holds the store, not a \
         hub; end that session first. {verb} acts only on a hub started by agent-hub start or serve",
        record.pid
    );
    ExitCode::FAILURE
}

/// Process mode is not available on Windows yet: the verbs need the hub's pid
/// and a stop signal that reach across a console boundary, which the Windows
/// service provides. `status` and `logs` do not need either.
fn unsupported_here(verb: &str) -> Option<ExitCode> {
    if !cfg!(windows) {
        return None;
    }
    eprintln!(
        "agent-hub {verb}: running the hub as a plain background process is not supported on \
         Windows; the Windows service provides start, stop and restart (agent-hub service)"
    );
    Some(ExitCode::from(69))
}

/// Whether the hub the record names answers `/readyz` with a 200.
pub fn ready(record: &Record) -> bool {
    probe_record(record).is_some_and(|(status, _)| status == 200)
}

/// `/readyz` of the hub a record names: the status and body, or `None` when
/// nothing answered.
pub fn probe_record(record: &Record) -> Option<(u16, String)> {
    let bind: SocketAddr = record.bind.as_deref()?.parse().ok()?;
    http_get(lock::display_address(bind), "/readyz").ok()
}

/// Refuse a start when the configured address is taken, and say by whom.
fn check_port(settings: &LifecycleSettings) -> Result<(), String> {
    let bind = settings.bind;
    match std::net::TcpListener::bind(bind) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            let port = bind.port();
            let fix = "set [hub] bind in config.toml to use another address";
            Err(match probe_hub(bind) {
                Some(()) => format!(
                    "port {port} is held by another hub, not one serving {}; a container? {fix}",
                    settings.data_dir.display()
                ),
                None => format!("port {port} is in use by another program; {fix}"),
            })
        }
        Err(err) => Err(format!("cannot bind {bind}: {err}")),
    }
}

/// Whether a hub answers on `address`: a ready answer from `/readyz`.
pub fn probe_hub(address: SocketAddr) -> Option<()> {
    let (status, body) = http_get(lock::display_address(address), "/readyz").ok()?;
    (status == 200 && body.contains("\"status\":\"ready\"")).then_some(())
}

/// One plain-HTTP GET, bounded: the status and the body.
pub fn http_get(address: SocketAddr, path: &str) -> std::io::Result<(u16, String)> {
    http_get_within(address, path, PROBE_DEADLINE, PROBE_MAX_BYTES)
}

/// As [`http_get`], with the whole exchange held to `deadline` and the answer to
/// `max_bytes`. A peer that never closes, trickles bytes, or never stops
/// sending ends the probe with an error rather than holding the caller.
fn http_get_within(
    address: SocketAddr,
    path: &str,
    deadline: Duration,
    max_bytes: usize,
) -> std::io::Result<(u16, String)> {
    let end = Instant::now() + deadline;
    let remaining = || {
        end.checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "the probe ran out of time")
            })
    };
    let mut stream = TcpStream::connect_timeout(&address, remaining()?.min(PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(remaining()?))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    )?;
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        stream.set_read_timeout(Some(remaining()?))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if response.len() + read > max_bytes {
                    return Err(std::io::Error::other(
                        "the answer is larger than a probe reads",
                    ));
                }
                response.extend_from_slice(&chunk[..read]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            // A read timeout is reported as WouldBlock on Unix, TimedOut elsewhere.
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the probe ran out of time",
                ));
            }
            Err(err) => return Err(err),
        }
    }
    let response = String::from_utf8_lossy(&response);
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::other("the answer was not HTTP"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.trim().to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// Run the child in its own session, so a closing terminal does not end it.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

/// A process to signal, by a handle that cannot reach another process.
///
/// On Linux that is a pidfd: a signal sent through it reaches the process it
/// was opened on or nobody, even if that process dies and its pid is reused.
/// Elsewhere on Unix it is the bare pid.
#[cfg(unix)]
struct Handle {
    pid: rustix::process::Pid,
    #[cfg(target_os = "linux")]
    fd: Option<std::os::fd::OwnedFd>,
}

#[cfg(unix)]
impl Handle {
    /// Open the process, or `None` when it no longer exists.
    ///
    /// On Linux a pidfd that cannot be opened is refused unless `force` is
    /// set: then the bare pid is used, still after the caller's checks.
    fn open(pid: u32, force: bool) -> Result<Option<Self>, String> {
        use rustix::io::Errno;
        use rustix::process::{Pid, test_kill_process};
        let raw = pid;
        let pid = i32::try_from(pid)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| "the record names no usable pid".to_string())?;
        #[cfg(target_os = "linux")]
        match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
            Ok(fd) => return Ok(Some(Self { pid, fd: Some(fd) })),
            Err(Errno::SRCH) => return Ok(None),
            Err(err) if !force => {
                return Err(format!(
                    "could not open a pidfd for pid {raw} ({err}), so the signal cannot be held \
                     to that one process; nothing was signalled. agent-hub stop --force signals \
                     the bare pid after the same checks"
                ));
            }
            Err(_) => {}
        }
        #[cfg(not(target_os = "linux"))]
        let _ = force;
        match test_kill_process(pid) {
            Ok(()) | Err(Errno::PERM) => Ok(Some(Self {
                pid,
                #[cfg(target_os = "linux")]
                fd: None,
            })),
            Err(Errno::SRCH) => Ok(None),
            Err(err) => Err(format!("could not reach pid {raw}: {err}")),
        }
    }

    /// SIGTERM so the process drains and folds the store, or SIGKILL when
    /// `force` says the checkpoint is not wanted. A process already gone is not
    /// an error: the lock is released next, which the caller waits for.
    fn signal(&self, force: bool) -> std::io::Result<()> {
        use rustix::io::Errno;
        use rustix::process::{Signal, kill_process};
        let signal = if force { Signal::KILL } else { Signal::TERM };
        #[cfg(target_os = "linux")]
        let sent = match &self.fd {
            Some(fd) => rustix::process::pidfd_send_signal(fd, signal),
            None => kill_process(self.pid, signal),
        };
        #[cfg(not(target_os = "linux"))]
        let sent = kill_process(self.pid, signal);
        match sent {
            Ok(()) | Err(Errno::SRCH) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

/// Signal the process that holds `hub.lock`, after proving it is that process.
///
/// The process is opened first (a pidfd on Linux), then the record is read
/// again and the lock asked again: the lock must still be held, the record
/// must name the same holder the caller read, and its pid must still be the
/// process that wrote it (the same start time and pid namespace). The record
/// was read before the pidfd was opened and names a process that started
/// before that, so a start time that still matches proves the pidfd is on it.
/// A lock that has been freed means nothing is signalled. A record that does
/// not hold up is an error, never a signal at whatever now has that number.
///
/// Elsewhere on Unix there is no start time to compare and no pidfd, so the
/// check is the held lock, the same record, and a pid that exists: weaker, and
/// said so in the platform notes.
#[cfg(unix)]
fn signal_holder(data_dir: &Path, record: &Record, force: bool) -> Result<(), String> {
    let handle = Handle::open(record.pid, force)?;
    let current = lock::last_record(data_dir);
    let held = lock::is_held(data_dir)
        .map_err(|err| format!("could not read {}: {err}", data_dir.display()))?;
    if !held {
        return Ok(());
    }
    match current {
        Some(now) if now.same_holder(record) && now.names_a_live_process() => match handle {
            Some(handle) => handle
                .signal(force)
                .map_err(|err| format!("could not signal pid {}: {err}", record.pid)),
            None => Ok(()),
        },
        _ => Err(format!(
            "the record names pid {}, which is no longer the process holding {}; not \
             signalling it. Try again",
            record.pid,
            lock::path_in(data_dir).display()
        )),
    }
}

#[cfg(not(unix))]
fn signal_holder(_data_dir: &Path, _record: &Record, _force: bool) -> Result<(), String> {
    Err("stopping a hub by pid is not supported on this platform yet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A server that runs `behave` on the first connection it gets.
    fn serve_once(
        behave: impl FnOnce(TcpStream) + Send + 'static,
    ) -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let handle = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                behave(stream);
            }
        });
        (address, handle)
    }

    /// Read the request through its blank line, so closing the socket afterwards
    /// does not reset a request the peer is still sending.
    fn read_request(stream: &mut TcpStream) {
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        while !seen.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
            seen.push(byte[0]);
        }
    }

    #[test]
    fn a_probe_reads_a_small_answer() {
        let (address, handle) = serve_once(|mut stream| {
            read_request(&mut stream);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}");
        });
        let (status, body) = http_get(address, "/readyz").expect("probe");
        assert_eq!((status, body.as_str()), (200, "{\"status\":\"ok\"}"));
        handle.join().unwrap();
    }

    #[test]
    fn a_peer_that_trickles_bytes_cannot_hold_a_probe_past_its_deadline() {
        let (address, handle) = serve_once(|mut stream| {
            read_request(&mut stream);
            // One byte every 50ms, forever: never closes, never goes quiet
            // for long enough to trip a per-read timeout.
            while stream.write_all(b"x").is_ok() {
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let began = Instant::now();
        let err = http_get_within(address, "/", Duration::from_millis(600), PROBE_MAX_BYTES)
            .expect_err("a trickle is not an answer");
        let took = began.elapsed();
        assert!(took < Duration::from_millis(1500), "took {took:?}");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        handle.join().unwrap();
    }

    #[test]
    fn a_peer_that_never_stops_sending_is_cut_off_at_the_size_cap() {
        let (address, handle) = serve_once(|mut stream| {
            read_request(&mut stream);
            let block = [b'x'; 4096];
            while stream.write_all(&block).is_ok() {}
        });
        let began = Instant::now();
        let err = http_get_within(address, "/", Duration::from_secs(10), 16 * 1024)
            .expect_err("an endless body is not an answer");
        assert!(began.elapsed() < Duration::from_secs(5));
        assert!(
            err.to_string().contains("larger than a probe reads"),
            "{err}"
        );
        handle.join().unwrap();
    }

    #[test]
    fn a_peer_that_accepts_and_says_nothing_times_out() {
        let (address, handle) = serve_once(|stream| {
            std::thread::sleep(Duration::from_millis(1500));
            drop(stream);
        });
        let began = Instant::now();
        assert!(http_get_within(address, "/", Duration::from_millis(300), 1024).is_err());
        assert!(began.elapsed() < Duration::from_secs(1));
        handle.join().unwrap();
    }
}
