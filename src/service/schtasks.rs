//! The Windows Task Scheduler manager: a per-user task at logon, driven with
//! `schtasks`.
//!
//! The task runs `conhost.exe --headless agent-hub service run --config PATH`
//! (see [`super::supervise`]), so the hub has no console window and a
//! supervisor restarts it when it crashes. The task is registered from an XML
//! file, which is the only way to give it a logon trigger for one user, no
//! battery limits and no time limit. Nothing here needs an elevated shell
//! except `--boot`, which asks for one and never elevates itself.
//!
//! Output of `schtasks` is localised, so nothing here reads a word of it. State
//! comes from the supervisor's own record (is it alive), the numeric last
//! result of the task, and the task's XML. What the task runs, which uninstall
//! must be sure of, is read through the Task Scheduler's own export (see
//! [`EXPORT_TASK`]), never from console text in a code page.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_hub::config::{LifecycleSettings, Os, present};
use serde::{Deserialize, Serialize};

use super::runner::{Outcome, Runner, SystemRunner};
use super::{Active, Job, Manager, Prepared, StartsAt, Unavailable, UnitSpec, UnitState, Want};
use crate::daemon::LogsOptions;
use crate::daemon::lock;

/// The task's name under the root folder, as `schtasks /TN` takes it.
pub const TASK_NAME: &str = "Agent Hub";

/// The task's URI, as the XML names it.
const TASK_URI: &str = "\\Agent Hub";

/// The rendered task, kept beside the previous binary. It is the service
/// definition on disk: its presence means the service is installed.
pub const DEFINITION_FILE: &str = "agent-hub-task.xml";

/// What the supervisor writes about itself while it runs.
pub const SUPERVISOR_FILE: &str = "supervisor.json";

/// The file a supervisor holds locked for as long as it runs.
pub const SUPERVISOR_LOCK: &str = "supervisor.lock";

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the waits in `start` and `stop` last, kept together so a test can
/// shorten them.
#[derive(Debug, Clone, Copy)]
struct Waits {
    /// For a supervisor that was asked to stop to exit before the task runs
    /// again.
    exit: Duration,
    /// For a new supervisor to show itself after `/Run`.
    appear: Duration,
    /// For a starting supervisor to publish its stop event.
    signal: Duration,
    poll: Duration,
}

const WAITS: Waits = Waits {
    exit: Duration::from_secs(15),
    appear: Duration::from_secs(15),
    signal: Duration::from_secs(5),
    poll: Duration::from_millis(100),
};

/// The task's definition through the Task Scheduler's own API, as hex of its
/// UTF-8 bytes: hex is plain ASCII, so the console's code page cannot change a
/// character of a path on the way. Exit 3 means there is no such task, read
/// from the error's category, which is not localised; any other failure is 4.
const EXPORT_TASK: &str = "$ErrorActionPreference = 'Stop'; \
try { $xml = Export-ScheduledTask -TaskName 'Agent Hub' -TaskPath '\\' } \
catch { if ($_.CategoryInfo.Category -eq 'ObjectNotFound') { exit 3 }; \
[Console]::Error.WriteLine($_.Exception.Message); exit 4 }; \
[BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($xml)).Replace('-', '')";

/// The task's last result while it is running (`SCHED_S_TASK_RUNNING`).
const LAST_RESULT_RUNNING: i64 = 0x0004_1301;
/// The task has not run yet (`SCHED_S_TASK_HAS_NOT_RUN`).
const LAST_RESULT_NOT_RUN: i64 = 0x0004_1303;
/// The task was ended by a user or by `schtasks /End` (`SCHED_S_TASK_TERMINATED`).
const LAST_RESULT_TERMINATED: i64 = 0x0004_1306;
/// The exit code of a hub that refuses its configuration.
const EXIT_CONFIG: i64 = 78;

/// Where the definition and the supervisor's record live for this user:
/// `%LOCALAPPDATA%\agent-hub\state`, the directory the install script keeps
/// the previous binary in.
pub fn state_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    super::purge::state_dir(env, Os::Windows)
}

/// What the supervisor writes: who it is and which hub process it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRecord {
    pub pid: u32,
    /// The supervisor's start time as the system counts it, so a reused pid is
    /// not taken for it.
    pub start: u64,
    /// The hub process it runs now, or 0 between a crash and the restart.
    pub child: u32,
    /// Whether it was asked to stop and is on its way out.
    #[serde(default)]
    pub stopping: bool,
}

impl SupervisorRecord {
    pub fn path_in(state_dir: &Path) -> PathBuf {
        state_dir.join(SUPERVISOR_FILE)
    }

    pub fn read(state_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path_in(state_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn write(&self, state_dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        super::write_definition(&Self::path_in(state_dir), &format!("{text}\n"))
    }

    pub fn remove(state_dir: &Path) {
        let _ = std::fs::remove_file(Self::path_in(state_dir));
    }

    /// Whether the process the record names is still the one that wrote it.
    pub fn alive(&self) -> bool {
        lock::process_start_ticks(self.pid) == Some(self.start)
    }
}

/// Finds the live supervisor whose record is in a state directory.
type Probe = Box<dyn Fn(&Path) -> Option<SupervisorRecord>>;

pub struct Schtasks {
    runner: Box<dyn Runner>,
    state_dir: PathBuf,
    /// The task's working directory: `%LOCALAPPDATA%\agent-hub`.
    work_dir: PathBuf,
    /// Whether this process holds an elevated token. `--boot` needs it.
    elevated: bool,
    /// The user's SID, asked for once.
    sid: RefCell<Option<String>>,
    /// Sets the stop event of the supervisor with this pid: true when it was
    /// set, false when the supervisor has none.
    signal: Box<dyn Fn(u32) -> std::io::Result<bool>>,
    /// The live supervisor whose record is in the state directory, if any.
    probe: Probe,
    waits: Waits,
    /// The installed binary, which the task must run to be this install's.
    binary: Option<PathBuf>,
}

impl Schtasks {
    pub fn host(env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let state_dir = state_dir(env)?;
        let work_dir = present(env("LOCALAPPDATA"))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())?
            .join("agent-hub");
        Some(
            Self::new(Box::new(SystemRunner), state_dir, work_dir, is_elevated())
                .with_binary(std::env::current_exe().ok()),
        )
    }

    /// The installed binary the task has to name to be this install's.
    #[must_use]
    pub fn with_binary(mut self, binary: Option<PathBuf>) -> Self {
        self.binary = binary;
        self
    }

    pub fn new(
        runner: Box<dyn Runner>,
        state_dir: PathBuf,
        work_dir: PathBuf,
        elevated: bool,
    ) -> Self {
        Self {
            runner,
            state_dir,
            work_dir,
            elevated,
            sid: RefCell::new(None),
            signal: Box::new(signal_supervisor),
            probe: Box::new(|dir| SupervisorRecord::read(dir).filter(SupervisorRecord::alive)),
            waits: WAITS,
            binary: None,
        }
    }

    fn schtasks(&self, args: &[&str]) -> std::io::Result<Outcome> {
        self.runner.run("schtasks", args, &[], COMMAND_TIMEOUT)
    }

    /// `schtasks` as a step that must work: its complaint on failure.
    fn must(&self, args: &[&str]) -> Result<Outcome, String> {
        match self.schtasks(args) {
            Ok(outcome) if outcome.ok() => Ok(outcome),
            Ok(outcome) => Err(format!(
                "schtasks {} failed: {}",
                args.first().copied().unwrap_or(""),
                outcome.complaint()
            )),
            Err(err) => Err(format!("could not run schtasks: {err}")),
        }
    }

    fn must_or_unreachable(&self, args: &[&str]) -> Result<Outcome, Unavailable> {
        self.must(args)
            .map_err(|message| Unavailable::after_check(self, message))
    }

    fn missing_schtasks() -> Unavailable {
        Unavailable::none(
            "schtasks was not found, so there is no Task Scheduler to register the hub with. \
             Run the hub with: agent-hub start",
        )
    }

    /// The task's definition as the scheduler holds it, or `None` when there
    /// is no such task. A failure to read it is `Err`: the task is then of
    /// unknown state, never taken for absent.
    fn task_xml(&self) -> Result<Option<String>, Unavailable> {
        let outcome = self
            .runner
            .run(
                "powershell.exe",
                &["-NoProfile", "-NonInteractive", "-Command", EXPORT_TASK],
                &[],
                COMMAND_TIMEOUT,
            )
            .map_err(|err| {
                Unavailable::failed(format!(
                    "could not run powershell.exe to read the task {TASK_URI}: {err}; its state \
                     is unknown"
                ))
            })?;
        match read_export(&outcome) {
            Export::Task(xml) => Ok(Some(xml)),
            Export::Absent => self.check().map(|()| None),
            Export::Unknown(why) => Err(Unavailable::failed(format!(
                "the task {TASK_URI} could not be read ({why}), so what it runs is unknown"
            ))),
        }
    }

    /// Whether the scheduler says the task is running now (its numeric last
    /// result). A query that fails or prints nothing readable is `Err`: the
    /// state is unknown, which is not the same as stopped.
    fn task_running(&self) -> Result<bool, Unavailable> {
        let unknown = |why: String| {
            Unavailable::none(format!(
                "the state of the task {TASK_URI} is unknown ({why}), so nothing was done"
            ))
        };
        let outcome = self
            .schtasks(&["/Query", "/TN", TASK_NAME, "/FO", "CSV", "/NH", "/V"])
            .map_err(|err| unknown(format!("could not run schtasks: {err}")))?;
        if !outcome.ok() {
            return Err(unknown(format!("schtasks failed: {}", outcome.complaint())));
        }
        last_result(&outcome.stdout)
            .map(|result| result == LAST_RESULT_RUNNING)
            .ok_or_else(|| unknown("schtasks printed no readable Last Result".to_string()))
    }

    /// `Ok` with the config the task pins when the task is this install's:
    /// it names a `--config`, and runs the installed binary. A task that
    /// cannot be shown to be ours is refused, never touched.
    fn verified_config(&self, xml: &str) -> Result<PathBuf, Unavailable> {
        let hint = self.unload_hint();
        let config = task_pinned_config(xml).ok_or_else(|| {
            Unavailable::failed(format!(
                "the task {TASK_URI} is registered but names no --config, so what it runs \
                 cannot be verified. Delete it first with: {hint}"
            ))
        })?;
        let installed = self.binary.as_deref().ok_or_else(|| {
            Unavailable::failed(format!(
                "the task {TASK_URI} is registered but the path of this agent-hub is unknown, \
                 so what it runs cannot be verified. Delete it first with: {hint}"
            ))
        })?;
        let runs = task_executable(xml).ok_or_else(|| {
            Unavailable::failed(format!(
                "the task {TASK_URI} is registered but names no program to run, so what it \
                 runs cannot be verified. Delete it first with: {hint}"
            ))
        })?;
        let env: &dyn Fn(&str) -> Option<String> = &super::process_env;
        match installed.to_str() {
            Some(installed) if same_executable(&runs, installed, env) => Ok(config),
            _ => Err(Unavailable::failed(format!(
                "the task {TASK_URI} runs {runs}, not {}, so it is not this install's and is \
                 left alone. Delete it first with: {hint}",
                installed.display()
            ))),
        }
    }

    /// Wait for the task to be idle: no supervisor and the scheduler not
    /// running it. It is never ended here; a task that does not finish on its
    /// own is for `stop --force`.
    fn wait_idle(&self) -> Result<(), Unavailable> {
        let began = Instant::now();
        loop {
            let busy = match self.supervisor() {
                Some(_) => true,
                None => self.task_running()?,
            };
            if !busy {
                return Ok(());
            }
            if began.elapsed() >= self.waits.exit {
                return Err(Unavailable::failed(format!(
                    "the task is still running (not stopped within {}s), so it was not removed. \
                     agent-hub stop --force ends it without a checkpoint; then run this again",
                    self.waits.exit.as_secs()
                )));
            }
            std::thread::sleep(self.waits.poll);
        }
    }

    /// The user's SID, for the task's principal and trigger.
    fn user_sid(&self) -> Result<String, String> {
        if let Some(sid) = self.sid.borrow().as_ref() {
            return Ok(sid.clone());
        }
        let outcome = self
            .runner
            .run(
                "whoami",
                &["/user", "/fo", "csv", "/nh"],
                &[],
                COMMAND_TIMEOUT,
            )
            .map_err(|err| format!("could not run whoami to find your account: {err}"))?;
        if !outcome.ok() {
            return Err(format!(
                "whoami could not name your account: {}",
                outcome.complaint()
            ));
        }
        let sid = parse_whoami_sid(&outcome.stdout).ok_or_else(|| {
            format!(
                "whoami did not print an account SID: {}",
                outcome.stdout.trim()
            )
        })?;
        *self.sid.borrow_mut() = Some(sid.clone());
        Ok(sid)
    }

    fn supervisor(&self) -> Option<SupervisorRecord> {
        (self.probe)(&self.state_dir)
    }

    fn settings(&self) -> Option<LifecycleSettings> {
        let config = self.pinned_config().ok()?;
        let env: &dyn Fn(&str) -> Option<String> = &super::process_env;
        LifecycleSettings::resolve(&super::pinned_env(env, Some(&config)), Os::Windows).ok()
    }
}

impl Manager for Schtasks {
    fn kind(&self) -> &'static str {
        "task"
    }

