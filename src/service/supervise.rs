//! `agent-hub service run`: what the Windows scheduled task runs.
//!
//! Task Scheduler restarts a task that fails to start, not a process that dies
//! later, and a task cannot set environment variables. So the task runs this
//! supervisor, which starts `agent-hub serve` with the pinned config, starts it
//! again when it crashes, and passes a stop request on to it so the store is
//! checkpointed. It never ends the hub itself on a stop: a hub that does not
//! drain is left to `agent-hub stop --force`, which ends the task, and the job
//! object the hub runs in takes the hub with it.
//!
//! The loop is written against [`Host`] and [`Child`], so its decisions (when
//! to restart, how long to wait, when to give up, what a stop does) run in the
//! tests on any platform. The Windows host, with the real process and the real
//! event, is the last part of the file.

// Only the Windows host drives the loop outside the tests; elsewhere it is
// built (and tested) but not reached.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::VecDeque;
use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// How often the loop looks at the hub and at the stop request.
const POLL: Duration = Duration::from_millis(100);
/// How long a stop waits for a hub that has not made its stop event yet before
/// it says so in the startup file. It keeps asking after that.
const EVENT_WAIT: Duration = Duration::from_secs(5);
/// How long a hub usually takes to drain and checkpoint after it was asked to
/// stop (it drains for ten seconds, then folds the store). Past this the
/// supervisor says it is still waiting; it does not end the hub.
const STOP_GRACE: Duration = Duration::from_secs(30);
/// The delay before the first restart, doubled for each crash after it.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Five exits inside this window and the supervisor gives up.
const GIVE_UP_WINDOW: Duration = Duration::from_secs(60);
const GIVE_UP_EXITS: usize = 5;
/// A hub that ran this long before it crashed starts the backoff over.
const STABLE_RUN: Duration = Duration::from_secs(60);

/// What the supervisor does after the hub exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Leave, with this exit code: the hub stopped on purpose (0), or it
    /// refused its configuration or its arguments, which a restart cannot fix.
    Done(u8),
    /// Start it again after this long.
    Restart(Duration),
    /// Too many exits in too little time.
    GiveUp,
}

/// When to start the hub again: the restart policy, as a state machine over the
/// times of its exits.
#[derive(Debug, Default)]
pub struct Restarts {
    exits: VecDeque<Instant>,
    /// Crashes in a row without a stable run between them.
    streak: u32,
}

impl Restarts {
    /// The decision after the hub exited with `code` at `now`, having run for
    /// `ran_for`.
    ///
    /// Exit 0 is a stop someone asked for. 2 (usage) and 78 (configuration)
    /// would fail the same way again. Anything else is a crash: restart after
    /// 1s, 2s, 4s and so on up to 30s, and give up after five exits inside
    /// sixty seconds.
    pub fn decide(&mut self, ran_for: Duration, now: Instant, code: i32) -> Decision {
        match code {
            0 => return Decision::Done(0),
            2 | 78 => return Decision::Done(code as u8),
            _ => {}
        }
        if ran_for >= STABLE_RUN {
            self.streak = 0;
        }
        self.exits.push_back(now);
        while self
            .exits
            .front()
            .is_some_and(|first| now.duration_since(*first) > GIVE_UP_WINDOW)
        {
            self.exits.pop_front();
        }
        if self.exits.len() >= GIVE_UP_EXITS {
            return Decision::GiveUp;
        }
        let delay = BACKOFF_START
            .saturating_mul(1u32.checked_shl(self.streak).unwrap_or(u32::MAX))
            .min(BACKOFF_MAX);
        self.streak = self.streak.saturating_add(1);
        Decision::Restart(delay)
    }
}

/// The hub process, as the loop sees it.
pub trait Child {
    fn id(&self) -> u32;
    /// The exit code once the hub has exited.
    fn exited(&mut self) -> io::Result<Option<i32>>;
    /// Ask the hub to drain and checkpoint. `Ok(false)` when it cannot be asked
    /// yet (it has not made its stop event).
    fn ask_to_stop(&mut self) -> io::Result<bool>;
    /// End the hub at once and wait for it to be gone. Only for a hub that can
    /// no longer be watched, never for a stop.
    fn kill(&mut self) -> io::Result<()>;
}

