//! A service manager that keeps its state in a directory.
//!
//! It exists so the built binary can be driven through install, upgrade, stop
//! and uninstall without a real service manager anywhere near it. Every
//! operation appends a line to `calls.log`; the unit it renders is written to
//! `<dir>/unit`; and `start` runs a real `agent-hub serve` detached, so the
//! lifecycle verbs have a real hub to act on. Selected by
//! `AGENT_HUB_SERVICE_FAKE=<dir>` (see [`super::host_manager`]).
//!
//! Marker files steer it: `unavailable` (its text is the reason) makes it a
//! manager that does not answer, so `prepare`, `check` and `remove` answer 69
//! and `state` cannot be read; `hub_config` (the config the loaded job runs
//! with) says the job is loaded, whatever the definition file says; `linger-refused` makes a boot request fall back
//! to login the way a refused `loginctl enable-linger` does.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use agent_hub::config::Os;

use super::render;
use super::{Active, Job, Manager, Prepared, StartsAt, Unavailable, UnitSpec, UnitState, Want};
use crate::daemon::LogsOptions;

pub struct Fake {
    dir: PathBuf,
    launchd: bool,
}

impl Fake {
    pub fn new(dir: PathBuf, launchd: bool) -> Self {
        Self { dir, launchd }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn log(&self, line: &str) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file("calls.log"))
        {
            let _ = writeln!(file, "{line}");
        }
    }

    fn pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.file("pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn alive(&self) -> bool {
        self.pid().is_some_and(alive)
    }

    /// The reason it does not answer, when the `unavailable` marker is there.
    fn unavailable(&self) -> Option<String> {
        std::fs::read_to_string(self.file("unavailable"))
            .ok()
            .map(|reason| reason.trim().to_string())
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    use rustix::process::{Pid, test_kill_process};
    i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .is_some_and(|pid| test_kill_process(pid).is_ok())
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn signal(pid: u32, kill: bool) {
    use rustix::process::{Pid, Signal, kill_process};
    if let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process(pid, if kill { Signal::KILL } else { Signal::TERM });
    }
}

#[cfg(not(unix))]
fn signal(_pid: u32, _kill: bool) {}