    fn os(&self) -> Os {
        Os::Windows
    }

    fn unit_path(&self) -> PathBuf {
        self.state_dir.join(DEFINITION_FILE)
    }

    fn render(&self, spec: &UnitSpec) -> Result<String, String> {
        render_task(spec, &self.user_sid()?, &self.work_dir)
    }

    fn pinned_config(&self) -> Result<PathBuf, String> {
        let path = self.unit_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        task_pinned_config(&text).ok_or_else(|| {
            format!(
                "{} names no --config, so the settings the service runs with are unknown; \
                 re-run agent-hub service install to rewrite it, or remove it",
                path.display()
            )
        })
    }

    fn prepare(&self, want: Want) -> Result<Prepared, Unavailable> {
        self.check()?;
        match want {
            Want::Boot => {
                if !self.elevated {
                    return Err(Unavailable::failed(
                        "--boot needs an elevated shell; re-run there, or use the default logon start",
                    ));
                }
                Ok(Prepared {
                    starts_at: StartsAt::Boot,
                    row: "boot (runs without a logon)".to_string(),
                    advice: None,
                })
            }
            Want::Default | Want::Login => Ok(Prepared {
                starts_at: StartsAt::Login,
                row: "logon".to_string(),
                advice: None,
            }),
        }
    }

    fn check(&self) -> Result<(), Unavailable> {
        match self.schtasks(&["/Query", "/FO", "CSV", "/NH"]) {
            Ok(outcome) if outcome.ok() => Ok(()),
            Ok(outcome) => Err(Unavailable::none(format!(
                "the Task Scheduler does not answer ({}), so the service cannot be stopped or \
                 removed safely",
                outcome.complaint()
            ))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Self::missing_schtasks()),
            Err(err) => Err(Unavailable::none(format!("could not run schtasks: {err}"))),
        }
    }

    fn register(
        &self,
        _spec: &UnitSpec,
        _changed: bool,
        _start: bool,
    ) -> Result<Option<String>, Unavailable> {
        // The definition was just written. The scheduler wants the XML as UTF-16
        // with a byte order mark, so the file it reads is a copy in that form.
        let definition = self.unit_path();
        let text = std::fs::read_to_string(&definition).map_err(|err| {
            Unavailable::failed(format!("could not read {}: {err}", definition.display()))
        })?;
        let temp = self
            .state_dir
            .join(format!(".{DEFINITION_FILE}.{}.utf16", std::process::id()));
        std::fs::write(&temp, utf16_with_bom(&text)).map_err(|err| {
            Unavailable::failed(format!("could not write {}: {err}", temp.display()))
        })?;
        let created = self.must_or_unreachable(&[
            "/Create",
            "/TN",
            TASK_NAME,
            "/XML",
            temp.to_str().unwrap_or_default(),
            "/F",
        ]);
        let _ = std::fs::remove_file(&temp);
        created.map(|_| None)
    }

    fn start(&self) -> Result<(), Unavailable> {
        // A supervisor asked to stop (a restart) may still be on its way out,
        // and with IgnoreNew the scheduler would drop a `/Run` that comes while
        // the old instance runs. So wait for it to be gone first.
        let began = Instant::now();
        let mut old = None;
        loop {
            let live = self.supervisor();
            if live.is_some() {
                old.clone_from(&live);
            }
            // A scheduler that cannot be asked is not a task that is stopped:
            // `/Run` is only sent on an answer.
            let running = match live {
                Some(_) => false,
                None => self.task_running()?,
            };
            match before_run(live.as_ref(), running) {
                BeforeRun::AlreadyRunning => return Ok(()),
                BeforeRun::Run => break,
                BeforeRun::WaitForExit => {}
            }
            if began.elapsed() >= self.waits.exit {
                return Err(Unavailable::failed(format!(
                    "the task is still running from before (its supervisor has not exited \
                     within {}s), so it was not started again; try again",
                    self.waits.exit.as_secs()
                )));
            }
            std::thread::sleep(self.waits.poll);
        }
        self.must_or_unreachable(&["/Run", "/TN", TASK_NAME])?;
        // `/Run` returns before the task has started. Wait until a supervisor
        // that is not the old one shows itself, so a state read right after
        // does not take a task that is about to start for one that stopped.
        let began = Instant::now();
        loop {
            if let Some(now) = self.supervisor()
                && is_new(old.as_ref(), &now)
            {
                return Ok(());
            }
            if began.elapsed() >= self.waits.appear {
                return Err(Unavailable::failed(format!(
                    "the task was started but no supervisor appeared within {}s; see agent-hub logs",
                    self.waits.appear.as_secs()
                )));
            }
            std::thread::sleep(self.waits.poll);
        }
    }

    fn stop(&self, force: bool) -> Result<(), Unavailable> {
        // The supervisor relays the request to the hub, which drains and folds
        // the store. `schtasks /End` is TerminateProcess and skips all of that,
        // so it is for `--force` only: a graceful stop that cannot reach the
        // supervisor says so and ends nothing.
        if force {
            return match self.schtasks(&["/End", "/TN", TASK_NAME]) {
                // A task with nothing running answers /End with an error; that
                // is the state asked for.
                Ok(_) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    Err(Self::missing_schtasks())
                }
                Err(err) => Err(Unavailable::none(format!("could not run schtasks: {err}"))),
            };
        }
        let began = Instant::now();
        loop {
            match self.supervisor() {
                Some(record) => match (self.signal)(record.pid) {
                    Ok(true) => return Ok(()),
                    // The record is there but its event is not reachable yet.
                    Ok(false) => {}
                    Err(err) => {
                        return Err(Unavailable::failed(format!(
                            "could not signal the supervisor (pid {}): {err}; nothing was \
                             stopped. agent-hub stop --force ends the task without a checkpoint",
                            record.pid
                        )));
                    }
                },
                // Nothing runs, so there is nothing to stop.
                None => {
                    if !self.task_running()? {
                        return Ok(());
                    }
                    // The task is starting and its supervisor has not
                    // published its stop event yet.
                }
            }
            if began.elapsed() >= self.waits.signal {
                return Err(Unavailable::failed(format!(
                    "the task's supervisor could not be asked to stop within {}s, so nothing \
                     was stopped; agent-hub stop --force ends the task without a checkpoint",
                    self.waits.signal.as_secs()
                )));
            }
            std::thread::sleep(self.waits.poll);
        }
    }

    fn state(&self) -> Result<UnitState, String> {
        let queried = self
            .schtasks(&["/Query", "/TN", TASK_NAME, "/FO", "CSV", "/NH", "/V"])
            .map_err(|err| format!("could not run schtasks: {err}"))?;
        if !queried.ok() {
            // Not there, the scheduler is down, or the task cannot be read:
            // only the first is a state.
            if self.task_xml().map_err(|err| err.message)?.is_some() {
                return Err(format!(
                    "the task {TASK_URI} is registered but schtasks could not query it: {}",
                    queried.complaint()
                ));
            }
            return Ok(UnitState {
                active: Active::Inactive,
                enabled: false,
                starts_at: "manual",
                detail: Some(
                    "the task is not registered; re-run agent-hub service install".to_string(),
                ),
            });
        }
        let last_result = last_result(&queried.stdout);
        let xml = self
            .schtasks(&["/Query", "/TN", TASK_NAME, "/XML"])
            .ok()
            .filter(Outcome::ok)
            .map(|outcome| outcome.stdout)
            .unwrap_or_default();
        let enabled = task_enabled(&xml);
        let starts_at = if !enabled {
            "manual"
        } else if xml.contains("<BootTrigger") {
            "boot"
        } else {
            "login"
        };
        let running = self.supervisor().is_some() || last_result == Some(LAST_RESULT_RUNNING);
        let (active, detail) = if running {
            (Active::Running, None)
        } else {
            match last_result {
                None => {
                    return Err(format!(
                        "schtasks printed no readable Last Result for the task {TASK_URI}, so \
                         whether it runs is unknown"
                    ));
                }
                Some(0 | LAST_RESULT_NOT_RUN | LAST_RESULT_TERMINATED) => (Active::Inactive, None),
                Some(EXIT_CONFIG) => (Active::Failed, Some("configuration error".to_string())),
                Some(code) => (
                    Active::Failed,
                    Some(format!("the last run ended with {code}")),
                ),
            }
        };
        Ok(UnitState {
            active,
            enabled,
            starts_at,
            detail,
        })
    }

    fn job(&self) -> Result<Job, Unavailable> {
        match self.task_xml()? {
            None => Ok(Job::Absent),
            // A task that cannot be shown to be this install's (its config
            // unreadable, or it runs some other binary) is refused as a stray
            // one: nothing is removed on its account.
            Some(xml) => self.verified_config(&xml).map(|config| Job::Loaded {
                config: Some(config),
            }),
        }
    }

    fn job_name(&self) -> String {
        TASK_URI.to_string()
    }

    fn unload_hint(&self) -> String {
        format!("schtasks /Delete /TN \"{TASK_NAME}\" /F")
    }

    fn main_pid(&self) -> Result<Option<u32>, String> {
        Ok(self
            .supervisor()
            .map(|record| record.child)
            .filter(|child| *child != 0))
    }

    fn remove(&self) -> Result<(), Unavailable> {
        self.check()?;
        if let Some(xml) = self.task_xml()? {
            self.verified_config(&xml)?;
            // Uninstall stopped the hub first, so a supervisor still going is
            // on its way out. Whatever does not finish is not ended here: that
            // is TerminateProcess, and it is for `stop --force` alone.
            self.wait_idle()?;
            self.must_or_unreachable(&["/Delete", "/TN", TASK_NAME, "/F"])?;
        }
        match std::fs::remove_file(self.unit_path()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(Unavailable::failed(format!(
                    "the task was deleted but {} could not be removed: {err}",
                    self.unit_path().display()
                )));
            }
        }
        SupervisorRecord::remove(&self.state_dir);
        // No supervisor runs once the task is gone, so its lock file goes too
        // and a purge finds the state directory empty.
        let _ = std::fs::remove_file(self.state_dir.join(SUPERVISOR_LOCK));
        Ok(())
    }

    fn logs(&self, _options: &LogsOptions) -> Option<ExitCode> {
        // The hub writes its own log file; there is no journal to ask.
        None
    }

    fn failure_output(&self) -> Vec<String> {
        let Some(settings) = self.settings() else {
            return Vec::new();
        };
        let startup = crate::daemon::process::startup_file(&settings);
        crate::daemon::logfile::last_lines(&startup, 20).unwrap_or_default()
    }

    fn log_hint(&self) -> String {
        match self.settings() {
            Some(settings) => format!("{} (agent-hub logs)", settings.log_file.display()),
            None => "agent-hub logs".to_string(),
        }
    }
}

