//! The lifecycle verbs, acting through an installed service.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_hub::config::LifecycleSettings;

use super::{Active, Manager, Unavailable};
use crate::daemon::lock::{self, Holder, Mode, Record};
use crate::daemon::process::{self, Process};
use crate::daemon::{Backend, LogsOptions, Managed, StopOptions};

/// How long `start` waits for the service to bind and answer `/readyz`.
const START_WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(100);
/// How often the manager is asked whether the service died while `start` waits.
const STATE_EVERY: Duration = Duration::from_millis(500);

/// The service manager behind the lifecycle verbs.
pub struct ServiceBackend {
    manager: Box<dyn Manager>,
}

impl ServiceBackend {
    pub fn new(manager: Box<dyn Manager>) -> Self {
        Self { manager }
    }

    pub fn manager(&self) -> &dyn Manager {
        self.manager.as_ref()
    }
}

fn fail(message: String) -> ExitCode {
    eprintln!("agent-hub: {message}");
    ExitCode::FAILURE
}

/// The manager could not be asked: 69, the same as no manager at all.
fn unreachable(message: String) -> ExitCode {
    eprintln!("agent-hub: {message}; nothing was signalled");
    ExitCode::from(69)
}

/// A manager step that failed: 69 when the manager no longer answers, else 1.
fn refused(err: Unavailable, what: &str) -> ExitCode {
    if err.code == 69 {
        unreachable(format!("{what}: {}", err.message))
    } else {
        fail(format!("{what}: {}", err.message))
    }
}

impl Backend for ServiceBackend {
    fn managed(&self, _settings: &LifecycleSettings, holder: Option<&Record>) -> Managed {
        if let Some(record) = holder
            && record.mode == Mode::Mcp
        {
            return Managed {
                kind: "process",
                label: "an embedded agent-hub mcp session".to_string(),
                enabled: false,
                starts_at: "manual",
            };
        }
        let kind = self.manager.kind();
        let name = match kind {
            "launchd" => "launchd agent",
            "task" => "scheduled task",
            _ => "systemd user service",
        };
        let state = match self.manager.state() {
            Ok(state) => state,
            Err(message) => {
                return Managed {
                    kind,
                    label: format!("{name} (the manager does not answer: {message})"),
                    enabled: false,
                    starts_at: "manual",
                };
            }
        };
        let mut parts = Vec::new();
        if state.enabled {
            parts.push(format!("enabled, starts at {}", state.starts_at));
        } else if kind == "launchd" {
            parts.push("disabled in Login Items".to_string());
        } else {
            parts.push("disabled".to_string());
        }
        if state.active == Active::Failed {
            parts.push(format!(
                "failed: {}",
                state.detail.as_deref().unwrap_or("see agent-hub logs")
            ));
        } else if let Some(detail) = &state.detail {
            parts.push(detail.clone());
        }
        let mut label = format!("{name} ({})", parts.join("; "));
        if let Some(record) = holder
            && self.owns(record) == Ok(false)
        {
            label = if record.managed_by == kind {
                format!(
                    "{label}; the running hub (pid {}) is not the service's own process",
                    record.pid
                )
            } else {
                format!(
                    "{label}; the running hub was started by {}",
                    record.manager_label()
                )
            };
        }
        Managed {
            kind,
            label,
            enabled: state.enabled,
            starts_at: state.starts_at,
        }
    }