impl Manager for Fake {
    fn kind(&self) -> &'static str {
        if self.launchd { "launchd" } else { "systemd" }
    }

    fn os(&self) -> Os {
        if self.launchd { Os::Macos } else { Os::Linux }
    }

    fn unit_path(&self) -> PathBuf {
        self.file("unit")
    }

    fn render(&self, spec: &UnitSpec) -> Result<String, String> {
        if self.launchd {
            render::launchd_plist(spec, &self.file("launchd.log"))
        } else {
            render::systemd_unit(spec)
        }
    }

    fn prepare(&self, want: Want) -> Result<Prepared, Unavailable> {
        self.log(&format!("prepare {want:?}"));
        if let Some(reason) = self.unavailable() {
            return Err(Unavailable::none(reason));
        }
        if self.launchd && want == Want::Boot {
            return Err(Unavailable {
                code: 2,
                message: "--boot is not possible on macOS".to_string(),
            });
        }
        let refused = self.file("linger-refused").exists();
        if self.launchd || want == Want::Login || refused {
            let advice = (!self.launchd && refused && want != Want::Login).then(|| {
                "To start it at boot with no login, run once:\n  sudo loginctl enable-linger fake"
                    .to_string()
            });
            return Ok(Prepared {
                starts_at: StartsAt::Login,
                row: if self.launchd {
                    "login".to_string()
                } else {
                    "login (linger is off: the hub stops when you log out)".to_string()
                },
                advice,
            });
        }
        Ok(Prepared {
            starts_at: StartsAt::Boot,
            row: "boot (linger on)".to_string(),
            advice: None,
        })
    }

    fn check(&self) -> Result<(), Unavailable> {
        match self.unavailable() {
            Some(reason) => Err(Unavailable::none(reason)),
            None => Ok(()),
        }
    }

    fn register(
        &self,
        spec: &UnitSpec,
        changed: bool,
        start: bool,
    ) -> Result<Option<String>, Unavailable> {
        self.log(&format!("register changed={changed}"));
        if let Some(reason) = self.unavailable() {
            return Err(Unavailable::none(reason));
        }
        let failed = |err: std::io::Error| Unavailable::failed(err.to_string());
        std::fs::write(self.file("enabled"), spec.starts_at.word()).map_err(failed)?;
        let loaded = self.file("hub_config").exists();
        if self.launchd && !start {
            // As launchd does: a no-start install loads nothing, and a loaded
            // job whose definition changed is unloaded, so disk and running
            // state agree.
            if changed && loaded {
                if let Some(pid) = self.pid() {
                    signal(pid, false);
                }
                for name in ["hub_config", "pid"] {
                    let _ = std::fs::remove_file(self.file(name));
                }
                return Ok(Some(super::UNLOADED_NOTE.to_string()));
            }
            return Ok(None);
        }
        std::fs::write(self.file("hub_config"), spec.config.display().to_string())
            .map_err(failed)?;
        Ok(None)
    }

    fn start(&self) -> Result<(), Unavailable> {
        self.log("start");
        if let Some(reason) = self.unavailable() {
            return Err(Unavailable::none(reason));
        }
        if self.alive() {
            return Ok(());
        }
        let failed = |err: String| Unavailable::failed(err);
        let config = self
            .pinned_config()
            .map_err(|err| failed(format!("nothing was registered: {err}")))?;
        std::fs::write(self.file("hub_config"), config.display().to_string())
            .map_err(|err| failed(err.to_string()))?;
        let exe = std::env::current_exe().map_err(|err| failed(err.to_string()))?;
        let out =
            std::fs::File::create(self.file("serve.out")).map_err(|err| failed(err.to_string()))?;
        let err_out = out.try_clone().map_err(|err| failed(err.to_string()))?;
        let mut command = Command::new(exe);
        command
            .arg("serve")
            .env("HUB_CONFIG", &config)
            .env("AGENT_HUB_MANAGED_BY", self.kind())
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err_out);
        // As the unit and the plist arrange: no setting variable reaches it.
        for key in agent_hub::config::HUB_KEYS {
            command.env_remove(format!("HUB_{}", key.to_ascii_uppercase()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and touches no parent memory.
            unsafe {
                command.pre_exec(|| {
                    rustix::process::setsid()
                        .map(|_| ())
                        .map_err(std::io::Error::from)
                });
            }
        }
        let child = command.spawn().map_err(|err| failed(err.to_string()))?;
        std::fs::write(self.file("pid"), child.id().to_string())
            .map_err(|err| failed(err.to_string()))
    }

    fn stop(&self, force: bool) -> Result<(), Unavailable> {
        self.log(&format!("stop force={force}"));
        if let Some(reason) = self.unavailable() {
            return Err(Unavailable::none(reason));
        }
        if let Some(pid) = self.pid() {
            signal(pid, force);
        }
        Ok(())
    }

    fn job(&self) -> Result<Job, Unavailable> {
        self.check()?;
        Ok(match std::fs::read_to_string(self.file("hub_config")) {
            Ok(config) => Job::Loaded {
                config: Some(PathBuf::from(config.trim())),
            },
            Err(_) => Job::Absent,
        })
    }

    fn job_name(&self) -> String {
        "agent-hub (fake)".to_string()
    }

    fn unload_hint(&self) -> String {
        format!("rm {}", self.file("hub_config").display())
    }

    fn state(&self) -> Result<UnitState, String> {
        if let Some(reason) = self.unavailable() {
            return Err(reason);
        }
        let enabled = std::fs::read_to_string(self.file("enabled")).ok();
        Ok(UnitState {
            active: if self.alive() {
                Active::Running
            } else {
                Active::Inactive
            },
            enabled: enabled.is_some(),
            starts_at: match enabled.as_deref() {
                Some("boot") => "boot",
                Some(_) => "login",
                None => "manual",
            },
            detail: None,
        })
    }

    fn main_pid(&self) -> Result<Option<u32>, String> {
        if let Some(reason) = self.unavailable() {
            return Err(reason);
        }
        Ok(self.pid().filter(|pid| alive(*pid)))
    }

    fn remove(&self) -> Result<(), Unavailable> {
        self.check()?;
        self.log("remove");
        if let Some(pid) = self.pid() {
            signal(pid, false);
        }
        for name in ["unit", "enabled", "hub_config", "pid"] {
            let _ = std::fs::remove_file(self.file(name));
        }
        Ok(())
    }

    fn logs(&self, options: &LogsOptions) -> Option<ExitCode> {
        // The fake's own log: what the hub it started printed.
        for line in
            crate::daemon::logfile::last_lines(&self.file("serve.out"), options.lines).ok()?
        {
            println!("{line}");
        }
        Some(ExitCode::SUCCESS)
    }

    fn failure_output(&self) -> Vec<String> {
        crate::daemon::logfile::last_lines(&self.file("serve.out"), 20).unwrap_or_default()
    }

    fn log_hint(&self) -> String {
        "agent-hub logs  (the fake manager's log)".to_string()
    }

    fn manager_logs(&self) -> Vec<PathBuf> {
        std::fs::read_to_string(self.unit_path())
            .ok()
            .and_then(|text| render::launchd_log_path(&text))
            .into_iter()
            .collect()
    }
}