/// Set the supervisor's stop event. `Ok(false)` when it has none (it has not
/// created it yet, or this is not Windows).
fn signal_supervisor(pid: u32) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        match agent_hub::app::stop_event::StopEvent::open(pid)? {
            Some(event) => event.signal().map(|()| true),
            None => Ok(false),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Ok(false)
    }
}

// ---- the task's XML ----

/// A path the task names, as text it can carry.
///
/// The scheduler expands `%NAME%` in a command line, a closing quote cannot be
/// escaped inside one, and a control character has no business in either.
fn task_path<'a>(path: &'a Path, what: &str) -> Result<&'a str, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("the {what} path is not valid UTF-8: {}", path.display()))?;
    if text.chars().any(char::is_control) {
        return Err(format!(
            "the {what} path holds a control character and cannot go in a task"
        ));
    }
    if text.contains('%') || text.contains('"') {
        return Err(format!(
            "the {what} path {text} holds a '%' or a '\"', which a Task Scheduler command line \
             cannot carry safely; move the file or use another account"
        ));
    }
    if text.ends_with('\\') {
        return Err(format!("the {what} path {text} ends in a backslash"));
    }
    Ok(text)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The Task Scheduler definition for the hub.
///
/// A logon task runs as the user's interactive token with least privilege. A
/// boot task (`--boot`, from an elevated shell) runs without a logon, as S4U.
pub fn render_task(spec: &UnitSpec, sid: &str, work_dir: &Path) -> Result<String, String> {
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("{sid:?} is not an account SID"));
    }
    let binary = xml_escape(task_path(&spec.binary, "binary")?);
    let config = xml_escape(task_path(&spec.config, "config")?);
    let work = xml_escape(task_path(work_dir, "working directory")?);
    let (trigger, logon_type) = match spec.starts_at {
        StartsAt::Login => (
            format!("<LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger>"),
            "InteractiveToken",
        ),
        StartsAt::Boot => (
            "<BootTrigger><Enabled>true</Enabled></BootTrigger>".to_string(),
            "S4U",
        ),
    };
    Ok(format!(
        "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">
  <RegistrationInfo>
    <URI>{TASK_URI}</URI>
    <Description>Agent Hub. Written by agent-hub service install.</Description>
  </RegistrationInfo>
  <Triggers>
    {trigger}
  </Triggers>
  <Principals>
    <Principal id=\"Author\">
      <UserId>{sid}</UserId>
      <LogonType>{logon_type}</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <AllowHardTerminate>true</AllowHardTerminate>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context=\"Author\">
    <Exec>
      <Command>%SystemRoot%\\System32\\conhost.exe</Command>
      <Arguments>--headless \"{binary}\" service run --config \"{config}\"</Arguments>
      <WorkingDirectory>{work}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"
    ))
}

/// The config file a task [`render_task`] wrote names after `--config`, read
/// back from its text (or from the scheduler's export of it).
pub fn task_pinned_config(xml: &str) -> Option<PathBuf> {
    let arguments = xml
        .split_once("<Arguments>")?
        .1
        .split_once("</Arguments>")?
        .0;
    let arguments = xml_unescape(arguments);
    let rest = arguments.split_once("--config \"")?.1;
    let value = rest.split_once('"')?.0;
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// The program the task's first action runs: the `Command`, or for the
/// `conhost.exe --headless <program> ...` the task is written with, the
/// program it hosts. `%VAR%` is left as written.
pub fn task_executable(xml: &str) -> Option<String> {
    let command = xml.split_once("<Command>")?.1.split_once("</Command>")?.0;
    let command = xml_unescape(command).trim().trim_matches('"').to_string();
    let host = command
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("conhost.exe"));
    if !host {
        return (!command.is_empty()).then_some(command);
    }
    let arguments = xml
        .split_once("<Arguments>")?
        .1
        .split_once("</Arguments>")?
        .0;
    let arguments = xml_unescape(arguments);
    let rest = arguments.trim_start();
    let rest = rest.strip_prefix("--headless").unwrap_or(rest).trim_start();
    let program = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?.0,
        None => rest.split_whitespace().next()?,
    };
    (!program.is_empty()).then(|| program.to_string())
}

/// `text` with each `%NAME%` that `env` knows replaced, as the scheduler
/// expands a command. A name it does not know is kept as written.
fn expand_vars(text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('%') {
            Some(close) if close > 0 => {
                let name = &after[..close];
                match env(name).or_else(|| env(&name.to_uppercase())) {
                    Some(value) => out.push_str(&value),
                    None => out.push_str(&rest[open..open + close + 2]),
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Windows compares names without case, beyond ASCII too.
fn same_name(a: &str, b: &str) -> bool {
    a.to_uppercase() == b.to_uppercase()
}

/// Windows compares paths without case (beyond ASCII too), with either slash,
/// and without a trailing separator.
fn same_path(a: &str, b: &str) -> bool {
    let norm = |text: &str| {
        text.replace('/', "\\")
            .trim_end_matches('\\')
            .to_uppercase()
    };
    norm(a) == norm(b)
}

/// Whether the program a task runs is the installed binary, once `%VAR%` in it
/// is expanded.
fn same_executable(runs: &str, installed: &str, env: &dyn Fn(&str) -> Option<String>) -> bool {
    same_path(&expand_vars(runs, env), &expand_vars(installed, env))
}

/// Whether the task's settings leave it enabled. A task with no `Enabled`
/// setting is enabled.
fn task_enabled(xml: &str) -> bool {
    let Some(settings) = xml.split_once("<Settings>").map(|(_, rest)| rest) else {
        return true;
    };
    let settings = settings
        .split_once("</Settings>")
        .map_or(settings, |(a, _)| a);
    !settings.contains("<Enabled>false</Enabled>")
}

/// The XML as the scheduler reads it: UTF-16 little-endian with a byte order
/// mark, and a declaration that says so.
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let text = text.replacen("encoding=\"UTF-8\"", "encoding=\"UTF-16\"", 1);
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

// ---- reading what `schtasks` and `whoami` print ----

/// Split one CSV line: fields in double quotes, a doubled quote inside one for
/// a quote.
pub fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// The SID in `whoami /user /fo csv /nh`: `"DOMAIN\name","S-1-5-21-..."`.
fn parse_whoami_sid(output: &str) -> Option<String> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let sid = csv_fields(line.trim()).into_iter().nth(1)?;
    let sid = sid.trim();
    (sid.starts_with("S-1-") && sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| sid.to_string())
}

/// The numeric "Last Result" of `schtasks /Query /FO CSV /NH /V`, the seventh
/// column. The columns are in a fixed order whatever the language; a few are
/// words, but this one is a number.
fn last_result(output: &str) -> Option<i64> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let field = csv_fields(line.trim()).into_iter().nth(6)?;
    let field = field.trim();
    // The scheduler prints 0, 267009, or a negative HRESULT as a decimal.
    match field.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => field.parse().ok(),
    }
}

// ---- what `start` waits for ----

/// What `start` does about the supervisor it finds before it runs the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeforeRun {
    /// No supervisor and the task is not running: run it.
    Run,
    /// A supervisor runs and was not asked to stop: it keeps the hub up, and
    /// a `/Run` would be dropped anyway.
    AlreadyRunning,
    /// A supervisor is on its way out, or the task still runs without one.
    WaitForExit,
}

fn before_run(live: Option<&SupervisorRecord>, task_running: bool) -> BeforeRun {
    match live {
        Some(record) if !record.stopping => BeforeRun::AlreadyRunning,
        Some(_) => BeforeRun::WaitForExit,
        None if task_running => BeforeRun::WaitForExit,
        None => BeforeRun::Run,
    }
}

/// Whether `now` is a different supervisor from `old`: another pid, or the
/// same pid with another start time.
fn is_new(old: Option<&SupervisorRecord>, now: &SupervisorRecord) -> bool {
    old.is_none_or(|old| (old.pid, old.start) != (now.pid, now.start))
}

// ---- the task's export ----

#[derive(Debug, PartialEq, Eq)]
enum Export {
    Task(String),
    Absent,
    /// The task could not be read, for this reason.
    Unknown(String),
}