    fn start(&self, settings: &LifecycleSettings) -> ExitCode {
        let data_dir = &settings.data_dir;
        let began = Instant::now();
        let mut said = false;
        // A hub already holding the store decides what a start means.
        loop {
            match lock::inspect(data_dir) {
                Ok(Holder::Free) => break,
                Ok(Holder::Held(Some(record))) if record.mode == Mode::Mcp => {
                    return fail(format!(
                        "an embedded agent-hub mcp session (pid {}) holds the store, not a hub; \
                         end that session first",
                        record.pid
                    ));
                }
                Ok(Holder::Held(Some(record))) if process::ready(&record) => {
                    println!("already running (pid {})", record.pid);
                    return ExitCode::SUCCESS;
                }
                Ok(Holder::Held(_)) => {
                    if began.elapsed() >= START_WAIT {
                        return fail(format!(
                            "a process holds {} and no ready hub answered within {}s; see \
                             agent-hub status and agent-hub logs",
                            lock::path_in(data_dir).display(),
                            START_WAIT.as_secs()
                        ));
                    }
                    if !said {
                        eprintln!("agent-hub start: a hub is starting; waiting for it to answer");
                        said = true;
                    }
                }
                Err(err) => {
                    return fail(format!("could not read {}: {err}", data_dir.display()));
                }
            }
            std::thread::sleep(POLL);
        }

        if let Some(problem) = super::port_problem(settings.bind, data_dir) {
            return fail(problem);
        }
        if let Err(err) = self.manager.start() {
            return refused(err, "the service manager could not start the hub");
        }

        let started = Instant::now();
        let mut checked = Instant::now();
        loop {
            if let Ok(Holder::Held(Some(record))) = lock::inspect(data_dir)
                && record.mode == Mode::Serve
                && let Some(url) = &record.url
                && process::ready(&record)
            {
                println!("started (pid {}) at {url}/", record.pid);
                return ExitCode::SUCCESS;
            }
            if checked.elapsed() >= STATE_EVERY
                && let Ok(state) = self.manager.state()
            {
                checked = Instant::now();
                if state.active == Active::Failed
                    || (state.active == Active::Inactive && started.elapsed() > STATE_EVERY)
                {
                    eprintln!(
                        "agent-hub start: the service stopped at once{}",
                        state
                            .detail
                            .as_deref()
                            .map(|detail| format!(" ({detail})"))
                            .unwrap_or_default()
                    );
                    self.print_failure_output();
                    return ExitCode::FAILURE;
                }
            }
            if started.elapsed() >= START_WAIT {
                self.print_failure_output();
                return fail(format!(
                    "the service was started but the hub did not answer /readyz within {}s; \
                     see agent-hub logs",
                    START_WAIT.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    fn stop(&self, settings: &LifecycleSettings, options: &StopOptions) -> ExitCode {
        let data_dir = &settings.data_dir;
        let record = match lock::inspect(data_dir) {
            Ok(Holder::Free) => {
                // A service waiting to restart a hub that has just died holds no
                // lock, and would start one again as soon as the stop returned.
                let state = match self.manager.state() {
                    Ok(state) => state,
                    Err(message) => return unreachable(message),
                };
                if matches!(state.active, Active::Running | Active::Activating) {
                    return self.stop_service(options, None);
                }
                println!("not running");
                return ExitCode::SUCCESS;
            }
            Ok(Holder::Held(Some(record))) if record.mode == Mode::Mcp => {
                return fail(format!(
                    "an embedded agent-hub mcp session (pid {}) holds the store, not a hub; end \
                     that session first. stop acts only on a hub started by agent-hub start or serve",
                    record.pid
                ));
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
        match self.owns(&record) {
            Ok(true) => self.stop_service(options, Some((settings, record))),
            // Not the service's hub: the manager cannot stop it, the pid can.
            Ok(false) => Process.stop(settings, options),
            Err(message) => unreachable(message),
        }
    }

    fn logs(&self, settings: &LifecycleSettings, options: &LogsOptions) -> ExitCode {
        match self.manager.logs(options) {
            Some(code) => code,
            None => Process.logs(settings, options),
        }
    }
}

impl ServiceBackend {
    /// Whether the hub holding the store is the service's own: the manager
    /// runs a main process and it is the one the lock names. The label in the
    /// record is not enough, since a hub started by hand from a process the
    /// manager runs inherits what the label is read from.
    fn owns(&self, record: &Record) -> Result<bool, String> {
        if record.managed_by != self.manager.kind() {
            return Ok(false);
        }
        Ok(self.manager.main_pid()? == Some(record.pid))
    }

    fn print_failure_output(&self) {
        let lines = self.manager.failure_output();
        if lines.is_empty() {
            return;
        }
        eprintln!("the last {} lines the service said:", lines.len());
        for line in lines {
            eprintln!("  {line}");
        }
    }

    /// Ask the manager to stop, then wait for the hub to let go of the store.
    fn stop_service(
        &self,
        options: &StopOptions,
        held: Option<(&LifecycleSettings, Record)>,
    ) -> ExitCode {
        let pid = held.as_ref().map(|(_, record)| record.pid);
        match pid {
            Some(pid) => print!("Stopping the hub (pid {pid})... "),
            None => print!("Stopping the hub... "),
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let began = Instant::now();
        if let Err(err) = self.manager.stop(options.force) {
            println!();
            return refused(err, "the service manager could not stop the hub");
        }
        let settings = match &held {
            Some((settings, _)) => *settings,
            None => {
                // No lock was held: the manager's word is all there is.
                let deadline = began + options.timeout;
                loop {
                    match self.manager.state() {
                        Ok(state) if state.active == Active::Inactive => break,
                        Ok(_) => {}
                        Err(message) => {
                            println!();
                            return unreachable(message);
                        }
                    }
                    if Instant::now() >= deadline {
                        println!();
                        return fail(format!(
                            "the service did not stop within {}s",
                            options.timeout.as_secs()
                        ));
                    }
                    std::thread::sleep(POLL);
                }
                println!("stopped.");
                return ExitCode::SUCCESS;
            }
        };
        let data_dir = &settings.data_dir;
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
                    pid.unwrap_or(0),
                    options.timeout.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
        let seconds = began.elapsed().as_secs_f64();
        let clean = lock::last_record(data_dir)
            .is_some_and(|last| Some(last.pid) == pid && last.clean_stop);
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
}