/// Everything the loop needs from outside itself.
pub trait Host {
    fn spawn(&mut self) -> io::Result<Box<dyn Child>>;
    /// Wait up to `within` for a request to stop the supervisor, and say
    /// whether one has come. Once it has, it stays true. This is also the
    /// loop's clock tick.
    fn stop_requested(&mut self, within: Duration) -> bool;
    fn sleep(&mut self, how_long: Duration);
    fn now(&self) -> Instant;
    /// A line for the startup file, where `agent-hub logs` and a failed `start`
    /// look.
    fn note(&mut self, text: &str);
    /// The hub process now running, or 0 when there is none.
    fn running(&mut self, child: u32);
}

enum Ended {
    /// The supervisor was asked to stop, and the hub has.
    Stopped,
    Exited(i32),
}

/// Supervise the hub until it is stopped on purpose or the supervisor gives up.
/// Returns the supervisor's own exit code.
pub fn supervise(host: &mut dyn Host) -> u8 {
    let mut restarts = Restarts::default();
    loop {
        if host.stop_requested(Duration::ZERO) {
            return 0;
        }
        let began = host.now();
        let ended = match host.spawn() {
            Ok(child) => run_child(host, child),
            Err(err) => {
                host.note(&format!("could not start the hub: {err}"));
                Ended::Exited(1)
            }
        };
        host.running(0);
        let code = match ended {
            Ended::Stopped => return 0,
            Ended::Exited(code) => code,
        };
        let now = host.now();
        match restarts.decide(now.saturating_duration_since(began), now, code) {
            Decision::Done(0) => {
                host.note("the hub stopped; not starting it again");
                return 0;
            }
            Decision::Done(code) => {
                host.note(&format!(
                    "the hub exited with {code}, which a restart cannot fix; not starting it again"
                ));
                return code;
            }
            Decision::GiveUp => {
                host.note(&format!(
                    "the hub exited {GIVE_UP_EXITS} times in {}s; giving up",
                    GIVE_UP_WINDOW.as_secs()
                ));
                return 1;
            }
            Decision::Restart(delay) => {
                host.note(&format!(
                    "the hub exited with {code}; starting it again in {}s",
                    delay.as_secs()
                ));
                if host.stop_requested(delay) {
                    return 0;
                }
            }
        }
    }
}

/// Watch one hub process until it exits or the supervisor is asked to stop.
fn run_child(host: &mut dyn Host, mut child: Box<dyn Child>) -> Ended {
    host.running(child.id());
    loop {
        if host.stop_requested(POLL) {
            stop_child(host, child.as_mut());
            return Ended::Stopped;
        }
        match child.exited() {
            Ok(Some(code)) => return Ended::Exited(code),
            Ok(None) => {}
            Err(err) => {
                host.note(&format!("could not watch the hub: {err}"));
                let _ = child.kill();
                return Ended::Exited(1);
            }
        }
    }
}

/// Ask the hub to stop and wait until it has. A stop never ends the hub: one
/// that does not drain stays until `agent-hub stop --force` ends the task.
fn stop_child(host: &mut dyn Host, child: &mut dyn Child) {
    let began = host.now();
    let mut asked = false;
    let mut said_no_event = false;
    let mut said_slow = false;
    let mut said_error = false;
    loop {
        if matches!(child.exited(), Ok(Some(_))) {
            host.note("the hub stopped");
            return;
        }
        let waited = host.now().saturating_duration_since(began);
        if !asked {
            match child.ask_to_stop() {
                Ok(true) => {
                    asked = true;
                    host.note("asked the hub to stop");
                }
                Ok(false) => {
                    if waited >= EVENT_WAIT && !said_no_event {
                        said_no_event = true;
                        host.note(
                            "the hub has no stop event yet; still asking. agent-hub stop --force \
                             ends it without a checkpoint",
                        );
                    }
                }
                Err(err) => {
                    if !said_error {
                        said_error = true;
                        host.note(&format!(
                            "could not ask the hub to stop: {err}; still trying. agent-hub stop \
                             --force ends it without a checkpoint"
                        ));
                    }
                }
            }
        }
        if asked && waited >= STOP_GRACE && !said_slow {
            said_slow = true;
            host.note(&format!(
                "the hub has not stopped after {}s; still waiting. agent-hub stop --force ends \
                 it without a checkpoint",
                STOP_GRACE.as_secs()
            ));
        }
        host.sleep(POLL);
    }
}

const USAGE: &str = "usage: agent-hub service run --config PATH";