/// Read what [`EXPORT_TASK`] printed.
fn read_export(outcome: &Outcome) -> Export {
    match outcome.code {
        Some(0) => match from_hex(outcome.stdout.trim()).map(String::from_utf8) {
            Some(Ok(xml)) if !xml.is_empty() => Export::Task(xml),
            _ => Export::Unknown("the export was not readable".to_string()),
        },
        Some(3) => Export::Absent,
        _ => Export::Unknown(outcome.complaint()),
    }
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

// ---- elevation, the user PATH and the running binary ----

/// Whether this process holds an elevated token.
fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::Security::{
            GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: the out-pointers are valid for the calls; the token is closed once.
        unsafe {
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut size = 0u32;
            let read = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut size,
            );
            CloseHandle(token);
            read != 0 && elevation.TokenIsElevated != 0
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The file in the state directory in which the install script records the
/// user PATH entry it added, and only when it added one.
#[cfg_attr(not(windows), allow(dead_code))]
const PATH_MARKER: &str = "path-added";

/// The PATH entry the marker's text records, when it records `dir`: the
/// directory of the binary being removed. A marker that names another
/// directory, or none, is not this install's.
#[cfg_attr(not(windows), allow(dead_code))]
fn marked_entry(marker: &str, dir: &str) -> Option<String> {
    let entry = marker.trim_matches(['\r', '\n']);
    (!entry.is_empty() && same_path(entry, dir)).then(|| entry.to_string())
}

/// The PATH entry the install script recorded for `dir` and the marker file
/// that records it, or `None` when there is no marker or it names another
/// directory.
#[cfg_attr(not(windows), allow(dead_code))]
fn owned_path_entry(state: Option<&Path>, dir: &str) -> Option<(String, PathBuf)> {
    let marker = state?.join(PATH_MARKER);
    let text = std::fs::read_to_string(&marker).ok()?;
    marked_entry(&text, dir).map(|entry| (entry, marker))
}

/// `value` (a `;`-separated PATH) without `entry`, or `None` when it has none.
/// Every other entry, an empty one included, is kept exactly as written.
#[cfg_attr(not(windows), allow(dead_code))]
fn without_path_entry(value: &str, entry: &str) -> Option<String> {
    let mut found = false;
    let kept: Vec<&str> = value
        .split(';')
        .filter(|item| {
            let ours = !item.is_empty() && same_name(item, entry);
            found |= ours;
            !ours
        })
        .collect();
    found.then(|| kept.join(";"))
}

/// Remove `entry`, which the install script recorded as its own, from the
/// user's PATH (`HKCU\Environment`), keeping the value's type so a `%VAR%`
/// entry the user has stays expandable. `Ok(false)` when it was not there.
#[cfg(windows)]
fn remove_from_user_path(entry: &str) -> Result<bool, String> {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey = wide("Environment");
    let name = wide("Path");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: valid NUL-terminated names and out-pointer; the key is closed below.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            &mut key,
        )
    };
    if opened != 0 {
        return Err(format!("could not open HKCU\\Environment (error {opened})"));
    }
    let result = (|| {
        let mut kind = 0u32;
        let mut size = 0u32;
        // SAFETY: a size query: no buffer, valid out-pointers.
        let queried = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if queried == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        if queried != 0 {
            return Err(format!("could not read the user PATH (error {queried})"));
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err("the user PATH is not a string value, so it was left alone".to_string());
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: the buffer holds `bytes` bytes.
        let read = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if read != 0 {
            return Err(format!("could not read the user PATH (error {read})"));
        }
        buffer.truncate(bytes as usize / 2);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        let current = String::from_utf16(&buffer)
            .map_err(|_| "the user PATH is not valid text, so it was left alone".to_string())?;
        let Some(updated) = without_path_entry(&current, entry) else {
            return Ok(false);
        };
        let data = wide(&updated);
        // SAFETY: `data` is a NUL-terminated UTF-16 string of the byte length given.
        let written = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if written != 0 {
            return Err(format!("could not write the user PATH (error {written})"));
        }
        Ok(true)
    })();
    // SAFETY: the key was opened above and is closed once.
    unsafe { RegCloseKey(key) };
    if result == Ok(true) {
        // Tell running programs (Explorer, so new terminals) the environment
        // changed. Best effort: the registry value is already right.
        let environment = wide("Environment");
        let mut out = 0usize;
        // SAFETY: a valid NUL-terminated string and out-pointer.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5000,
                &mut out,
            );
        }
    }
    result
}

/// The installed binary's file name.
#[cfg_attr(not(windows), allow(dead_code))]
const BINARY_NAME: &str = "agent-hub.exe";
/// The previous binary the install script keeps in the state directory.
#[cfg_attr(not(windows), allow(dead_code))]
const PREVIOUS_NAME: &str = "agent-hub.prev.exe";
/// The only directory name the helper may remove (when empty) and the only
/// one whose PATH entry is taken out: a directory of the hub's own.
#[cfg_attr(not(windows), allow(dead_code))]
const OWN_DIR_NAME: &str = "agent-hub";
/// How many times the helper tries, about a second apart, while this process
/// still holds its binary.
#[cfg_attr(not(windows), allow(dead_code))]
const DELETE_TRIES: u32 = 60;

/// What an upgrade left beside the binary when it could not delete it:
/// `agent-hub.exe.<8 hex digits>.old`.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_rollback_name(name: &str) -> bool {
    let prefix = "agent-hub.exe.";
    let suffix = ".old";
    name.len() == prefix.len() + 8 + suffix.len()
        && name
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        && name
            .get(name.len() - suffix.len()..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
        && name[prefix.len()..prefix.len() + 8]
            .chars()
            .all(|c| c.is_ascii_hexdigit())
}

/// Text `cmd.exe` takes literally inside double quotes, even with delayed
/// expansion on: none of `%`, `!`, `"`, `^`, `&`, `|`, `<`, `>`, and no control
/// character.
#[cfg_attr(not(windows), allow(dead_code))]
fn cmd_literal(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() || matches!(c, '%' | '!' | '"' | '^' | '&' | '|' | '<' | '>'))
}

/// What a path is on disk, without following a link.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Missing,
    /// A symlink, junction or other reparse point, or anything else.
    Other,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn kind_of(path: &Path) -> Kind {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Kind::Missing;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Kind::Other;
        }
    }
    let kind = meta.file_type();
    if kind.is_symlink() {
        Kind::Other
    } else if kind.is_file() {
        Kind::File
    } else if kind.is_dir() {
        Kind::Dir
    } else {
        Kind::Other
    }
}

/// What the deleting helper gets: files it may delete and the directory it may
/// remove once that leaves it empty, each checked and safe to quote.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
struct Retire {
    files: Vec<String>,
    dir: Option<String>,
}

/// The files to delete after this process exits, each checked: an absolute
/// path, a regular file (no directory, link or reparse point) and one of the
/// exact names the install writes. Something else of a rollback name is left
/// alone. `Err` says why the helper cannot be trusted with the binary at all.
#[cfg_attr(not(windows), allow(dead_code))]
fn retire_plan(binary: &Path, previous: Option<&Path>) -> Result<Retire, String> {
    if !binary.is_absolute() {
        return Err(format!("{} is not an absolute path", binary.display()));
    }
    let name_ok = binary
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(BINARY_NAME));
    if !name_ok {
        return Err(format!("{} is not named {BINARY_NAME}", binary.display()));
    }
    if kind_of(binary) != Kind::File {
        return Err(format!("{} is not a regular file", binary.display()));
    }
    let dir = binary
        .parent()
        .ok_or_else(|| format!("{} has no directory", binary.display()))?;
    let mut files = vec![binary.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut rollbacks: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_rollback_name)
                    && kind_of(path) == Kind::File
            })
            .collect();
        rollbacks.sort();
        files.extend(rollbacks);
    }
    if let Some(previous) = previous
        && previous.is_absolute()
        && previous
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(PREVIOUS_NAME))
        && kind_of(previous) == Kind::File
    {
        files.push(previous.to_path_buf());
    }
    let mut texts = Vec::new();
    for file in &files {
        let text = file
            .to_str()
            .filter(|text| cmd_literal(text))
            .ok_or_else(|| {
                format!(
                    "{} holds a character cmd.exe would not take literally",
                    file.display()
                )
            })?;
        texts.push(text.to_string());
    }
    let dir = (dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(OWN_DIR_NAME))
        && kind_of(dir) == Kind::Dir)
        .then(|| dir.to_str().filter(|text| cmd_literal(text)))
        .flatten()
        .map(str::to_string);
    Ok(Retire { files: texts, dir })
}

/// The `cmd.exe` command line that deletes the files once this process has
/// exited. Delayed expansion is off and every path is literal. Each round
/// deletes what is still there, only if it is not a directory (`X\*` exists
/// for a directory), then removes the directory if that left it empty; while
/// the binary is still there (this process has not exited), it waits about a
/// second (a ping, since `timeout` needs a console) and tries again, up to
/// [`DELETE_TRIES`] rounds.
#[cfg_attr(not(windows), allow(dead_code))]
fn delete_command(plan: &Retire) -> String {
    let mut round = Vec::new();
    for file in &plan.files {
        round.push(format!(
            "(if exist \"{file}\" if not exist \"{file}\\*\" del /f /q \"{file}\" >nul 2>&1)"
        ));
    }
    if let Some(dir) = &plan.dir {
        // Without /s this removes only an empty directory.
        round.push(format!("(if exist \"{dir}\\*\" rmdir \"{dir}\" >nul 2>&1)"));
    }
    if let Some(binary) = plan.files.first() {
        round.push(format!(
            "(if exist \"{binary}\" ping -n 2 127.0.0.1 >nul 2>&1)"
        ));
    }
    format!(
        "/d /v:off /q /c for /l %i in (1,1,{DELETE_TRIES}) do ({})",
        round.join(" & ")
    )
}

