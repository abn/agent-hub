//! The systemd user manager: `systemctl --user` and `loginctl`.
//!
//! Nothing here runs `sudo`. Linger is asked for with `loginctl enable-linger`
//! for the caller's own user; when the system refuses, the one command for the
//! human to run is printed and the service starts at login instead.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_hub::config::{Os, present};

use super::render::{self, SYSTEMD_UNIT};
use super::runner::{EnvVar, Outcome, Runner, SystemRunner};
use super::{Active, Job, Manager, Prepared, StartsAt, Unavailable, UnitSpec, UnitState, Want};
use crate::daemon::LogsOptions;

/// The unit's name for `systemctl`.
const UNIT: &str = "agent-hub";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Systemd {
    runner: Box<dyn Runner>,
    config_dir: PathBuf,
    /// The caller's uid: loginctl is given the number, never a name from the
    /// environment, which may name another account or read as an option.
    uid: u32,
    /// `XDG_RUNTIME_DIR` as set for every command, when the caller's
    /// environment does not carry the user's own (a shell reached by `su` or
    /// `sudo -u`, or a user manager that linger had to start).
    runtime_dir: RefCell<Option<String>>,
    /// How long to wait for a user manager that linger just started.
    settle: Duration,
}

impl Systemd {
    pub fn host(env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let home = present(env("HOME")).map(PathBuf::from)?;
        let config_dir = present(env("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let systemd = Self::new(
            Box::new(SystemRunner),
            config_dir,
            super::current_uid(),
            Duration::from_secs(10),
        );
        // With no runtime directory in the environment, every command (install
        // and every later verb alike) looks for the bus where the user's own
        // manager keeps it.
        if present(env("XDG_RUNTIME_DIR")).is_none() {
            *systemd.runtime_dir.borrow_mut() = Some(systemd.own_runtime_dir());
        }
        Some(systemd)
    }

    pub fn new(runner: Box<dyn Runner>, config_dir: PathBuf, uid: u32, settle: Duration) -> Self {
        Self {
            runner,
            config_dir,
            uid,
            runtime_dir: RefCell::new(None),
            settle,
        }
    }

    fn own_runtime_dir(&self) -> String {
        format!("/run/user/{}", self.uid)
    }

    fn env(&self) -> Vec<EnvVar> {
        match self.runtime_dir.borrow().as_deref() {
            Some(dir) => vec![
                ("XDG_RUNTIME_DIR".to_string(), dir.to_string()),
                (
                    "DBUS_SESSION_BUS_ADDRESS".to_string(),
                    format!("unix:path={dir}/bus"),
                ),
            ],
            None => Vec::new(),
        }
    }

    fn systemctl(&self, args: &[&str]) -> std::io::Result<Outcome> {
        let mut full = vec!["--user"];
        full.extend_from_slice(args);
        self.runner
            .run("systemctl", &full, &self.env(), COMMAND_TIMEOUT)
    }

    fn loginctl(&self, args: &[&str]) -> std::io::Result<Outcome> {
        self.runner
            .run("loginctl", args, &self.env(), COMMAND_TIMEOUT)
    }

    /// `systemctl --user` as a step that must work: its complaint on failure.
    fn must(&self, args: &[&str]) -> Result<Outcome, String> {
        match self.systemctl(args) {
            Ok(outcome) if outcome.ok() => Ok(outcome),
            Ok(outcome) => Err(format!(
                "systemctl --user {} failed: {}",
                args.join(" "),
                outcome.complaint()
            )),
            Err(err) => Err(format!("could not run systemctl: {err}")),
        }
    }

    /// [`Self::must`] for a step after the manager was checked: when it fails
    /// and the manager no longer answers, that is 69, not a refusal.
    fn must_or_unreachable(&self, args: &[&str]) -> Result<Outcome, Unavailable> {
        self.must(args)
            .map_err(|message| Unavailable::after_check(self, message))
    }

    /// Whether the user manager answers.
    fn reachable(&self) -> Result<bool, Unavailable> {
        match self.systemctl(&["show-environment"]) {
            Ok(outcome) => Ok(outcome.ok()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Unavailable::none(
                "systemctl was not found: this system does not run systemd, so there is no \
                 user service manager. Run the hub with: agent-hub start",
            )),
            Err(err) => Err(Unavailable::none(format!("could not run systemctl: {err}"))),
        }
    }

    /// Ask again at the user's own runtime directory, when the commands were
    /// not already pointed there. Nothing about linger changes.
    fn reachable_at_own_runtime_dir(&self) -> Result<bool, Unavailable> {
        let own = self.own_runtime_dir();
        if self.runtime_dir.borrow().as_deref() == Some(own.as_str()) {
            return Ok(false);
        }
        let before = self.runtime_dir.replace(Some(own));
        if self.reachable()? {
            return Ok(true);
        }
        *self.runtime_dir.borrow_mut() = before;
        Ok(false)
    }

    /// Whether the user's linger is on: `loginctl show-user -p Linger -- UID`.
    fn linger(&self) -> bool {
        let uid = self.uid.to_string();
        self.loginctl(&["show-user", "-p", "Linger", "--value", "--", &uid])
            .is_ok_and(|outcome| outcome.ok() && outcome.stdout.trim() == "yes")
    }

    /// Try to turn linger on, as the user and without privilege.
    fn enable_linger(&self) -> Result<(), String> {
        let uid = self.uid.to_string();
        match self.loginctl(&["enable-linger", "--", &uid]) {
            Ok(outcome) if outcome.ok() => Ok(()),
            Ok(outcome) => Err(outcome.complaint()),
            Err(err) => Err(err.to_string()),
        }
    }

    /// The one command for the human. It names the numeric uid, so nothing
    /// from the environment is pasted into a line run with sudo.
    fn sudo_command(&self) -> String {
        format!("sudo loginctl enable-linger {}", self.uid)
    }

    /// The user manager is not running and linger would start it. Turn linger
    /// on, point commands at the user's runtime directory, and wait for it.
    fn start_user_manager(&self) -> Result<(), Unavailable> {
        if !self.linger()
            && let Err(complaint) = self.enable_linger()
        {
            return Err(Unavailable::none(format!(
                "no systemd user manager is running for uid {uid} and linger could not be \
                 turned on ({complaint}).\nTo start it at boot with no login, run once:\n  \
                 {sudo}\nUntil then, run the hub with: agent-hub start",
                uid = self.uid,
                sudo = self.sudo_command()
            )));
        }
        // A shell reached by su or sudo -u carries no runtime directory, and
        // systemctl then reports a bare bus error instead of the cause.
        *self.runtime_dir.borrow_mut() = Some(self.own_runtime_dir());
        let began = Instant::now();
        loop {
            if self.reachable()? {
                return Ok(());
            }
            if began.elapsed() >= self.settle {
                return Err(Unavailable::none(format!(
                    "linger is on for uid {uid}, but the user manager did not answer at \
                     /run/user/{uid} within {secs}s. Log in once, or check \
                     systemctl status user@{uid}. Until then, run the hub with: agent-hub start",
                    uid = self.uid,
                    secs = self.settle.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// `systemctl --user show` for the unit, or why it could not be asked.
    fn show(&self, properties: &str) -> Result<String, String> {
        match self.systemctl(&["show", "-p", properties, UNIT]) {
            Ok(outcome) if outcome.ok() => Ok(outcome.stdout),
            Ok(outcome) => Err(format!(
                "the systemd user manager did not answer: {}",
                outcome.complaint()
            )),
            Err(err) => Err(format!("could not run systemctl: {err}")),
        }
    }
}

/// `key=value` lines from `systemctl show`.
fn property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

impl Manager for Systemd {
    fn kind(&self) -> &'static str {
        "systemd"
    }

    fn os(&self) -> Os {
        Os::Linux
    }

    fn unit_path(&self) -> PathBuf {
        self.config_dir
            .join("systemd")
            .join("user")
            .join(SYSTEMD_UNIT)
    }

    fn render(&self, spec: &UnitSpec) -> Result<String, String> {
        render::systemd_unit(spec)
    }

    fn prepare(&self, want: Want) -> Result<Prepared, Unavailable> {
        let reached = self.reachable()? || self.reachable_at_own_runtime_dir()?;
        if want == Want::Login {
            // --login leaves linger exactly as it is: not read, not changed.
            if !reached {
                return Err(Unavailable::none(format!(
                    "no systemd user manager answers for uid {uid}, and --login leaves linger \
                     alone. Log in to a session that starts it, or drop --login to let install \
                     turn linger on. Until then, run the hub with: agent-hub start",
                    uid = self.uid
                )));
            }
            return Ok(Prepared {
                starts_at: StartsAt::Login,
                row: "login".to_string(),
                advice: None,
            });
        }
        if !reached {
            // Starting the manager takes linger, which is then on.
            self.start_user_manager()?;
        } else if !self.linger()
            && let Err(complaint) = self.enable_linger()
        {
            return Ok(Prepared {
                starts_at: StartsAt::Login,
                row: format!(
                    "login (linger is off: the hub stops when you log out; \
                     could not turn it on: {complaint})"
                ),
                advice: Some(format!(
                    "To start it at boot with no login, run once:\n  {}",
                    self.sudo_command()
                )),
            });
        }
        Ok(Prepared {
            starts_at: StartsAt::Boot,
            row: "boot (linger on)".to_string(),
            advice: None,
        })
    }

    fn check(&self) -> Result<(), Unavailable> {
        if self.reachable()? || self.reachable_at_own_runtime_dir()? {
            return Ok(());
        }
        Err(Unavailable::none(format!(
            "the systemd user manager for uid {} does not answer, so the service cannot be \
             stopped or removed safely; log in to a session that starts it and try again",
            self.uid
        )))
    }

    fn register(
        &self,
        _spec: &UnitSpec,
        _changed: bool,
        _start: bool,
    ) -> Result<Option<String>, Unavailable> {
        // `enable` links the unit into default.target; it never starts it.
        self.must_or_unreachable(&["daemon-reload"])?;
        self.must_or_unreachable(&["enable", UNIT])?;
        Ok(None)
    }

    fn start(&self) -> Result<(), Unavailable> {
        self.must_or_unreachable(&["start", UNIT]).map(|_| ())
    }

    fn stop(&self, force: bool) -> Result<(), Unavailable> {
        if force {
            // No drain and no checkpoint: kill the whole unit, then let the
            // manager settle its state.
            self.must_or_unreachable(&["kill", "--signal=SIGKILL", UNIT])?;
        }
        self.must_or_unreachable(&["stop", "--no-block", UNIT])
            .map(|_| ())
    }

    fn job(&self) -> Result<Job, Unavailable> {
        // No systemctl on this machine: there is no manager to hold a job.
        if let Err(err) = self.systemctl(&["show-environment"])
            && err.kind() == std::io::ErrorKind::NotFound
        {
            return Ok(Job::Absent);
        }
        self.check()?;
        let shown = self
            .show("LoadState,ActiveState,Environment")
            .map_err(Unavailable::none)?;
        let get = |key: &str| property(&shown, key).unwrap_or("");
        if get("LoadState") == "not-found" && get("ActiveState") == "inactive" {
            return Ok(Job::Absent);
        }
        Ok(Job::Loaded {
            config: render::systemd_shown_config(get("Environment")),
        })
    }

    fn job_name(&self) -> String {
        SYSTEMD_UNIT.to_string()
    }

    fn unload_hint(&self) -> String {
        format!("systemctl --user disable --now {UNIT} && systemctl --user daemon-reload")
    }

    fn state(&self) -> Result<UnitState, String> {
        let shown =
            self.show("ActiveState,SubState,MainPID,ExecMainStatus,UnitFileState,Result")?;
        let get = |key: &str| property(&shown, key).unwrap_or("");
        let active = match get("ActiveState") {
            "active" => Active::Running,
            "activating" | "reloading" => Active::Activating,
            "failed" => Active::Failed,
            _ => Active::Inactive,
        };
        let enabled = get("UnitFileState") == "enabled";
        let starts_at = if !enabled {
            "manual"
        } else if self.linger() {
            "boot"
        } else {
            "login"
        };
        let detail = (active == Active::Failed).then(|| {
            let status = get("ExecMainStatus");
            let result = get("Result");
            if status == "78" {
                "configuration error".to_string()
            } else if !status.is_empty() && status != "0" {
                format!("exit {status}")
            } else if !result.is_empty() {
                result.to_string()
            } else {
                "see agent-hub logs".to_string()
            }
        });
        Ok(UnitState {
            active,
            enabled,
            starts_at,
            detail,
        })
    }

    fn main_pid(&self) -> Result<Option<u32>, String> {
        let shown = self.show("MainPID")?;
        Ok(property(&shown, "MainPID")
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            .filter(|pid| *pid != 0))
    }

    fn remove(&self) -> Result<(), Unavailable> {
        self.check()?;
        // A unit the manager never loaded has nothing to stop or disable, and
        // says so; anything else must stop and disable before its file goes.
        let loaded = self
            .show("LoadState")
            .map_err(Unavailable::none)
            .map(|shown| property(&shown, "LoadState") != Some("not-found"))?;
        if loaded {
            self.must_or_unreachable(&["stop", UNIT])?;
            self.must_or_unreachable(&["disable", UNIT])?;
        }
        match std::fs::remove_file(self.unit_path()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(Unavailable::failed(format!(
                    "could not remove {}: {err}",
                    self.unit_path().display()
                )));
            }
        }
        self.must_or_unreachable(&["daemon-reload"])?;
        let _ = self.systemctl(&["reset-failed", UNIT]);
        Ok(())
    }

    fn logs(&self, options: &LogsOptions) -> Option<ExitCode> {
        let lines = options.lines.to_string();
        let mut args = vec!["--user", "-u", UNIT, "-n", &lines, "--no-pager"];
        if options.follow {
            args.push("-f");
        }
        Some(
            match self.runner.passthrough("journalctl", &args, &self.env()) {
                Ok(Some(0)) | Ok(None) => ExitCode::SUCCESS,
                Ok(Some(_)) => ExitCode::FAILURE,
                Err(err) => {
                    eprintln!("agent-hub logs: could not run journalctl: {err}");
                    ExitCode::FAILURE
                }
            },
        )
    }

    fn failure_output(&self) -> Vec<String> {
        let Ok(outcome) = self.runner.run(
            "journalctl",
            &["--user", "-u", UNIT, "-n", "20", "--no-pager", "-o", "cat"],
            &self.env(),
            COMMAND_TIMEOUT,
        ) else {
            return Vec::new();
        };
        outcome.stdout.lines().map(String::from).collect()
    }

    fn log_hint(&self) -> String {
        "agent-hub logs  (journalctl --user -u agent-hub)".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::runner::recording::Recorder;

    fn systemd(recorder: &Recorder) -> Systemd {
        Systemd::new(
            Box::new(recorder.clone()),
            PathBuf::from("/nowhere/config"),
            1000,
            Duration::ZERO,
        )
    }

    fn calls(recorder: &Recorder) -> Vec<String> {
        recorder.calls()
    }

    fn no_loginctl(recorder: &Recorder) {
        let all = calls(recorder);
        assert!(
            all.iter().all(|call| !call.starts_with("loginctl")),
            "loginctl was run: {all:?}"
        );
    }

    #[test]
    fn the_unit_lives_in_the_xdg_user_directory() {
        let recorder = Recorder::default();
        assert_eq!(
            systemd(&recorder).unit_path(),
            PathBuf::from("/nowhere/config/systemd/user/agent-hub.service")
        );
    }

    #[test]
    fn linger_already_on_means_boot_and_no_change() {
        let recorder = Recorder::default();
        recorder.exit("loginctl show-user", 0, "yes\n", "");
        let prepared = systemd(&recorder).prepare(Want::Default).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Boot);
        assert!(prepared.advice.is_none());
        assert_eq!(
            calls(&recorder),
            vec![
                "systemctl --user show-environment",
                "loginctl show-user -p Linger --value -- 1000",
            ]
        );
    }

    #[test]
    fn linger_is_turned_on_by_uid_without_sudo_when_the_system_allows_it() {
        let recorder = Recorder::default();
        recorder.exit("loginctl show-user", 0, "no\n", "");
        let prepared = systemd(&recorder).prepare(Want::Boot).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Boot);
        let all = calls(&recorder);
        assert!(
            all.contains(&"loginctl enable-linger -- 1000".to_string()),
            "{all:?}"
        );
        assert!(
            all.iter().all(|call| !call.contains("sudo")),
            "no command runs through sudo: {all:?}"
        );
    }

    #[test]
    fn a_refused_linger_falls_back_to_login_and_prints_the_one_command() {
        let recorder = Recorder::default();
        recorder.exit("loginctl show-user", 0, "no\n", "");
        recorder.exit(
            "loginctl enable-linger",
            1,
            "",
            "Interactive authentication required.",
        );
        let prepared = systemd(&recorder).prepare(Want::Default).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Login);
        assert!(
            prepared.row.starts_with("login (linger is off"),
            "{}",
            prepared.row
        );
        let advice = prepared.advice.expect("the command for the human");
        assert!(
            advice.contains("sudo loginctl enable-linger 1000"),
            "{advice}"
        );
    }

    #[test]
    fn login_never_runs_loginctl() {
        let recorder = Recorder::default();
        recorder.exit("loginctl show-user", 0, "no\n", "");
        let prepared = systemd(&recorder).prepare(Want::Login).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Login);
        assert_eq!(calls(&recorder), vec!["systemctl --user show-environment"]);
    }

    #[test]
    fn login_with_no_user_manager_is_69_and_never_runs_loginctl() {
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        let err = systemd(&recorder)
            .prepare(Want::Login)
            .expect_err("unavailable");
        assert_eq!(err.code, 69);
        assert!(
            err.message.contains("--login leaves linger"),
            "{}",
            err.message
        );
        no_loginctl(&recorder);
    }

    #[test]
    fn the_users_own_runtime_bus_is_tried_before_linger_is_touched() {
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        recorder.exit("systemctl --user show-environment", 0, "", "");
        recorder.exit("loginctl show-user", 0, "yes\n", "");
        let prepared = systemd(&recorder).prepare(Want::Default).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Boot);
        let all = calls(&recorder);
        assert_eq!(
            all,
            vec![
                "systemctl --user show-environment",
                "systemctl --user show-environment",
                "loginctl show-user -p Linger --value -- 1000",
            ]
        );
        assert!(
            recorder
                .env_of(1)
                .contains(&("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string())),
            "the second probe is at the user's runtime directory"
        );
    }

    #[test]
    fn no_user_manager_and_linger_off_turns_linger_on_and_retries() {
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        recorder.exit("systemctl --user show-environment", 0, "", "");
        recorder.exit("loginctl show-user", 0, "no\n", "");
        let manager = systemd(&recorder);
        let prepared = manager.prepare(Want::Default).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Boot);
        let all = calls(&recorder);
        let enable = all
            .iter()
            .position(|call| call == "loginctl enable-linger -- 1000")
            .expect("linger was enabled");
        let retry = all
            .iter()
            .rposition(|call| call == "systemctl --user show-environment")
            .expect("the manager was asked again");
        assert!(retry > enable, "{all:?}");
        let env = recorder.env_of(retry);
        assert!(
            env.contains(&("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string())),
            "the retry points at the user's runtime directory: {env:?}"
        );
    }

    #[test]
    fn no_user_manager_and_linger_refused_is_exit_69_with_the_sudo_command() {
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        recorder.exit("loginctl show-user", 0, "no\n", "");
        recorder.exit("loginctl enable-linger", 1, "", "Access denied");
        let err = systemd(&recorder)
            .prepare(Want::Default)
            .expect_err("unavailable");
        assert_eq!(err.code, 69);
        assert!(
            err.message.contains("sudo loginctl enable-linger 1000"),
            "{}",
            err.message
        );
        assert!(err.message.contains("agent-hub start"), "{}", err.message);
    }

    #[test]
    fn a_system_without_systemctl_is_exit_69_and_never_reaches_loginctl() {
        let recorder = Recorder::default();
        recorder.missing("systemctl");
        let err = systemd(&recorder)
            .prepare(Want::Default)
            .expect_err("unavailable");
        assert_eq!(err.code, 69);
        assert_eq!(calls(&recorder), vec!["systemctl --user show-environment"]);
    }

    #[test]
    fn registering_reloads_then_enables() {
        let recorder = Recorder::default();
        let spec = UnitSpec {
            binary: PathBuf::from("/b"),
            config: PathBuf::from("/c"),
            home: PathBuf::from("/h"),
            starts_at: StartsAt::Boot,
        };
        systemd(&recorder)
            .register(&spec, true, false)
            .expect("register");
        assert_eq!(
            calls(&recorder),
            vec![
                "systemctl --user daemon-reload",
                "systemctl --user enable agent-hub"
            ]
        );
    }

    #[test]
    fn a_normal_stop_does_not_wait_and_a_forced_one_kills_first() {
        let recorder = Recorder::default();
        systemd(&recorder).stop(false).expect("stop");
        assert_eq!(
            calls(&recorder),
            vec!["systemctl --user stop --no-block agent-hub"]
        );
        let forced = Recorder::default();
        systemd(&forced).stop(true).expect("stop");
        assert_eq!(
            calls(&forced),
            vec![
                "systemctl --user kill --signal=SIGKILL agent-hub",
                "systemctl --user stop --no-block agent-hub"
            ]
        );
    }

    #[test]
    fn the_state_reads_what_systemctl_shows() {
        let recorder = Recorder::default();
        recorder.exit(
            "systemctl --user show",
            0,
            "ActiveState=active\nSubState=running\nMainPID=4121\nUnitFileState=enabled\nResult=success\nExecMainStatus=0\n",
            "",
        );
        recorder.exit("loginctl show-user", 0, "yes\n", "");
        let state = systemd(&recorder).state().expect("state");
        assert_eq!(state.active, Active::Running);
        assert!(state.enabled);
        assert_eq!(state.starts_at, "boot");

        let failed = Recorder::default();
        failed.exit(
            "systemctl --user show",
            0,
            "ActiveState=failed\nSubState=failed\nMainPID=0\nUnitFileState=enabled\nResult=exit-code\nExecMainStatus=78\n",
            "",
        );
        failed.exit("loginctl show-user", 0, "no\n", "");
        let state = systemd(&failed).state().expect("state");
        assert_eq!(state.active, Active::Failed);
        assert_eq!(state.detail.as_deref(), Some("configuration error"));
        assert_eq!(state.starts_at, "login");
    }

    #[test]
    fn a_manager_that_cannot_be_asked_is_not_a_stopped_service() {
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show", 1, "", "Failed to connect to bus");
        let manager = systemd(&recorder);
        assert!(manager.state().is_err());
        assert!(manager.main_pid().is_err());
    }

    #[test]
    fn the_main_pid_is_what_systemctl_shows_and_zero_is_none() {
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show -p MainPID", 0, "MainPID=4121\n", "");
        recorder.exit("systemctl --user show -p MainPID", 0, "MainPID=0\n", "");
        let manager = systemd(&recorder);
        assert_eq!(manager.main_pid(), Ok(Some(4121)));
        assert_eq!(manager.main_pid(), Ok(None));
    }

    fn seeded_unit(tag: &str) -> PathBuf {
        let dir = PathBuf::from("target/tmp").join(format!("systemd-{tag}-{}", std::process::id()));
        let unit = dir.join("systemd").join("user");
        std::fs::create_dir_all(&unit).expect("create");
        std::fs::write(unit.join("agent-hub.service"), "[Unit]\n").expect("seed");
        dir
    }

    fn manager_at(recorder: &Recorder, dir: &std::path::Path) -> Systemd {
        Systemd::new(
            Box::new(recorder.clone()),
            dir.to_path_buf(),
            1000,
            Duration::ZERO,
        )
    }

    #[test]
    fn removing_stops_disables_deletes_the_file_and_reloads_but_leaves_linger() {
        let dir = seeded_unit("remove");
        let recorder = Recorder::default();
        recorder.exit(
            "systemctl --user show -p LoadState",
            0,
            "LoadState=loaded\n",
            "",
        );
        let manager = manager_at(&recorder, &dir);
        manager.remove().expect("remove");
        assert!(!manager.unit_path().exists());
        let all = calls(&recorder);
        assert_eq!(
            all,
            vec![
                "systemctl --user show-environment",
                "systemctl --user show -p LoadState agent-hub",
                "systemctl --user stop agent-hub",
                "systemctl --user disable agent-hub",
                "systemctl --user daemon-reload",
                "systemctl --user reset-failed agent-hub",
            ]
        );
        no_loginctl(&recorder);
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn an_unreachable_manager_is_69_and_the_unit_stays() {
        let dir = seeded_unit("unreachable");
        let recorder = Recorder::default();
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        let manager = manager_at(&recorder, &dir);
        let err = manager.remove().expect_err("refused");
        assert_eq!(err.code, 69);
        assert!(manager.unit_path().exists(), "the definition stays");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_stop_that_fails_leaves_the_unit_in_place() {
        let dir = seeded_unit("stopfail");
        let recorder = Recorder::default();
        recorder.exit(
            "systemctl --user show -p LoadState",
            0,
            "LoadState=loaded\n",
            "",
        );
        recorder.exit("systemctl --user stop", 1, "", "Job failed");
        let manager = manager_at(&recorder, &dir);
        let err = manager.remove().expect_err("refused");
        assert_eq!(err.code, 1);
        assert!(manager.unit_path().exists(), "the definition stays");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_unit_the_manager_never_loaded_is_only_deleted() {
        let dir = seeded_unit("notfound");
        let recorder = Recorder::default();
        recorder.exit(
            "systemctl --user show -p LoadState",
            0,
            "LoadState=not-found\n",
            "",
        );
        let manager = manager_at(&recorder, &dir);
        manager.remove().expect("remove");
        assert!(!manager.unit_path().exists());
        assert!(
            !calls(&recorder).iter().any(|call| call.contains(" stop ")),
            "{:?}",
            calls(&recorder)
        );
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_step_that_fails_with_the_bus_gone_is_69_and_with_the_bus_up_is_1() {
        for (verb, call) in [
            ("start", "systemctl --user start agent-hub"),
            ("stop", "systemctl --user stop --no-block agent-hub"),
            ("register", "systemctl --user daemon-reload"),
        ] {
            let gone = Recorder::default();
            gone.exit(call, 1, "", "Failed to connect to bus");
            gone.exit("systemctl --user show-environment", 1, "", "no bus");
            let manager = systemd(&gone);
            let err = match verb {
                "start" => manager.start().expect_err("failed"),
                "stop" => manager.stop(false).expect_err("failed"),
                _ => manager
                    .register(
                        &UnitSpec {
                            binary: PathBuf::from("/b"),
                            config: PathBuf::from("/c"),
                            home: PathBuf::from("/h"),
                            starts_at: StartsAt::Boot,
                        },
                        true,
                        false,
                    )
                    .expect_err("failed"),
            };
            assert_eq!(err.code, 69, "{verb}: {}", err.message);

            let up = Recorder::default();
            up.exit(call, 1, "", "Job failed");
            let manager = systemd(&up);
            let err = match verb {
                "start" => manager.start().expect_err("failed"),
                "stop" => manager.stop(false).expect_err("failed"),
                _ => manager
                    .register(
                        &UnitSpec {
                            binary: PathBuf::from("/b"),
                            config: PathBuf::from("/c"),
                            home: PathBuf::from("/h"),
                            starts_at: StartsAt::Boot,
                        },
                        true,
                        false,
                    )
                    .expect_err("failed"),
            };
            assert_eq!(err.code, 1, "{verb}: {}", err.message);
        }
    }

    #[test]
    fn a_remove_step_that_fails_with_the_bus_gone_is_69_and_the_unit_stays() {
        let dir = seeded_unit("busgone");
        let recorder = Recorder::default();
        recorder.exit(
            "systemctl --user show -p LoadState",
            0,
            "LoadState=loaded\n",
            "",
        );
        recorder.exit("systemctl --user stop", 1, "", "Failed to connect to bus");
        recorder.exit("systemctl --user show-environment", 0, "", "");
        recorder.exit("systemctl --user show-environment", 1, "", "no bus");
        let manager = manager_at(&recorder, &dir);
        let err = manager.remove().expect_err("refused");
        assert_eq!(err.code, 69, "{}", err.message);
        assert!(manager.unit_path().exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_manager_is_asked_whether_the_job_is_loaded_and_what_config_it_runs() {
        let recorder = Recorder::default();
        recorder.exit(
            "systemctl --user show -p LoadState,ActiveState,Environment",
            0,
            "LoadState=loaded\nActiveState=active\nEnvironment=AGENT_HUB_MANAGED_BY=systemd \"HUB_CONFIG=/a dir/config.toml\"\n",
            "",
        );
        assert_eq!(
            systemd(&recorder).job().expect("job"),
            Job::Loaded {
                config: Some(PathBuf::from("/a dir/config.toml"))
            }
        );

        let gone = Recorder::default();
        gone.exit(
            "systemctl --user show -p LoadState,ActiveState,Environment",
            0,
            "LoadState=not-found\nActiveState=inactive\nEnvironment=\n",
            "",
        );
        assert_eq!(systemd(&gone).job().expect("job"), Job::Absent);

        // A unit whose file is gone but is still running is still loaded.
        let running = Recorder::default();
        running.exit(
            "systemctl --user show -p LoadState,ActiveState,Environment",
            0,
            "LoadState=not-found\nActiveState=active\nEnvironment=\n",
            "",
        );
        assert_eq!(
            systemd(&running).job().expect("job"),
            Job::Loaded { config: None }
        );
    }

    #[test]
    fn a_job_cannot_be_asked_about_without_the_bus_but_a_machine_with_no_systemctl_has_none() {
        let down = Recorder::default();
        down.exit("systemctl --user show-environment", 1, "", "no bus");
        assert_eq!(systemd(&down).job().expect_err("69").code, 69);

        let none = Recorder::default();
        none.missing("systemctl");
        assert_eq!(systemd(&none).job().expect("absent"), Job::Absent);
    }
}