/// The config path from `service run`'s arguments.
fn parse_args(args: &[String]) -> Result<std::path::PathBuf, String> {
    let mut config = None;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        let value = if argument == "--config" {
            rest.next()
                .ok_or_else(|| "--config needs a path".to_string())?
                .clone()
        } else if let Some(value) = argument.strip_prefix("--config=") {
            value.to_string()
        } else {
            return Err(format!("unknown option '{argument}'"));
        };
        if value.is_empty() {
            return Err("--config needs a path".to_string());
        }
        config = Some(std::path::PathBuf::from(value));
    }
    config.ok_or_else(|| "--config is required".to_string())
}

/// `agent-hub service run --config PATH`.
pub fn run(args: &[String]) -> ExitCode {
    let config = match parse_args(args) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("agent-hub service run: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    #[cfg(windows)]
    {
        windows::run(config)
    }
    #[cfg(not(windows))]
    {
        let _ = config;
        eprintln!(
            "agent-hub service run: this is what the Windows scheduled task runs; it does nothing here"
        );
        ExitCode::from(2)
    }
}

#[cfg(windows)]
mod windows {
    use std::fs::File;
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, ExitCode, Stdio};

    use agent_hub::app::stop_event::StopEvent;
    use agent_hub::config::{HUB_KEYS, LifecycleSettings, Os};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    use super::{Child, Host, Instant, supervise};
    use crate::daemon::{lock, logfile, process};
    use crate::service::schtasks::{self, SupervisorRecord};
    use crate::service::{pinned_env, process_env};

    /// A job object that ends every process in it when its last handle
    /// closes: when the supervisor exits, however it exits, the hub goes too.
    struct Job(HANDLE);

    impl Job {
        fn new() -> std::io::Result<Self> {
            // SAFETY: an unnamed job with default security; the result is checked.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the job is open and `limits` is the structure the class names.
            let set = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        fn assign(&self, process: HANDLE) -> std::io::Result<()> {
            // SAFETY: both handles are open.
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: opened in `new` and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Resume every thread of a process created suspended. The number resumed.
    fn resume_threads(pid: u32) -> std::io::Result<usize> {
        // SAFETY: a thread snapshot; the result is checked and closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        let mut resumed = 0;
        // SAFETY: the snapshot is open and `entry` carries its own size.
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: a thread id from the snapshot; the handle is checked and closed.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    // SAFETY: the thread handle is open with THREAD_SUSPEND_RESUME.
                    if unsafe { ResumeThread(thread) } != u32::MAX {
                        resumed += 1;
                    }
                    // SAFETY: opened just above and closed once.
                    unsafe { CloseHandle(thread) };
                }
            }
            // SAFETY: as for Thread32First.
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        // SAFETY: opened above and closed once.
        unsafe { CloseHandle(snapshot) };
        Ok(resumed)
    }

    struct HubProcess(std::process::Child);

    impl Child for HubProcess {
        fn id(&self) -> u32 {
            self.0.id()
        }

        fn exited(&mut self) -> std::io::Result<Option<i32>> {
            Ok(self.0.try_wait()?.map(|status| status.code().unwrap_or(1)))
        }

        fn ask_to_stop(&mut self) -> std::io::Result<bool> {
            match StopEvent::open(self.0.id())? {
                Some(event) => event.signal().map(|()| true),
                None => Ok(false),
            }
        }

        fn kill(&mut self) -> std::io::Result<()> {
            self.0.kill()?;
            self.0.wait().map(|_| ())
        }
    }

    struct Windows {
        settings: LifecycleSettings,
        config: PathBuf,
        /// The supervisor's own stop event, set by `agent-hub stop`.
        stop: StopEvent,
        /// Whether a stop has come; once it has, the record says so.
        stopping: bool,
        child: u32,
        job: Job,
        /// The hub's stdout and stderr, kept across restarts.
        output: File,
        notes: File,
        state_dir: PathBuf,
        start: u64,
    }

    impl Windows {
        fn publish(&mut self) -> std::io::Result<()> {
            SupervisorRecord {
                pid: std::process::id(),
                start: self.start,
                child: self.child,
                stopping: self.stopping,
            }
            .write(&self.state_dir)
        }

        fn publish_or_note(&mut self) {
            if let Err(err) = self.publish() {
                self.note(&format!("could not write the supervisor record: {err}"));
            }
        }
    }

    impl Host for Windows {
        fn spawn(&mut self) -> std::io::Result<Box<dyn Child>> {
            let exe = std::env::current_exe()?;
            let mut command = Command::new(exe);
            command.arg("serve");
            // The hub reads the pinned config and nothing the task's
            // environment holds: no `HUB_*` setting overrides it.
            for key in HUB_KEYS {
                command.env_remove(format!("HUB_{}", key.to_ascii_uppercase()));
            }
            command
                .env("HUB_CONFIG", &self.config)
                .env("AGENT_HUB_MANAGED_BY", "task");
            if !self.settings.data_dir_set {
                command.env("HUB_DATA_DIR", &self.settings.data_dir);
            }
            if !self.settings.log_file_set {
                command.env("HUB_LOG_FILE", &self.settings.log_file);
            }
            // Suspended, so it is in the job before it runs a single instruction.
            command
                .stdin(Stdio::null())
                .stdout(Stdio::from(self.output.try_clone()?))
                .stderr(Stdio::from(self.output.try_clone()?))
                .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
            let mut child = command.spawn()?;
            let placed = self
                .job
                .assign(child.as_raw_handle() as HANDLE)
                .and_then(|()| resume_threads(child.id()))
                .and_then(|resumed| {
                    if resumed == 0 {
                        Err(std::io::Error::other(
                            "no thread of the hub could be resumed",
                        ))
                    } else {
                        Ok(())
                    }
                });
            if let Err(err) = placed {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    err.kind(),
                    format!("could not put the hub in the supervisor's job: {err}"),
                ));
            }
            Ok(Box::new(HubProcess(child)))
        }

        fn stop_requested(&mut self, within: std::time::Duration) -> bool {
            if self.stopping {
                return true;
            }
            let set = match self.stop.wait(Some(within)) {
                Ok(set) => set,
                Err(err) => {
                    // A failed wait is not a stop. Keep the loop's pace.
                    self.note(&format!("could not wait on the stop event: {err}"));
                    std::thread::sleep(within);
                    false
                }
            };
            if set {
                self.stopping = true;
                self.publish_or_note();
            }
            set
        }

        fn sleep(&mut self, how_long: std::time::Duration) {
            std::thread::sleep(how_long);
        }

        fn now(&self) -> Instant {
            Instant::now()
        }

        fn note(&mut self, text: &str) {
            let when = time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            let _ = writeln!(self.notes, "{when} agent-hub supervisor: {text}");
        }

        fn running(&mut self, child: u32) {
            self.child = child;
            self.publish_or_note();
        }
    }

    pub fn run(config: PathBuf) -> ExitCode {
        let fail = |code: u8, message: String| {
            eprintln!("agent-hub service run: {message}");
            ExitCode::from(code)
        };
        let config = std::path::absolute(&config).unwrap_or(config);
        let env: &dyn Fn(&str) -> Option<String> = &process_env;
        let settings =
            match LifecycleSettings::resolve(&pinned_env(env, Some(&config)), Os::Windows) {
                Ok(settings) => settings,
                Err(err) => return fail(78, format!("{}: {err}", config.display())),
            };
        if let Err(message) = logfile::check_log_path(&settings.log_file, &settings.data_dir) {
            return fail(78, message);
        }
        let Some(state_dir) = schtasks::state_dir(env) else {
            return fail(
                78,
                "LOCALAPPDATA is not set to an absolute path, so there is no state directory"
                    .to_string(),
            );
        };
        if let Err(err) = std::fs::create_dir_all(&state_dir) {
            return fail(
                1,
                format!("could not create {}: {err}", state_dir.display()),
            );
        }

        // One supervisor at a time: the lock is held until this process ends,
        // and a second one leaves before it touches anything.
        let lock_path = state_dir.join(schtasks::SUPERVISOR_LOCK);
        let lock = match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(err) => return fail(1, format!("could not open {}: {err}", lock_path.display())),
        };
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                println!(
                    "agent-hub service run: another supervisor is already running; leaving it"
                );
                return ExitCode::SUCCESS;
            }
            Err(std::fs::TryLockError::Error(err)) => {
                return fail(1, format!("could not lock {}: {err}", lock_path.display()));
            }
        }

        let pid = std::process::id();
        // The stop event exists and the record names it before the hub does:
        // a stop that comes while the hub starts is never lost.
        let stop = match StopEvent::create(pid) {
            Ok(event) => event,
            Err(err) => {
                return fail(
                    1,
                    format!(
                        "could not create the stop event ({err}), so agent-hub stop could not \
                         reach the hub; not starting it"
                    ),
                );
            }
        };
        let job = match Job::new() {
            Ok(job) => job,
            Err(err) => return fail(1, format!("could not create a job object: {err}")),
        };
        let startup = process::startup_file(&settings);
        let output = match logfile::create_startup_file(&startup) {
            Ok(file) => file,
            Err(err) => return fail(1, format!("could not open {}: {err}", startup.display())),
        };
        let notes = match logfile::open_append(&startup) {
            Ok(file) => file,
            Err(err) => return fail(1, format!("could not open {}: {err}", startup.display())),
        };
        let mut host = Windows {
            settings,
            config,
            stop,
            stopping: false,
            child: 0,
            job,
            output,
            notes,
            state_dir: state_dir.clone(),
            start: lock::process_start_ticks(pid).unwrap_or(0),
        };
        if let Err(err) = host.publish() {
            return fail(
                1,
                format!(
                    "could not write the supervisor record in {}: {err}",
                    state_dir.display()
                ),
            );
        }
        host.note("started");
        let code = supervise(&mut host);
        SupervisorRecord::remove(&state_dir);
        host.note(&format!("exiting with {code}"));
        drop(lock);
        ExitCode::from(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn at(base: Instant, seconds: u64) -> Instant {
        base + Duration::from_secs(seconds)
    }

    #[test]
    fn a_clean_exit_or_a_refusal_is_not_restarted() {
        let base = Instant::now();
        let mut restarts = Restarts::default();
        assert_eq!(
            restarts.decide(Duration::from_secs(5), at(base, 5), 0),
            Decision::Done(0)
        );
        assert_eq!(
            restarts.decide(Duration::from_secs(1), at(base, 6), 78),
            Decision::Done(78)
        );
        assert_eq!(
            restarts.decide(Duration::from_secs(1), at(base, 7), 2),
            Decision::Done(2)
        );
    }

    #[test]
    fn crashes_back_off_one_two_four_eight_and_then_it_gives_up() {
        let base = Instant::now();
        let mut restarts = Restarts::default();
        let mut now = 0;
        let mut delays = Vec::new();
        for _ in 0..4 {
            match restarts.decide(Duration::from_secs(1), at(base, now), 1) {
                Decision::Restart(delay) => {
                    delays.push(delay.as_secs());
                    now += delay.as_secs() + 1;
                }
                other => panic!("expected a restart, got {other:?}"),
            }
        }
        assert_eq!(delays, vec![1, 2, 4, 8]);
        // The fifth exit inside sixty seconds.
        assert_eq!(
            restarts.decide(Duration::from_secs(1), at(base, now), 1),
            Decision::GiveUp
        );
    }

    #[test]
    fn the_delay_stops_growing_at_thirty_seconds() {
        let base = Instant::now();
        let mut restarts = Restarts::default();
        let mut delays = Vec::new();
        // One crash a minute apart never fills the window, so only the delay grows.
        for round in 0..8u64 {
            match restarts.decide(Duration::from_secs(1), at(base, round * 61), 1) {
                Decision::Restart(delay) => delays.push(delay.as_secs()),
                other => panic!("round {round}: {other:?}"),
            }
        }
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn a_stable_run_starts_the_backoff_over() {
        let base = Instant::now();
        let mut restarts = Restarts::default();
        for round in 0..3u64 {
            let _ = restarts.decide(Duration::from_secs(1), at(base, round * 61), 1);
        }
        assert_eq!(
            restarts.decide(Duration::from_secs(120), at(base, 400), 1),
            Decision::Restart(Duration::from_secs(1))
        );
    }

    #[test]
    fn old_exits_leave_the_window() {
        let base = Instant::now();
        let mut restarts = Restarts::default();
        for second in [0, 1, 3, 7] {
            assert!(matches!(
                restarts.decide(Duration::from_secs(1), at(base, second), 1),
                Decision::Restart(_)
            ));
        }
        // A fifth exit, but the first four are more than a minute old.
        assert!(matches!(
            restarts.decide(Duration::from_secs(1), at(base, 200), 1),
            Decision::Restart(_)
        ));
    }

    // ---- the loop, with a scripted hub and a virtual clock ----

    #[derive(Default)]
    struct World {
        now: Option<Instant>,
        clock: Duration,
        /// For each spawn, how long the hub runs and how it ends.
        script: VecDeque<Result<(Duration, i32), String>>,
        /// Virtual time at which a stop request arrives, if one does.
        stop_at: Option<Duration>,
        /// Whether the hub makes its stop event at once, and how long it takes
        /// to drain once asked (None: never exits by itself).
        has_event: bool,
        /// Virtual time from which a hub without an event at first has one.
        event_at: Option<Duration>,
        drain: Option<Duration>,
        spawns: u32,
        notes: Vec<String>,
        running: Vec<u32>,
        asked: bool,
        killed: bool,
    }

    struct FakeHub {
        world: Rc<RefCell<World>>,
        started: Duration,
        lives: Duration,
        code: i32,
        asked_at: Option<Duration>,
    }

    impl Child for FakeHub {
        fn id(&self) -> u32 {
            1000
        }

        fn exited(&mut self) -> io::Result<Option<i32>> {
            let world = self.world.borrow();
            let now = world.clock;
            if let Some(asked) = self.asked_at
                && let Some(drain) = world.drain
                && now >= asked + drain
            {
                return Ok(Some(0));
            }
            if world.killed {
                return Ok(Some(1));
            }
            if now >= self.started + self.lives {
                return Ok(Some(self.code));
            }
            Ok(None)
        }

        fn ask_to_stop(&mut self) -> io::Result<bool> {
            let mut world = self.world.borrow_mut();
            let late = world.event_at.is_some_and(|at| world.clock >= at);
            if !world.has_event && !late {
                return Ok(false);
            }
            world.asked = true;
            self.asked_at = Some(world.clock);
            Ok(true)
        }

        fn kill(&mut self) -> io::Result<()> {
            self.world.borrow_mut().killed = true;
            Ok(())
        }
    }

    struct Fake(Rc<RefCell<World>>);

    impl Host for Fake {
        fn spawn(&mut self) -> io::Result<Box<dyn Child>> {
            let mut world = self.0.borrow_mut();
            world.spawns += 1;
            world.killed = false;
            match world.script.pop_front().expect("a scripted hub") {
                Ok((lives, code)) => Ok(Box::new(FakeHub {
                    world: self.0.clone(),
                    started: world.clock,
                    lives,
                    code,
                    asked_at: None,
                })),
                Err(message) => Err(io::Error::other(message)),
            }
        }

        fn stop_requested(&mut self, within: Duration) -> bool {
            let mut world = self.0.borrow_mut();
            if let Some(at) = world.stop_at
                && world.clock >= at
            {
                return true;
            }
            world.clock += within;
            matches!(world.stop_at, Some(at) if world.clock >= at)
        }

        fn sleep(&mut self, how_long: Duration) {
            self.0.borrow_mut().clock += how_long;
        }

        fn now(&self) -> Instant {
            let world = self.0.borrow();
            world.now.expect("a base instant") + world.clock
        }

        fn note(&mut self, text: &str) {
            self.0.borrow_mut().notes.push(text.to_string());
        }

        fn running(&mut self, child: u32) {
            self.0.borrow_mut().running.push(child);
        }
    }

    fn world(script: Vec<Result<(Duration, i32), String>>) -> Rc<RefCell<World>> {
        Rc::new(RefCell::new(World {
            now: Some(Instant::now()),
            script: script.into(),
            has_event: true,
            drain: Some(Duration::from_secs(2)),
            ..World::default()
        }))
    }

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn a_crashing_hub_is_restarted_until_the_supervisor_gives_up() {
        let world = world((0..10).map(|_| Ok((secs(1), 1))).collect());
        let code = supervise(&mut Fake(world.clone()));
        let world = world.borrow();
        assert_eq!(code, 1);
        assert_eq!(
            world.spawns, 5,
            "five exits inside a minute, then it stops trying"
        );
        assert!(
            world.notes.iter().any(|note| note.contains("giving up")),
            "{:?}",
            world.notes
        );
        assert_eq!(world.running.iter().filter(|pid| **pid == 0).count(), 5);
    }

    #[test]
    fn a_hub_that_comes_back_after_a_crash_is_supervised_again() {
        let world = world(vec![Ok((secs(3), 1)), Ok((secs(10_000), 0))]);
        world.borrow_mut().stop_at = Some(secs(60));
        let code = supervise(&mut Fake(world.clone()));
        let world = world.borrow();
        assert_eq!(code, 0);
        assert_eq!(world.spawns, 2);
        assert!(world.asked, "the stop reached the second hub");
        assert!(!world.killed, "it drained, so nothing was ended");
    }

    #[test]
    fn a_hub_that_refuses_its_config_is_not_restarted_and_its_code_is_kept() {
        let world = world(vec![Ok((secs(1), 78)), Ok((secs(1), 0))]);
        assert_eq!(supervise(&mut Fake(world.clone())), 78);
        assert_eq!(world.borrow().spawns, 1);
    }

    #[test]
    fn a_clean_exit_ends_the_supervisor_with_zero() {
        let world = world(vec![Ok((secs(5), 0))]);
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        assert_eq!(world.borrow().spawns, 1);
    }

    #[test]
    fn a_stop_is_relayed_to_the_hub_and_the_hub_is_not_ended() {
        let world = world(vec![Ok((secs(10_000), 0))]);
        world.borrow_mut().stop_at = Some(secs(5));
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        let world = world.borrow();
        assert!(world.asked);
        assert!(!world.killed, "no TerminateProcess for a hub that drains");
        assert_eq!(world.spawns, 1, "and it is not started again");
    }

    #[test]
    fn a_stop_during_the_backoff_does_not_start_the_hub_again() {
        let world = world(vec![Ok((secs(1), 1)), Ok((secs(10_000), 0))]);
        // The crash is at 1s; the restart would be at 2s; the stop is at 1.5s.
        world.borrow_mut().stop_at = Some(Duration::from_millis(1500));
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        assert_eq!(world.borrow().spawns, 1);
    }

    #[test]
    fn a_slow_hub_is_waited_for_and_never_ended() {
        let world = world(vec![Ok((secs(10_000), 0))]);
        {
            let mut world = world.borrow_mut();
            world.stop_at = Some(secs(5));
            world.drain = Some(secs(120));
        }
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        let world = world.borrow();
        assert!(world.asked);
        assert!(!world.killed, "a stop never ends the hub");
        assert!(
            world.clock >= secs(5) + secs(120),
            "it waited until the hub drained: {:?}",
            world.clock
        );
        assert!(
            world
                .notes
                .iter()
                .any(|note| note.contains("still waiting")),
            "{:?}",
            world.notes
        );
        assert_eq!(world.spawns, 1);
    }

    #[test]
    fn a_hub_with_no_stop_event_is_asked_again_and_never_ended() {
        // The hub never makes its event and exits by itself at 50s.
        let world = world(vec![Ok((secs(50), 0))]);
        {
            let mut world = world.borrow_mut();
            world.stop_at = Some(secs(5));
            world.has_event = false;
        }
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        let world = world.borrow();
        assert!(!world.asked && !world.killed);
        assert!(
            world
                .notes
                .iter()
                .any(|note| note.contains("no stop event yet")),
            "{:?}",
            world.notes
        );
    }

    #[test]
    fn a_hub_that_makes_its_stop_event_late_is_asked_once_it_has() {
        let world = world(vec![Ok((secs(10_000), 0))]);
        {
            let mut world = world.borrow_mut();
            world.stop_at = Some(secs(1));
            world.has_event = false;
            world.event_at = Some(secs(9));
        }
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        let world = world.borrow();
        assert!(world.asked && !world.killed);
        assert!(world.clock >= secs(9) + secs(2), "{:?}", world.clock);
    }

    #[test]
    fn a_hub_that_will_not_start_counts_as_a_crash() {
        let world = world((0..8).map(|_| Err("no such file".to_string())).collect());
        assert_eq!(supervise(&mut Fake(world.clone())), 1);
        assert_eq!(world.borrow().spawns, 5);
    }

    #[test]
    fn a_stop_that_came_before_the_first_start_starts_nothing() {
        let world = world(vec![Ok((secs(10), 0))]);
        world.borrow_mut().stop_at = Some(Duration::ZERO);
        assert_eq!(supervise(&mut Fake(world.clone())), 0);
        assert_eq!(world.borrow().spawns, 0);
    }

    #[test]
    fn the_arguments_need_a_config_and_refuse_anything_else() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_args(&args(&["--config", "C:\\c.toml"])),
            Ok(std::path::PathBuf::from("C:\\c.toml"))
        );
        assert_eq!(
            parse_args(&args(&["--config=C:\\c.toml"])),
            Ok(std::path::PathBuf::from("C:\\c.toml"))
        );
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["--config"])).is_err());
        assert!(parse_args(&args(&["--config", "a", "--bogus"])).is_err());
    }
}