/// Delete the running binary, the previous one kept for a rollback and what an
/// upgrade left beside it, and take the installer's entry off the user PATH.
///
/// A running `.exe` cannot delete itself, so a detached `cmd.exe` does it once
/// this process has exited. That is best effort: a file it cannot delete stays
/// for the next install to sweep. A path the helper cannot be trusted with is
/// refused, with the command to run by hand.
pub fn retire_running_binary(binary: &Path) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let env: &dyn Fn(&str) -> Option<String> = &super::process_env;
        let state = state_dir(env);
        let previous = state.as_ref().map(|state| state.join(PREVIOUS_NAME));
        let plan = retire_plan(binary, previous.as_deref()).map_err(|why| {
            format!(
                "{} was not deleted ({why}); delete it by hand once this command has exited: \
                 del /f /q \"{}\"",
                binary.display(),
                binary.display()
            )
        })?;
        let dir = binary.parent().unwrap_or(binary);
        // Only an entry the install script recorded as its own: a directory
        // that was on the PATH before the install stays there.
        let path_note = match dir
            .to_str()
            .and_then(|text| owned_path_entry(state.as_deref(), text))
        {
            Some((entry, marker)) => {
                let outcome = remove_from_user_path(&entry);
                if outcome.is_ok() {
                    let _ = std::fs::remove_file(marker);
                }
                outcome
            }
            None => Ok(false),
        };
        std::process::Command::new("cmd.exe")
            .raw_arg(delete_command(&plan))
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| {
                format!(
                    "could not schedule the removal of {}: {err}",
                    binary.display()
                )
            })?;
        let mut said = format!(
            "{} (will be deleted after this process exits)",
            binary.display()
        );
        match path_note {
            Ok(true) => said.push_str(&format!("; {} removed from the user PATH", dir.display())),
            Ok(false) => {}
            Err(err) => {
                return Err(format!("{said}, but the user PATH was not updated: {err}"));
            }
        }
        Ok(Some(said))
    }
    #[cfg(not(windows))]
    {
        Err(format!(
            "{} cannot be retired this way off Windows",
            binary.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::runner::recording::Recorder;

    const SID: &str = "S-1-5-21-111-222-333-1001";

    fn spec(starts_at: StartsAt) -> UnitSpec {
        UnitSpec {
            binary: PathBuf::from(
                "C:\\Users\\u\\AppData\\Local\\Programs\\agent-hub\\agent-hub.exe",
            ),
            config: PathBuf::from("C:\\Users\\u\\AppData\\Roaming\\agent-hub\\config.toml"),
            home: PathBuf::from("C:\\Users\\u"),
            starts_at,
        }
    }

    fn work() -> PathBuf {
        PathBuf::from("C:\\Users\\u\\AppData\\Local\\agent-hub")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            PathBuf::from("target/tmp").join(format!("schtasks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// What [`EXPORT_TASK`] prints for this XML.
    fn exported(xml: &str) -> String {
        let hex: String = xml.bytes().map(|byte| format!("{byte:02X}")).collect();
        format!("{hex}\r\n")
    }

    fn quick() -> Waits {
        Waits {
            exit: Duration::from_millis(300),
            appear: Duration::from_millis(300),
            signal: Duration::from_millis(300),
            poll: Duration::from_millis(10),
        }
    }

    fn record(pid: u32, start: u64, stopping: bool) -> SupervisorRecord {
        SupervisorRecord {
            pid,
            start,
            child: 0,
            stopping,
        }
    }

    const INSTALLED: &str = "C:\\Users\\u\\AppData\\Local\\Programs\\agent-hub\\agent-hub.exe";

    fn manager(recorder: &Recorder, state: &Path, elevated: bool) -> Schtasks {
        Schtasks::new(
            Box::new(recorder.clone()),
            state.to_path_buf(),
            work(),
            elevated,
        )
        .with_binary(Some(PathBuf::from(INSTALLED)))
    }

    /// What `schtasks /Query ... /V` prints for a task with this last result.
    fn query_row(result: i64) -> String {
        format!("\"PC\",\"\\Agent Hub\",\"N/A\",\"Ready\",\"x\",\"-\",\"{result}\",\"u\"\r\n")
    }

    /// A scheduler whose task is not running.
    fn idle(recorder: &Recorder) {
        recorder.exit(
            "schtasks /Query /TN Agent Hub /FO CSV",
            0,
            &query_row(LAST_RESULT_NOT_RUN),
            "",
        );
    }

    /// The scheduler's export of the task `service install` registers.
    fn ours() -> String {
        exported(&render_task(&spec(StartsAt::Login), SID, &work()).expect("render"))
    }

    #[test]
    fn the_logon_task_names_the_user_the_headless_host_and_the_supervisor() {
        let xml = render_task(&spec(StartsAt::Login), SID, &work()).expect("render");
        assert!(xml.contains(&format!(
            "<LogonTrigger><Enabled>true</Enabled><UserId>{SID}</UserId></LogonTrigger>"
        )));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<URI>\\Agent Hub</URI>"));
        assert!(xml.contains("<Command>%SystemRoot%\\System32\\conhost.exe</Command>"));
        assert!(xml.contains(
            "<Arguments>--headless \"C:\\Users\\u\\AppData\\Local\\Programs\\agent-hub\\agent-hub.exe\" \
             service run --config \"C:\\Users\\u\\AppData\\Roaming\\agent-hub\\config.toml\"</Arguments>"
        ));
        assert!(xml.contains(
            "<WorkingDirectory>C:\\Users\\u\\AppData\\Local\\agent-hub</WorkingDirectory>"
        ));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(!xml.contains("BootTrigger"));
    }

    #[test]
    fn the_boot_task_has_a_boot_trigger_and_runs_without_a_logon() {
        let xml = render_task(&spec(StartsAt::Boot), SID, &work()).expect("render");
        assert!(xml.contains("<BootTrigger><Enabled>true</Enabled></BootTrigger>"));
        assert!(xml.contains("<LogonType>S4U</LogonType>"));
        assert!(!xml.contains("LogonTrigger"));
    }

    #[test]
    fn the_rendered_task_is_well_formed_and_carries_its_config_back() {
        let mut odd = spec(StartsAt::Login);
        odd.config = PathBuf::from("C:\\Users\\a&b <c>\\config.toml");
        let xml = render_task(&odd, SID, &work()).expect("render");
        assert!(xml.contains("a&amp;b &lt;c&gt;"), "{xml}");
        assert_eq!(
            task_pinned_config(&xml),
            Some(PathBuf::from("C:\\Users\\a&b <c>\\config.toml"))
        );
        // The scheduler's own export puts quotes in as plain text or as &quot;.
        let exported = "<Arguments>--headless &quot;C:\\x\\agent-hub.exe&quot; service run --config &quot;D:\\c.toml&quot;</Arguments>";
        assert_eq!(
            task_pinned_config(exported),
            Some(PathBuf::from("D:\\c.toml"))
        );
        assert_eq!(task_pinned_config("<Task/>"), None);
    }

    #[test]
    fn a_path_the_scheduler_would_expand_or_cut_is_refused() {
        for config in [
            "C:\\100%\\c.toml",
            "C:\\a\"b\\c.toml",
            "C:\\dir\\",
            "C:\\a\u{7}b\\c.toml",
        ] {
            let mut bad = spec(StartsAt::Login);
            bad.config = PathBuf::from(config);
            assert!(
                render_task(&bad, SID, &work()).is_err(),
                "{config:?} must be refused"
            );
        }
        assert!(render_task(&spec(StartsAt::Login), "S-1; drop", &work()).is_err());
    }

    #[test]
    fn the_scheduler_gets_utf16_with_a_byte_order_mark_and_a_matching_declaration() {
        let bytes = utf16_with_bom("<?xml version=\"1.0\" encoding=\"UTF-8\"?><a>\u{e9}</a>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16(&units).expect("utf16");
        assert!(text.contains("encoding=\"UTF-16\""), "{text}");
        assert!(text.contains('\u{e9}'));
    }

    #[test]
    fn csv_fields_keep_commas_and_doubled_quotes() {
        assert_eq!(
            csv_fields("\"a,b\",\"say \"\"hi\"\"\",c"),
            vec!["a,b", "say \"hi\"", "c"]
        );
        assert_eq!(
            parse_whoami_sid("\"HOST\\user\",\"S-1-5-21-1-2-3-1001\"\r\n").as_deref(),
            Some("S-1-5-21-1-2-3-1001")
        );
        assert_eq!(parse_whoami_sid("garbage"), None);
        assert_eq!(parse_whoami_sid("\"HOST\\user\",\"not-a-sid\""), None);
    }

    #[test]
    fn the_last_result_is_the_seventh_column_whatever_the_words() {
        let line = |result: &str| {
            format!(
                "\"PC\",\"\\Agent Hub\",\"N/A\",\"Wird ausgef\u{fc}hrt\",\"Interactive only\",\"11/23/2026 9:00:00 AM\",\"{result}\",\"u\",\"x\""
            )
        };
        assert_eq!(last_result(&line("0")), Some(0));
        assert_eq!(last_result(&line("267009")), Some(LAST_RESULT_RUNNING));
        assert_eq!(last_result(&line("78")), Some(78));
        assert_eq!(last_result(&line("-2147020576")), Some(-2147020576));
        assert_eq!(last_result("garbage"), None);
    }

    #[test]
    fn a_disabled_task_is_read_from_its_settings_not_its_trigger() {
        let on = "<Task><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Settings><Priority>5</Priority></Settings></Task>";
        let off = "<Task><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Settings><Enabled>false</Enabled></Settings></Task>";
        assert!(task_enabled(on));
        assert!(!task_enabled(off));
        assert!(task_enabled(""));
    }

    #[test]
    fn boot_without_elevation_is_refused_and_never_elevates() {
        let state = scratch("boot");
        let recorder = Recorder::default();
        let refused = manager(&recorder, &state, false)
            .prepare(Want::Boot)
            .expect_err("needs elevation");
        assert_eq!(refused.code, 1);
        assert!(
            refused.message.contains("--boot needs an elevated shell"),
            "{}",
            refused.message
        );
        assert!(
            !recorder.calls().iter().any(|call| call.contains("/Create")),
            "{:?}",
            recorder.calls()
        );
        let elevated = manager(&recorder, &state, true)
            .prepare(Want::Boot)
            .expect("elevated");
        assert_eq!(elevated.starts_at, StartsAt::Boot);
        let login = manager(&recorder, &state, false)
            .prepare(Want::Default)
            .expect("default");
        assert_eq!(login.starts_at, StartsAt::Login);
    }

    #[test]
    fn no_schtasks_is_69_with_the_way_out() {
        let state = scratch("missing");
        let recorder = Recorder::default();
        recorder.missing("schtasks");
        let err = manager(&recorder, &state, false)
            .prepare(Want::Default)
            .expect_err("no scheduler");
        assert_eq!(err.code, 69);
        assert!(err.message.contains("agent-hub start"), "{}", err.message);
    }

    #[test]
    fn register_hands_the_scheduler_a_utf16_copy_and_removes_it() {
        let state = scratch("register");
        let recorder = Recorder::default();
        recorder.exit(
            "whoami /user /fo csv /nh",
            0,
            "\"PC\\u\",\"S-1-5-21-9-9-9-1001\"\r\n",
            "",
        );
        let manager = manager(&recorder, &state, false);
        let xml = manager.render(&spec(StartsAt::Login)).expect("render");
        assert!(xml.contains("S-1-5-21-9-9-9-1001"));
        std::fs::write(manager.unit_path(), &xml).expect("definition");
        manager
            .register(&spec(StartsAt::Login), true, true)
            .expect("register");
        let create = recorder
            .calls()
            .into_iter()
            .find(|call| call.starts_with("schtasks /Create"))
            .expect("created");
        assert!(create.contains("/TN Agent Hub /XML "), "{create}");
        assert!(create.ends_with("/F"), "{create}");
        let leftovers: Vec<_> = std::fs::read_dir(&state)
            .expect("read")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            leftovers,
            vec![DEFINITION_FILE.to_string()],
            "the copy is removed"
        );
    }

    #[test]
    fn a_create_that_fails_with_the_scheduler_gone_is_69() {
        let state = scratch("gone");
        let recorder = Recorder::default();
        let manager = manager(&recorder, &state, false);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        recorder.exit("schtasks /Create", 1, "", "ERROR: access denied");
        recorder.exit("schtasks /Query /FO CSV /NH", 1, "", "service down");
        let err = manager
            .register(&spec(StartsAt::Login), true, true)
            .expect_err("failed");
        assert_eq!(err.code, 69, "{}", err.message);
    }

    #[test]
    fn a_graceful_stop_never_calls_end_and_a_forced_one_does() {
        let state = scratch("stop");
        let recorder = Recorder::default();
        let mut manager = manager(&recorder, &state, false);
        manager.signal = Box::new(|_| Ok(true));
        // A live supervisor: this test process, as the record names it.
        let pid = std::process::id();
        SupervisorRecord {
            pid,
            start: lock::process_start_ticks(pid).expect("start time"),
            child: 4242,
            stopping: false,
        }
        .write(&state)
        .expect("record");
        manager.stop(false).expect("graceful");
        assert!(
            !recorder.calls().iter().any(|call| call.contains("/End")),
            "{:?}",
            recorder.calls()
        );
        manager.stop(true).expect("forced");
        assert!(
            recorder
                .calls()
                .iter()
                .any(|call| call == "schtasks /End /TN Agent Hub"),
            "{:?}",
            recorder.calls()
        );
    }

    #[test]
    fn a_stop_that_cannot_reach_the_supervisor_times_out_and_ends_nothing() {
        let state = scratch("stop-unreachable");
        let recorder = Recorder::default();
        let mut manager = manager(&recorder, &state, false);
        manager.waits = quick();
        // A supervisor whose event is not there.
        manager.probe = Box::new(|_| Some(record(10, 1, false)));
        manager.signal = Box::new(|_| Ok(false));
        let err = manager.stop(false).expect_err("timed out");
        assert_eq!(err.code, 1, "{}", err.message);
        assert!(err.message.contains("--force"), "{}", err.message);
        assert!(
            !recorder.calls().iter().any(|call| call.contains("/End")),
            "{:?}",
            recorder.calls()
        );

        // The task is starting and no supervisor has published yet.
        let starting = Recorder::default();
        starting.exit(
            "schtasks /Query /TN Agent Hub /FO CSV",
            0,
            &format!("\"PC\",\"\\Agent Hub\",\"N/A\",\"x\",\"x\",\"-\",\"{LAST_RESULT_RUNNING}\",\"u\"\r\n"),
            "",
        );
        let mut manager = self::manager(&starting, &state, false);
        manager.waits = quick();
        manager.probe = Box::new(|_| None);
        assert!(manager.stop(false).is_err());
        assert!(!starting.calls().iter().any(|call| call.contains("/End")));
    }

    #[test]
    fn a_stop_retries_until_a_starting_supervisor_publishes_its_event() {
        let state = scratch("stop-late");
        let recorder = Recorder::default();
        let mut manager = manager(&recorder, &state, false);
        manager.waits = quick();
        manager.probe = Box::new(|_| Some(record(10, 1, false)));
        let tries = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = tries.clone();
        manager.signal = Box::new(move |_| {
            counted.set(counted.get() + 1);
            Ok(counted.get() >= 3)
        });
        manager.stop(false).expect("reached on the third try");
        assert_eq!(tries.get(), 3);
        assert!(!recorder.calls().iter().any(|call| call.contains("/End")));
    }

    #[test]
    fn a_stop_with_nothing_running_is_done() {
        let state = scratch("stop-idle");
        let recorder = Recorder::default();
        idle(&recorder);
        let mut manager = manager(&recorder, &state, false);
        manager.probe = Box::new(|_| None);
        manager.stop(false).expect("nothing to stop");
        assert!(!recorder.calls().iter().any(|call| call.contains("/End")));
    }

    #[test]
    fn the_hub_the_supervisor_runs_is_the_main_pid_only_while_it_is_alive() {
        let state = scratch("mainpid");
        let manager = manager(&Recorder::default(), &state, false);
        assert_eq!(manager.main_pid(), Ok(None));
        let pid = std::process::id();
        let start = lock::process_start_ticks(pid).expect("start time");
        SupervisorRecord {
            pid,
            start,
            child: 777,
            stopping: false,
        }
        .write(&state)
        .expect("record");
        assert_eq!(manager.main_pid(), Ok(Some(777)));
        SupervisorRecord {
            pid,
            start,
            child: 0,
            stopping: false,
        }
        .write(&state)
        .expect("record");
        assert_eq!(
            manager.main_pid(),
            Ok(None),
            "between a crash and the restart"
        );
        SupervisorRecord {
            pid,
            start: start + 1,
            child: 777,
            stopping: false,
        }
        .write(&state)
        .expect("record");
        assert_eq!(
            manager.main_pid(),
            Ok(None),
            "a reused pid is not the supervisor"
        );
    }

    #[test]
    fn state_reads_the_numeric_result_and_the_xml_not_the_words() {
        let state = scratch("state");
        let recorder = Recorder::default();
        let row = |result: &str| {
            format!("\"PC\",\"\\Agent Hub\",\"N/A\",\"Ready\",\"x\",\"-\",\"{result}\",\"u\"\r\n")
        };
        recorder.exit("schtasks /Query /TN Agent Hub /FO CSV", 0, &row("78"), "");
        recorder.exit(
            "schtasks /Query /TN Agent Hub /XML",
            0,
            "<Task><Triggers><LogonTrigger/></Triggers><Settings></Settings></Task>",
            "",
        );
        let manager = manager(&recorder, &state, false);
        let failed = manager.state().expect("state");
        assert_eq!(failed.active, Active::Failed);
        assert_eq!(failed.detail.as_deref(), Some("configuration error"));
        assert!(failed.enabled);
        assert_eq!(failed.starts_at, "login");

        let quiet = Recorder::default();
        quiet.exit(
            "schtasks /Query /TN Agent Hub /FO CSV",
            0,
            &row("267014"),
            "",
        );
        quiet.exit(
            "schtasks /Query /TN Agent Hub /XML",
            0,
            "<Task><Triggers><BootTrigger/></Triggers><Settings><Enabled>false</Enabled></Settings></Task>",
            "",
        );
        let disabled = self::manager(&quiet, &state, false).state().expect("state");
        assert_eq!(
            disabled.active,
            Active::Inactive,
            "ended by the user is not a failure"
        );
        assert!(!disabled.enabled);
        assert_eq!(disabled.starts_at, "manual");

        let running = Recorder::default();
        running.exit(
            "schtasks /Query /TN Agent Hub /FO CSV",
            0,
            &row("267009"),
            "",
        );
        let up = self::manager(&running, &state, false)
            .state()
            .expect("state");
        assert_eq!(up.active, Active::Running);
    }

    #[test]
    fn an_unregistered_task_is_inactive_and_an_unanswering_scheduler_is_an_error() {
        let state = scratch("absent");
        let recorder = Recorder::default();
        recorder.exit("schtasks /Query /TN Agent Hub", 1, "", "cannot find");
        recorder.exit("powershell.exe", 3, "", "");
        let absent = manager(&recorder, &state, false).state().expect("state");
        assert_eq!(absent.active, Active::Inactive);
        assert!(absent.detail.expect("detail").contains("not registered"));

        let down = Recorder::default();
        down.exit("schtasks /Query", 1, "", "service down");
        down.exit("powershell.exe", 3, "", "");
        assert!(manager(&down, &state, false).state().is_err());

        // Registered, but the query failed: not a state to report.
        let unread = Recorder::default();
        unread.exit("schtasks /Query /TN Agent Hub", 1, "", "access denied");
        unread.exit("powershell.exe", 0, &exported("<Task/>"), "");
        assert!(manager(&unread, &state, false).state().is_err());
    }

    #[test]
    fn job_reads_the_config_from_the_export_exactly_or_refuses() {
        let state = scratch("job");
        let recorder = Recorder::default();
        let xml = render_task(&spec(StartsAt::Login), SID, &work()).expect("render");
        recorder.exit("powershell.exe", 0, &exported(&xml), "");
        let job = manager(&recorder, &state, false).job().expect("job");
        assert_eq!(
            job,
            Job::Loaded {
                config: Some(PathBuf::from(
                    "C:\\Users\\u\\AppData\\Roaming\\agent-hub\\config.toml"
                ))
            }
        );
        // A path beyond ASCII comes through as it is, so it compares.
        let accented = Recorder::default();
        accented.exit(
            "powershell.exe",
            0,
            &exported(&format!(
                "<Command>%SystemRoot%\\System32\\conhost.exe</Command><Arguments>--headless &quot;{INSTALLED}&quot; service run --config &quot;C:\\Users\\J\u{f6}rg\\c.toml&quot;</Arguments>"
            )),
            "",
        );
        assert_eq!(
            manager(&accented, &state, false).job().expect("job"),
            Job::Loaded {
                config: Some(PathBuf::from("C:\\Users\\J\u{f6}rg\\c.toml"))
            }
        );
        // Registered but names no config: cannot be verified, so refused.
        let bare = Recorder::default();
        bare.exit("powershell.exe", 0, &exported("<Task/>"), "");
        let refused = manager(&bare, &state, false)
            .job()
            .expect_err("unverifiable");
        assert!(
            refused.message.contains("cannot be verified"),
            "{}",
            refused.message
        );

        let none = Recorder::default();
        none.exit("powershell.exe", 3, "", "");
        assert_eq!(
            manager(&none, &state, false).job().expect("job"),
            Job::Absent
        );
    }

    #[test]
    fn a_task_that_cannot_be_read_is_unknown_never_absent() {
        let state = scratch("job-unknown");
        for (code, stdout) in [(4, ""), (1, ""), (0, "not hex"), (0, "")] {
            let recorder = Recorder::default();
            recorder.exit("powershell.exe", code, stdout, "Access is denied.");
            let manager = manager(&recorder, &state, false);
            assert!(manager.job().is_err(), "exit {code} {stdout:?}");
            assert!(manager.task_xml().is_err(), "exit {code} {stdout:?}");
        }
        let missing = Recorder::default();
        missing.missing("powershell.exe");
        assert!(manager(&missing, &state, false).job().is_err());
    }

    #[test]
    fn the_export_is_hex_of_utf8_and_exit_3_is_absent() {
        let outcome = |code: i32, stdout: &str| Outcome {
            code: Some(code),
            stdout: stdout.to_string(),
            stderr: String::new(),
        };
        assert_eq!(
            read_export(&outcome(0, "3C613EC3A93C2F613E\r\n")),
            Export::Task("<a>\u{e9}</a>".to_string())
        );
        assert_eq!(read_export(&outcome(3, "")), Export::Absent);
        assert!(matches!(read_export(&outcome(4, "")), Export::Unknown(_)));
        assert!(matches!(
            read_export(&outcome(0, "3C6")),
            Export::Unknown(_)
        ));
        assert!(matches!(read_export(&outcome(0, "FF")), Export::Unknown(_)));
        assert!(EXPORT_TASK.contains("-TaskPath '\\'"), "{EXPORT_TASK}");
        assert!(
            EXPORT_TASK.contains("-TaskName 'Agent Hub'"),
            "{EXPORT_TASK}"
        );
    }

    #[test]
    fn remove_deletes_the_task_then_the_definition_and_the_record() {
        let state = scratch("remove");
        let recorder = Recorder::default();
        idle(&recorder);
        recorder.exit("powershell.exe", 0, &ours(), "");
        let manager = manager(&recorder, &state, false);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        std::fs::write(SupervisorRecord::path_in(&state), "{}").expect("record");
        std::fs::write(state.join(SUPERVISOR_LOCK), "").expect("lock");
        manager.remove().expect("remove");
        assert!(
            recorder
                .calls()
                .iter()
                .any(|call| call == "schtasks /Delete /TN Agent Hub /F")
        );
        assert!(!manager.unit_path().exists());
        assert!(!SupervisorRecord::path_in(&state).exists());
        assert!(!state.join(SUPERVISOR_LOCK).exists());
    }

    #[test]
    fn remove_keeps_the_definition_when_the_delete_fails() {
        let state = scratch("remove-fails");
        let recorder = Recorder::default();
        idle(&recorder);
        recorder.exit("schtasks /Delete", 1, "", "access denied");
        recorder.exit("powershell.exe", 0, &ours(), "");
        let manager = manager(&recorder, &state, false);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        let err = manager.remove().expect_err("refused");
        assert_eq!(err.code, 1, "{}", err.message);
        assert!(manager.unit_path().exists());
    }

    #[test]
    fn remove_of_a_task_that_was_never_registered_still_cleans_up() {
        let state = scratch("remove-absent");
        let recorder = Recorder::default();
        recorder.exit("powershell.exe", 3, "", "");
        let manager = manager(&recorder, &state, false);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        manager.remove().expect("remove");
        assert!(!recorder.calls().iter().any(|call| call.contains("/Delete")));
        assert!(!manager.unit_path().exists());
    }

    #[test]
    fn remove_refuses_when_the_task_cannot_be_read() {
        let state = scratch("remove-unknown");
        let recorder = Recorder::default();
        recorder.exit("powershell.exe", 4, "", "Access is denied.");
        let manager = manager(&recorder, &state, false);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        assert!(manager.remove().is_err());
        assert!(!recorder.calls().iter().any(|call| call.contains("/Delete")));
        assert!(manager.unit_path().exists());
    }

    #[test]
    fn only_the_recorded_path_entry_goes_and_every_other_stays_verbatim() {
        let entry = "C:\\Users\\U\\AppData\\Local\\Programs\\agent-hub";
        let value = "C:\\Windows;;%USERPROFILE%\\bin;c:\\users\\u\\appdata\\local\\programs\\AGENT-HUB;D:\\tools";
        assert_eq!(
            without_path_entry(value, entry).as_deref(),
            Some("C:\\Windows;;%USERPROFILE%\\bin;D:\\tools")
        );
        // Only the exact entry: another spelling of the directory is the
        // user's own and stays.
        assert_eq!(
            without_path_entry("%LOCALAPPDATA%\\Programs\\agent-hub;D:\\tools", entry),
            None
        );
        assert_eq!(without_path_entry(value, "C:\\other"), None);
        assert_eq!(without_path_entry("", entry), None);
        assert_eq!(without_path_entry("C:\\a\\bc", "C:\\a\\b"), None);
        assert_eq!(without_path_entry(&format!("{entry}\\"), entry), None);
        // Case beyond ASCII is folded the way Windows does.
        assert_eq!(
            without_path_entry(
                "C:\\\u{c9}T\u{c9}\\agent-hub",
                "c:\\\u{e9}t\u{e9}\\AGENT-HUB"
            ),
            Some(String::new())
        );
    }

    #[test]
    fn a_marker_owns_the_entry_only_for_the_directory_it_names() {
        let dir = "C:\\Users\\U\\AppData\\Local\\Programs\\agent-hub";
        // The script writes the entry with no newline; one is tolerated.
        assert_eq!(marked_entry(dir, dir).as_deref(), Some(dir));
        assert_eq!(
            marked_entry(&format!("{dir}\r\n"), dir).as_deref(),
            Some(dir)
        );
        assert_eq!(
            marked_entry(&dir.to_lowercase(), dir).as_deref(),
            Some(dir.to_lowercase().as_str())
        );
        assert_eq!(marked_entry("C:\\Other\\agent-hub", dir), None);
        assert_eq!(marked_entry("", dir), None);
        assert_eq!(marked_entry("\r\n", dir), None);

        let state = scratch("marker");
        // No marker: the directory was on the PATH before the install.
        assert_eq!(owned_path_entry(Some(&state), dir), None);
        assert_eq!(owned_path_entry(None, dir), None);
        std::fs::write(state.join(PATH_MARKER), "C:\\Other\\agent-hub").expect("marker");
        assert_eq!(owned_path_entry(Some(&state), dir), None);
        std::fs::write(state.join(PATH_MARKER), dir).expect("marker");
        let (entry, file) = owned_path_entry(Some(&state), dir).expect("owned");
        assert_eq!(entry, dir);
        assert_eq!(file, state.join("path-added"));
    }

    #[test]
    fn rollback_names_are_exact() {
        assert!(is_rollback_name("agent-hub.exe.0123abcd.old"));
        assert!(is_rollback_name("AGENT-HUB.EXE.0123ABCD.OLD"));
        for name in [
            "agent-hub.exe.7.old",
            "agent-hub.exe.0123abcde.old",
            "agent-hub.exe.0123abcg.old",
            "agent-hub.exe.0123abcd.old.txt",
            "other.exe.0123abcd.old",
            "agent-hub.exe..old",
        ] {
            assert!(!is_rollback_name(name), "{name}");
        }
        for text in [
            "C:\\a%b", "C:\\a!b", "C:\\a\"b", "C:\\a^b", "C:\\a&b", "C:\\a|b", "C:\\a<b",
            "C:\\a>b", "C:\\a\nb",
        ] {
            assert!(!cmd_literal(text), "{text:?}");
        }
        assert!(cmd_literal("C:\\Users\\J\u{f6}rg (work)\\agent-hub"));
    }

    fn install_dir(name: &str) -> PathBuf {
        let root = std::path::absolute(scratch(name)).expect("absolute");
        let dir = root.join("agent-hub");
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn the_helper_gets_only_regular_files_of_the_exact_names() {
        let dir = install_dir("retire");
        let binary = dir.join("agent-hub.exe");
        std::fs::write(&binary, "exe").expect("binary");
        std::fs::write(dir.join("agent-hub.exe.0123abcd.old"), "old").expect("old");
        // A directory with a rollback name, and a file inside it: never handed over.
        std::fs::create_dir_all(dir.join("agent-hub.exe.deadbeef.old")).expect("decoy dir");
        std::fs::write(
            dir.join("agent-hub.exe.deadbeef.old").join("keep.txt"),
            "keep",
        )
        .expect("keep");
        std::fs::write(dir.join("agent-hub.exe.7.old"), "not ours").expect("short name");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            dir.join("agent-hub.exe.0123abcd.old"),
            dir.join("agent-hub.exe.0badf00d.old"),
        )
        .expect("link");
        let state = dir.parent().expect("root").join("state");
        std::fs::create_dir_all(&state).expect("state");
        let previous = state.join("agent-hub.prev.exe");
        std::fs::write(&previous, "prev").expect("previous");

        let plan = retire_plan(&binary, Some(&previous)).expect("plan");
        let text = |path: PathBuf| path.to_str().expect("utf8").to_string();
        assert_eq!(
            plan,
            Retire {
                files: vec![
                    text(binary.clone()),
                    text(dir.join("agent-hub.exe.0123abcd.old")),
                    text(previous.clone()),
                ],
                dir: Some(text(dir.clone())),
            }
        );

        // A previous binary that is a directory is left alone.
        std::fs::remove_file(&previous).expect("rm");
        std::fs::create_dir_all(&previous).expect("prev dir");
        let plan = retire_plan(&binary, Some(&previous)).expect("plan");
        assert!(!plan.files.contains(&text(previous)), "{plan:?}");
    }

    #[test]
    fn a_binary_the_helper_cannot_be_trusted_with_is_refused() {
        let dir = install_dir("retire-refused");
        // Not a file.
        let as_dir = dir.join("agent-hub.exe");
        std::fs::create_dir_all(&as_dir).expect("dir");
        assert!(retire_plan(&as_dir, None).is_err());
        // Not the installed name.
        let other = dir.join("hub.exe");
        std::fs::write(&other, "exe").expect("file");
        assert!(retire_plan(&other, None).is_err());
        // Not absolute.
        assert!(retire_plan(Path::new("agent-hub.exe"), None).is_err());
        // A character cmd.exe would not take literally.
        let bang = dir.join("x!y");
        std::fs::create_dir_all(&bang).expect("bang");
        std::fs::write(bang.join("agent-hub.exe"), "exe").expect("file");
        let refused = retire_plan(&bang.join("agent-hub.exe"), None).expect_err("refused");
        assert!(refused.contains("literally"), "{refused}");
    }

    #[test]
    fn a_shared_directory_is_neither_removed_nor_taken_off_the_path() {
        let root = std::path::absolute(scratch("retire-shared")).expect("absolute");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        std::fs::write(bin.join("agent-hub.exe"), "exe").expect("binary");
        let plan = retire_plan(&bin.join("agent-hub.exe"), None).expect("plan");
        assert_eq!(plan.dir, None);
    }

    #[test]
    fn the_delete_line_has_no_delayed_expansion_and_never_deletes_a_directory() {
        let plan = Retire {
            files: vec![
                "C:\\p\\agent-hub\\agent-hub.exe".to_string(),
                "C:\\p\\agent-hub\\agent-hub.exe.0123abcd.old".to_string(),
            ],
            dir: Some("C:\\p\\agent-hub".to_string()),
        };
        assert_eq!(
            delete_command(&plan),
            "/d /v:off /q /c for /l %i in (1,1,60) do (\
             (if exist \"C:\\p\\agent-hub\\agent-hub.exe\" if not exist \"C:\\p\\agent-hub\\agent-hub.exe\\*\" del /f /q \"C:\\p\\agent-hub\\agent-hub.exe\" >nul 2>&1) & \
             (if exist \"C:\\p\\agent-hub\\agent-hub.exe.0123abcd.old\" if not exist \"C:\\p\\agent-hub\\agent-hub.exe.0123abcd.old\\*\" del /f /q \"C:\\p\\agent-hub\\agent-hub.exe.0123abcd.old\" >nul 2>&1) & \
             (if exist \"C:\\p\\agent-hub\\*\" rmdir \"C:\\p\\agent-hub\" >nul 2>&1) & \
             (if exist \"C:\\p\\agent-hub\\agent-hub.exe\" ping -n 2 127.0.0.1 >nul 2>&1))"
        );
        let line = delete_command(&plan);
        assert!(!line.contains("/s"), "never a recursive delete: {line}");
    }

    #[test]
    fn start_leaves_a_running_supervisor_alone_and_waits_out_a_stopping_one() {
        let state = scratch("start-plan");
        assert_eq!(before_run(None, false), BeforeRun::Run);
        assert_eq!(before_run(None, true), BeforeRun::WaitForExit);
        assert_eq!(
            before_run(Some(&record(1, 1, false)), true),
            BeforeRun::AlreadyRunning
        );
        assert_eq!(
            before_run(Some(&record(1, 1, true)), true),
            BeforeRun::WaitForExit
        );
        assert!(is_new(None, &record(1, 1, false)));
        assert!(!is_new(Some(&record(1, 1, true)), &record(1, 1, false)));
        assert!(is_new(Some(&record(1, 1, true)), &record(1, 2, false)));
        assert!(is_new(Some(&record(1, 1, true)), &record(2, 1, false)));

        // Running and not stopping: no /Run, which IgnoreNew would drop.
        let recorder = Recorder::default();
        let mut running = manager(&recorder, &state, false);
        running.probe = Box::new(|_| Some(record(1, 1, false)));
        running.start().expect("already running");
        assert!(!recorder.calls().iter().any(|call| call.contains("/Run")));

        // Stopping and never gone: no /Run, and it says so.
        let recorder = Recorder::default();
        let mut stuck = manager(&recorder, &state, false);
        stuck.waits = quick();
        stuck.probe = Box::new(|_| Some(record(1, 1, true)));
        let err = stuck.start().expect_err("still running");
        assert!(err.message.contains("still running"), "{}", err.message);
        assert!(!recorder.calls().iter().any(|call| call.contains("/Run")));
    }

    #[test]
    fn a_restart_runs_the_task_once_the_old_supervisor_is_gone_and_wants_a_new_one() {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        use std::rc::Rc;

        let state = scratch("restart");
        // The old supervisor is stopping, then gone; after /Run the old record
        // is seen once more (as if not yet replaced), then the new one.
        let script = |records: Vec<Option<SupervisorRecord>>| {
            let queue = Rc::new(RefCell::new(VecDeque::from(records)));
            Box::new(move |_: &Path| {
                let mut queue = queue.borrow_mut();
                if queue.len() > 1 {
                    queue.pop_front().flatten()
                } else {
                    queue.front().cloned().flatten()
                }
            }) as Probe
        };
        let recorder = Recorder::default();
        idle(&recorder);
        let mut manager = manager(&recorder, &state, false);
        manager.waits = quick();
        manager.probe = script(vec![
            Some(record(1, 1, true)),
            None,
            Some(record(1, 1, false)),
            Some(record(2, 5, false)),
        ]);
        manager.start().expect("started");
        assert_eq!(
            recorder
                .calls()
                .iter()
                .filter(|call| call.starts_with("schtasks /Run"))
                .count(),
            1
        );

        // The old one never makes way for a new one: not reported started.
        let recorder = Recorder::default();
        idle(&recorder);
        let mut manager = self::manager(&recorder, &state, false);
        manager.waits = quick();
        manager.probe = script(vec![Some(record(1, 1, true)), None, None]);
        let err = manager.start().expect_err("no new supervisor");
        assert!(
            err.message.contains("no supervisor appeared"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_scheduler_that_cannot_be_read_is_unknown_so_start_never_runs_and_stop_never_succeeds() {
        let state = scratch("unknown-query");
        let broken: [(&str, i32, &str); 3] = [
            ("fails", 1, ""),
            ("prints nothing", 0, ""),
            ("prints no result", 0, "\"PC\",\"x\"\r\n"),
        ];
        for (what, code, stdout) in broken {
            let recorder = Recorder::default();
            recorder.exit(
                "schtasks /Query /TN Agent Hub /FO CSV",
                code,
                stdout,
                "Access is denied.",
            );
            let mut manager = manager(&recorder, &state, false);
            manager.waits = quick();
            manager.probe = Box::new(|_| None);
            let err = manager.start().expect_err(what);
            assert_eq!(err.code, 69, "{what}: {}", err.message);
            assert!(err.message.contains("unknown"), "{what}: {}", err.message);
            let err = manager.stop(false).expect_err(what);
            assert_eq!(err.code, 69, "{what}: {}", err.message);
            assert!(err.message.contains("unknown"), "{what}: {}", err.message);
            let calls = recorder.calls();
            assert!(
                !calls
                    .iter()
                    .any(|call| call.contains("/Run") || call.contains("/End")),
                "{what}: {calls:?}"
            );
        }
        // A schtasks that cannot be run at all is unknown too.
        let missing = Recorder::default();
        missing.missing("schtasks /Query /TN Agent Hub");
        let mut manager = manager(&missing, &state, false);
        manager.probe = Box::new(|_| None);
        assert_eq!(manager.start().expect_err("start").code, 69);
        assert_eq!(manager.stop(false).expect_err("stop").code, 69);

        // And the status read does not call an unreadable result stopped.
        let unread = Recorder::default();
        unread.exit(
            "schtasks /Query /TN Agent Hub /FO CSV",
            0,
            "\"PC\",\"x\"\r\n",
            "",
        );
        let mut manager = self::manager(&unread, &state, false);
        manager.probe = Box::new(|_| None);
        assert!(manager.state().is_err());
        // With a live supervisor the answer is known whatever the query says.
        manager.probe = Box::new(|_| Some(record(1, 1, false)));
        assert_eq!(manager.state().expect("running").active, Active::Running);
    }

    #[test]
    fn the_task_is_ours_only_if_it_runs_the_installed_binary() {
        let state = scratch("identity");
        let other = |exe: &str| {
            let xml = render_task(
                &UnitSpec {
                    binary: PathBuf::from(exe),
                    ..spec(StartsAt::Login)
                },
                SID,
                &work(),
            )
            .expect("render");
            let recorder = Recorder::default();
            recorder.exit("powershell.exe", 0, &exported(&xml), "");
            manager(&recorder, &state, false)
        };
        // Another program in the same folder, another folder, a bare name.
        for exe in [
            "C:\\Users\\u\\AppData\\Local\\Programs\\agent-hub\\other.exe",
            "D:\\tools\\agent-hub.exe",
            "agent-hub.exe",
        ] {
            let err = other(exe).job().expect_err(exe);
            assert_eq!(err.code, 1, "{}", err.message);
            assert!(err.message.contains(exe), "{}", err.message);
            assert!(
                err.message.contains("not this install's"),
                "{}",
                err.message
            );
            assert!(err.message.contains("schtasks /Delete"), "{}", err.message);
        }
        // The same file spelled with other case or forward slashes is the
        // installed one.
        let slashes = INSTALLED.replace('\\', "/");
        for exe in [INSTALLED.to_uppercase(), slashes] {
            assert!(other(&exe).job().is_ok(), "{exe}");
        }
        // A program that is not named at all, or no known own path, is unknown.
        let recorder = Recorder::default();
        recorder.exit(
            "powershell.exe",
            0,
            &exported("<Arguments>--config &quot;C:\\c.toml&quot;</Arguments>"),
            "",
        );
        assert!(manager(&recorder, &state, false).job().is_err());
        let xml = render_task(&spec(StartsAt::Login), SID, &work()).expect("render");
        let recorder = Recorder::default();
        recorder.exit("powershell.exe", 0, &exported(&xml), "");
        let blind = manager(&recorder, &state, false).with_binary(None);
        assert!(blind.job().is_err());
    }

    #[test]
    fn the_program_a_task_runs_is_read_through_the_headless_host_and_expanded() {
        let xml = render_task(&spec(StartsAt::Login), SID, &work()).expect("render");
        assert_eq!(task_executable(&xml).as_deref(), Some(INSTALLED));
        // Escaped as the scheduler's export writes it, with a space in the path.
        assert_eq!(
            task_executable(
                "<Command>C:\\Windows\\System32\\CONHOST.EXE</Command>\
                 <Arguments>--headless &quot;C:\\A &amp; B\\agent-hub.exe&quot; service run</Arguments>"
            )
            .as_deref(),
            Some("C:\\A & B\\agent-hub.exe")
        );
        // Run directly, with no host.
        assert_eq!(
            task_executable(
                "<Command>\"C:\\x\\agent-hub.exe\"</Command><Arguments>service run</Arguments>"
            )
            .as_deref(),
            Some("C:\\x\\agent-hub.exe")
        );
        assert_eq!(task_executable("<Task/>"), None);
        assert_eq!(task_executable("<Command></Command>"), None);

        let env = |key: &str| match key {
            "LOCALAPPDATA" => Some("C:\\Users\\u\\AppData\\Local".to_string()),
            _ => None,
        };
        assert_eq!(
            expand_vars("%localappdata%\\Programs\\%MISSING%\\100%", &env),
            "C:\\Users\\u\\AppData\\Local\\Programs\\%MISSING%\\100%"
        );
        assert!(same_executable(
            "%LocalAppData%\\Programs\\agent-hub\\agent-hub.exe",
            INSTALLED,
            &env
        ));
        assert!(!same_executable(
            "%LOCALAPPDATA%\\Programs\\agent-hub\\agent-hub.exe.old",
            INSTALLED,
            &env
        ));
        // An unknown variable never equals a real path.
        assert!(!same_executable("%NOPE%\\agent-hub.exe", INSTALLED, &env));
    }

    #[test]
    fn remove_never_ends_a_task_that_is_still_running() {
        let state = scratch("remove-busy");
        // The scheduler says it runs, with no supervisor on record.
        let recorder = Recorder::default();
        recorder.exit(
            "schtasks /Query /TN Agent Hub /FO CSV",
            0,
            &query_row(LAST_RESULT_RUNNING),
            "",
        );
        recorder.exit("powershell.exe", 0, &ours(), "");
        let mut manager = manager(&recorder, &state, false);
        manager.waits = quick();
        manager.probe = Box::new(|_| None);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        let err = manager.remove().expect_err("busy");
        assert_eq!(err.code, 1, "{}", err.message);
        assert!(err.message.contains("stop --force"), "{}", err.message);
        let calls = recorder.calls();
        assert!(
            !calls
                .iter()
                .any(|call| call.contains("/End") || call.contains("/Delete")),
            "{calls:?}"
        );
        assert!(manager.unit_path().exists(), "nothing changed");

        // A supervisor that never leaves is the same.
        let recorder = Recorder::default();
        idle(&recorder);
        recorder.exit("powershell.exe", 0, &ours(), "");
        let mut manager = self::manager(&recorder, &state, false);
        manager.waits = quick();
        manager.probe = Box::new(|_| Some(record(1, 1, true)));
        let err = manager.remove().expect_err("busy");
        assert!(err.message.contains("stop --force"), "{}", err.message);
        assert!(
            !recorder
                .calls()
                .iter()
                .any(|call| call.contains("/End") || call.contains("/Delete"))
        );

        // A scheduler that cannot say is not idle.
        let recorder = Recorder::default();
        recorder.exit("schtasks /Query /TN Agent Hub /FO CSV", 1, "", "denied");
        recorder.exit("powershell.exe", 0, &ours(), "");
        let mut manager = self::manager(&recorder, &state, false);
        manager.probe = Box::new(|_| None);
        let err = manager.remove().expect_err("unknown");
        assert_eq!(err.code, 69, "{}", err.message);
        assert!(
            !recorder
                .calls()
                .iter()
                .any(|call| call.contains("/End") || call.contains("/Delete"))
        );
    }

    #[test]
    fn remove_waits_for_a_supervisor_that_is_leaving_and_then_deletes_without_ending() {
        use std::cell::Cell;
        use std::rc::Rc;
        let state = scratch("remove-draining");
        let recorder = Recorder::default();
        idle(&recorder);
        recorder.exit("powershell.exe", 0, &ours(), "");
        let mut manager = manager(&recorder, &state, false);
        manager.waits = quick();
        let polls = Rc::new(Cell::new(0));
        let seen = polls.clone();
        manager.probe = Box::new(move |_| {
            seen.set(seen.get() + 1);
            (seen.get() < 4).then(|| record(1, 1, true))
        });
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        manager.remove().expect("removed once it was gone");
        assert!(polls.get() >= 4);
        let calls = recorder.calls();
        assert!(
            calls.iter().any(|call| call.contains("/Delete")),
            "{calls:?}"
        );
        assert!(!calls.iter().any(|call| call.contains("/End")), "{calls:?}");
    }

    #[test]
    fn remove_leaves_a_task_that_runs_another_binary() {
        let state = scratch("remove-stray");
        let xml = render_task(
            &UnitSpec {
                binary: PathBuf::from("D:\\elsewhere\\agent-hub.exe"),
                ..spec(StartsAt::Login)
            },
            SID,
            &work(),
        )
        .expect("render");
        let recorder = Recorder::default();
        idle(&recorder);
        recorder.exit("powershell.exe", 0, &exported(&xml), "");
        let manager = manager(&recorder, &state, false);
        std::fs::write(manager.unit_path(), "<Task/>").expect("definition");
        let err = manager.remove().expect_err("not ours");
        assert!(
            err.message.contains("not this install's"),
            "{}",
            err.message
        );
        assert!(
            !recorder
                .calls()
                .iter()
                .any(|call| call.contains("/Delete") || call.contains("/End"))
        );
        assert!(manager.unit_path().exists());
    }
}
