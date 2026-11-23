//! The launchd manager: a LaunchAgent in the user's `gui/<uid>` domain.
//!
//! A LaunchAgent starts at login. There is no way to start one at boot without
//! a login, so `--boot` is refused here with the two ways the human can get
//! there. When nobody is logged in at the console (an SSH session to a Mac with
//! no one at the screen) the agent goes into the `user/<uid>` domain, which
//! lasts until reboot.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent_hub::config::{Os, present};

use super::render::{self, LAUNCHD_LABEL};
use super::runner::{Outcome, Runner, SystemRunner};
use super::{Active, Job, Manager, Prepared, StartsAt, Unavailable, UnitSpec, UnitState, Want};
use crate::daemon::LogsOptions;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// `bootout` waits out the agent's `ExitTimeOut` of 20 seconds.
const BOOTOUT_TIMEOUT: Duration = Duration::from_secs(45);

pub struct Launchd {
    runner: Box<dyn Runner>,
    home: PathBuf,
    uid: u32,
    domain: RefCell<Option<String>>,
}

impl Launchd {
    pub fn host(env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let home = present(env("HOME")).map(PathBuf::from)?;
        Some(Self::new(
            Box::new(SystemRunner),
            home,
            super::current_uid(),
        ))
    }

    pub fn new(runner: Box<dyn Runner>, home: PathBuf, uid: u32) -> Self {
        Self {
            runner,
            home,
            uid,
            domain: RefCell::new(None),
        }
    }

    fn launchctl(&self, args: &[&str]) -> std::io::Result<Outcome> {
        self.runner.run("launchctl", args, &[], COMMAND_TIMEOUT)
    }

    fn must(&self, args: &[&str]) -> Result<Outcome, String> {
        match self.launchctl(args) {
            Ok(outcome) if outcome.ok() => Ok(outcome),
            Ok(outcome) => Err(format!(
                "launchctl {} failed: {}",
                args.join(" "),
                outcome.complaint()
            )),
            Err(err) => Err(format!("could not run launchctl: {err}")),
        }
    }

    /// The domain the agent lives in, settled once.
    fn domain(&self) -> String {
        if let Some(domain) = self.domain.borrow().clone() {
            return domain;
        }
        let gui = format!("gui/{}", self.uid);
        let domain = match self.launchctl(&["print", &gui]) {
            Ok(outcome) if outcome.ok() => gui,
            _ => format!("user/{}", self.uid),
        };
        *self.domain.borrow_mut() = Some(domain.clone());
        domain
    }

    fn target(&self) -> String {
        format!("{}/{LAUNCHD_LABEL}", self.domain())
    }

    fn launchd_log(&self) -> PathBuf {
        self.home
            .join("Library")
            .join("Logs")
            .join("agent-hub")
            .join("launchd.log")
    }

    /// The output of `launchctl print` for the agent: `Some` when it is
    /// loaded, `None` when launchd says it has no such job (exit 113, "Could
    /// not find service") and still answers for the domain, and `Err` for
    /// every other failure: launchd cannot be asked, which is not the same as
    /// the agent being absent.
    fn printed(&self) -> Result<Option<String>, String> {
        match self.launchctl(&["print", &self.target()]) {
            Ok(outcome) if outcome.ok() => return Ok(Some(outcome.stdout)),
            Ok(outcome) if not_found(&outcome) => {}
            Ok(outcome) => {
                return Err(format!(
                    "launchctl print {} failed: {}",
                    self.target(),
                    outcome.complaint()
                ));
            }
            Err(err) => return Err(format!("could not run launchctl: {err}")),
        }
        let domain = self.domain();
        match self.launchctl(&["print", &domain]) {
            Ok(outcome) if outcome.ok() => Ok(None),
            Ok(outcome) => Err(format!(
                "launchd does not answer for {domain}: {}",
                outcome.complaint()
            )),
            Err(err) => Err(format!("could not run launchctl: {err}")),
        }
    }

    /// `bootout`, which waits for the agent to exit.
    fn bootout(&self) -> Result<(), String> {
        match self.runner.run(
            "launchctl",
            &["bootout", &self.target()],
            &[],
            BOOTOUT_TIMEOUT,
        ) {
            Ok(outcome) if outcome.ok() => Ok(()),
            Ok(outcome) => Err(format!("launchctl bootout failed: {}", outcome.complaint())),
            Err(err) => Err(format!("could not run launchctl: {err}")),
        }
    }

    fn plist(&self) -> PathBuf {
        self.unit_path()
    }
}

/// Whether `launchctl print` said the job does not exist: exit 113, or the
/// message launchd gives for it.
fn not_found(outcome: &Outcome) -> bool {
    outcome.code == Some(113) || outcome.stderr.contains("Could not find service")
}

/// `key = value` from the body of `launchctl print`.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        (name.trim() == key).then_some(value.trim())
    })
}

impl Manager for Launchd {
    fn kind(&self) -> &'static str {
        "launchd"
    }

    fn os(&self) -> Os {
        Os::Macos
    }

    fn unit_path(&self) -> PathBuf {
        self.home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist"))
    }

    fn render(&self, spec: &UnitSpec) -> Result<String, String> {
        render::launchd_plist(spec, &self.launchd_log())
    }

    fn prepare(&self, want: Want) -> Result<Prepared, Unavailable> {
        if want == Want::Boot {
            return Err(Unavailable {
                code: 2,
                message: "--boot is not possible on macOS: a LaunchAgent starts when you log in. \
                          To start the hub at boot, either turn on automatic login for this \
                          account, or install a LaunchDaemon by hand with sudo (the guide has \
                          the recipe). Drop --boot to install the agent that starts at login."
                    .to_string(),
            });
        }
        let gui = format!("gui/{}", self.uid);
        let user = format!("user/{}", self.uid);
        let reachable = |domain: &str| match self.launchctl(&["print", domain]) {
            Ok(outcome) => Ok(outcome.ok()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Unavailable::none(
                "launchctl was not found: there is no launchd here. Run the hub with: \
                 agent-hub start",
            )),
            Err(err) => Err(Unavailable::none(format!("could not run launchctl: {err}"))),
        };
        if reachable(&gui)? {
            *self.domain.borrow_mut() = Some(gui);
            return Ok(Prepared {
                starts_at: StartsAt::Login,
                row: "login".to_string(),
                advice: None,
            });
        }
        if reachable(&user)? {
            *self.domain.borrow_mut() = Some(user);
            return Ok(Prepared {
                starts_at: StartsAt::Login,
                row: "login (nobody is logged in at the console, so the agent is loaded in \
                      your user domain and runs until reboot; log in once to make it last)"
                    .to_string(),
                advice: None,
            });
        }
        Err(Unavailable::none(format!(
            "launchd answers for neither gui/{uid} nor user/{uid}: there is no session to \
             hold a LaunchAgent. Log in at the console once, or run the hub with: agent-hub start",
            uid = self.uid
        )))
    }

    fn check(&self) -> Result<(), Unavailable> {
        let domain = self.domain();
        match self.launchctl(&["print", &domain]) {
            Ok(outcome) if outcome.ok() => Ok(()),
            Ok(outcome) => Err(Unavailable::none(format!(
                "launchd does not answer for {domain} ({}), so the agent cannot be stopped or \
                 removed safely; log in at the console and try again",
                outcome.complaint()
            ))),
            Err(err) => Err(Unavailable::none(format!("could not run launchctl: {err}"))),
        }
    }

    fn register(
        &self,
        _spec: &UnitSpec,
        changed: bool,
        start: bool,
    ) -> Result<Option<String>, Unavailable> {
        let refused = |message: String| Unavailable::after_check(self, message);
        if let Some(parent) = self.launchd_log().parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                Unavailable::failed(format!("could not create {}: {err}", parent.display()))
            })?;
        }
        let target = self.target();
        // Enabled first: a job switched off in Login Items cannot be loaded.
        self.must(&["enable", &target]).map_err(refused)?;
        let loaded = self.printed().map_err(Unavailable::none)?.is_some();
        if !start {
            // A no-start install never loads or starts anything. A loaded
            // agent whose plist changed would keep running the old one, so
            // disk and running state would disagree: it is unloaded instead,
            // and a later start loads the new plist.
            if loaded && changed {
                self.bootout().map_err(refused)?;
                return Ok(Some(super::UNLOADED_NOTE.to_string()));
            }
            return Ok(None);
        }
        let mut loaded = loaded;
        if loaded && changed {
            // launchd reads the plist at bootstrap: a changed one is loaded again.
            self.bootout().map_err(refused)?;
            loaded = false;
        }
        if !loaded {
            self.must(&["bootstrap", &self.domain(), &self.plist().to_string_lossy()])
                .map_err(refused)?;
        }
        Ok(None)
    }

    fn start(&self) -> Result<(), Unavailable> {
        if self.printed().map_err(Unavailable::none)?.is_none() {
            // RunAtLoad starts it as it is loaded.
            return self
                .must(&["bootstrap", &self.domain(), &self.plist().to_string_lossy()])
                .map(|_| ())
                .map_err(|message| Unavailable::after_check(self, message));
        }
        self.must(&["kickstart", &self.target()])
            .map(|_| ())
            .map_err(|message| Unavailable::after_check(self, message))
    }

    fn stop(&self, force: bool) -> Result<(), Unavailable> {
        let target = self.target();
        if self.printed().map_err(Unavailable::none)?.is_none() {
            return Ok(());
        }
        if force {
            let _ = self.launchctl(&["kill", "SIGKILL", &target]);
        }
        // Unloading the agent is how it is stopped for good: a plain kill would
        // have KeepAlive start it again.
        self.bootout()
            .map_err(|message| Unavailable::after_check(self, message))
    }

    fn job(&self) -> Result<Job, Unavailable> {
        if let Err(err) = self.launchctl(&["print", &self.domain()])
            && err.kind() == std::io::ErrorKind::NotFound
        {
            return Ok(Job::Absent);
        }
        self.check()?;
        Ok(match self.printed().map_err(Unavailable::none)? {
            None => Job::Absent,
            Some(text) => Job::Loaded {
                config: render::launchd_shown_config(&text),
            },
        })
    }

    fn job_name(&self) -> String {
        LAUNCHD_LABEL.to_string()
    }

    fn unload_hint(&self) -> String {
        format!("launchctl bootout {}", self.target())
    }

    fn state(&self) -> Result<UnitState, String> {
        let printed = self.printed()?;
        let domain = self.domain();
        let disabled = self
            .launchctl(&["print-disabled", &domain])
            .ok()
            .filter(Outcome::ok)
            .is_some_and(|outcome| {
                outcome
                    .stdout
                    .lines()
                    .any(|line| line.contains(LAUNCHD_LABEL) && line.contains("=> disabled"))
                    || outcome
                        .stdout
                        .lines()
                        .any(|line| line.contains(LAUNCHD_LABEL) && line.contains("=> true"))
            });
        let enabled = !disabled;
        let (active, detail) = match printed {
            None => (Active::Inactive, None),
            Some(text) => match field(&text, "state") {
                Some("running") => (Active::Running, None),
                Some("spawn scheduled") | Some("xpcproxy") => (Active::Activating, None),
                _ => match field(&text, "last exit code") {
                    Some(code) if code != "0" && !code.contains("never") => {
                        (Active::Failed, Some(format!("exit {code}")))
                    }
                    _ => (Active::Inactive, None),
                },
            },
        };
        let user_domain = domain.starts_with("user/");
        let starts_at = if !enabled || user_domain {
            "manual"
        } else {
            "login"
        };
        let detail = if user_domain && detail.is_none() {
            Some("user domain: runs until reboot only".to_string())
        } else {
            detail
        };
        Ok(UnitState {
            active,
            enabled,
            starts_at,
            detail,
        })
    }

    fn main_pid(&self) -> Result<Option<u32>, String> {
        Ok(self
            .printed()?
            .and_then(|text| field(&text, "pid").and_then(|pid| pid.parse::<u32>().ok()))
            .filter(|pid| *pid != 0))
    }

    fn remove(&self) -> Result<(), Unavailable> {
        self.check()?;
        if let Err(complaint) = self.bootout() {
            // A failed bootout is fine only when launchd says it does not have
            // the agent; otherwise the plist stays so the job is not orphaned.
            match self.printed() {
                Ok(None) => {}
                Ok(Some(_)) => return Err(Unavailable::failed(complaint)),
                Err(err) => return Err(Unavailable::none(err)),
            }
        }
        match std::fs::remove_file(self.plist()) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Unavailable::failed(format!(
                "could not remove {}: {err}",
                self.plist().display()
            ))),
        }
    }

    fn logs(&self, _options: &LogsOptions) -> Option<ExitCode> {
        // The hub logs to the file in config.toml, which `logs` reads.
        None
    }

    fn failure_output(&self) -> Vec<String> {
        crate::daemon::logfile::last_lines(&self.launchd_log(), 20).unwrap_or_default()
    }

    fn log_hint(&self) -> String {
        "agent-hub logs  (~/Library/Logs/agent-hub/hub.log)".to_string()
    }

    fn manager_logs(&self) -> Vec<PathBuf> {
        // Only the file the installed plist names is the agent's own log.
        std::fs::read_to_string(self.plist())
            .ok()
            .and_then(|text| render::launchd_log_path(&text))
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::runner::recording::Recorder;
    use std::path::Path;

    fn scratch(tag: &str) -> PathBuf {
        let dir = PathBuf::from("target/tmp").join(format!("launchd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    fn launchd(recorder: &Recorder, home: &Path) -> Launchd {
        Launchd::new(Box::new(recorder.clone()), home.to_path_buf(), 501)
    }

    fn spec() -> UnitSpec {
        UnitSpec {
            binary: PathBuf::from("/b"),
            config: PathBuf::from("/c"),
            home: PathBuf::from("/h"),
            starts_at: StartsAt::Login,
        }
    }

    #[test]
    fn the_plist_lives_in_launch_agents() {
        let home = PathBuf::from("/macusers/me");
        assert_eq!(
            launchd(&Recorder::default(), &home).unit_path(),
            PathBuf::from("/macusers/me/Library/LaunchAgents/dev.agync.agent-hub.plist")
        );
    }

    #[test]
    fn boot_is_refused_with_the_two_options_and_runs_nothing() {
        let recorder = Recorder::default();
        let err = launchd(&recorder, &PathBuf::from("/macusers/me"))
            .prepare(Want::Boot)
            .expect_err("refused");
        assert_eq!(err.code, 2);
        assert!(err.message.contains("automatic login"), "{}", err.message);
        assert!(err.message.contains("LaunchDaemon"), "{}", err.message);
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn the_gui_domain_is_used_when_someone_is_logged_in() {
        let recorder = Recorder::default();
        let manager = launchd(&recorder, &PathBuf::from("/macusers/me"));
        let prepared = manager.prepare(Want::Default).expect("prepare");
        assert_eq!(prepared.starts_at, StartsAt::Login);
        assert_eq!(manager.target(), "gui/501/dev.agync.agent-hub");
        assert_eq!(recorder.calls(), vec!["launchctl print gui/501"]);
    }

    #[test]
    fn with_no_console_login_the_user_domain_is_used_and_said() {
        let recorder = Recorder::default();
        recorder.exit("launchctl print gui/501", 113, "", "Could not find domain");
        let manager = launchd(&recorder, &PathBuf::from("/macusers/me"));
        let prepared = manager.prepare(Want::Login).expect("prepare");
        assert!(prepared.row.contains("until reboot"), "{}", prepared.row);
        assert_eq!(manager.target(), "user/501/dev.agync.agent-hub");
    }

    #[test]
    fn no_launchd_at_all_is_exit_69() {
        let recorder = Recorder::default();
        recorder.missing("launchctl");
        let err = launchd(&recorder, &PathBuf::from("/macusers/me"))
            .prepare(Want::Default)
            .expect_err("unavailable");
        assert_eq!(err.code, 69);
    }

    #[test]
    fn a_fresh_register_bootstraps_then_enables() {
        let home = scratch("register");
        let recorder = Recorder::default();
        recorder.exit("launchctl print gui/501/dev", 113, "", "not found");
        launchd(&recorder, &home)
            .register(&spec(), false, true)
            .expect("register");
        let plist = home.join("Library/LaunchAgents/dev.agync.agent-hub.plist");
        assert_eq!(
            recorder.calls(),
            vec![
                "launchctl print gui/501".to_string(),
                "launchctl enable gui/501/dev.agync.agent-hub".to_string(),
                "launchctl print gui/501/dev.agync.agent-hub".to_string(),
                "launchctl print gui/501".to_string(),
                format!("launchctl bootstrap gui/501 {}", plist.display()),
            ]
        );
        assert!(home.join("Library/Logs/agent-hub").is_dir());
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn a_changed_plist_is_unloaded_and_loaded_again() {
        let home = scratch("changed");
        let recorder = Recorder::default();
        launchd(&recorder, &home)
            .register(&spec(), true, true)
            .expect("register");
        let calls = recorder.calls();
        let bootout = calls
            .iter()
            .position(|call| call == "launchctl bootout gui/501/dev.agync.agent-hub")
            .expect("the old one is unloaded");
        let bootstrap = calls
            .iter()
            .position(|call| call.starts_with("launchctl bootstrap"))
            .expect("the new one is loaded");
        assert!(bootout < bootstrap, "{calls:?}");
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn an_unchanged_loaded_plist_is_left_loaded() {
        let home = scratch("unchanged");
        let recorder = Recorder::default();
        launchd(&recorder, &home)
            .register(&spec(), false, true)
            .expect("register");
        let calls = recorder.calls();
        assert!(
            calls
                .iter()
                .all(|call| !call.starts_with("launchctl bootstrap") && !call.contains("bootout")),
            "{calls:?}"
        );
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn a_no_start_register_never_loads_or_starts_the_agent() {
        let home = scratch("nostart");
        for changed in [true, false] {
            let recorder = Recorder::default();
            recorder.exit("launchctl print gui/501/dev", 113, "", "not found");
            launchd(&recorder, &home)
                .register(&spec(), changed, false)
                .expect("register");
            let calls = recorder.calls();
            assert!(
                calls.iter().all(|call| !call.contains("bootstrap")
                    && !call.contains("kickstart")
                    && !call.contains("bootout")),
                "{calls:?}"
            );
            assert!(
                calls.contains(&"launchctl enable gui/501/dev.agync.agent-hub".to_string()),
                "{calls:?}"
            );
        }
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn the_main_pid_comes_from_launchctl_print() {
        let recorder = Recorder::default();
        recorder.exit(
            "launchctl print gui/501/dev",
            0,
            "dev.agync.agent-hub = {\n\tstate = running\n\tpid = 612\n}\n",
            "",
        );
        assert_eq!(
            launchd(&recorder, &PathBuf::from("/macusers/me")).main_pid(),
            Ok(Some(612))
        );
        let gone = Recorder::default();
        gone.exit(
            "launchctl print gui/501/dev",
            113,
            "",
            "Could not find service",
        );
        assert_eq!(
            launchd(&gone, &PathBuf::from("/macusers/me")).main_pid(),
            Ok(None)
        );
    }

    #[test]
    fn launchd_that_does_not_answer_is_69_and_the_plist_stays() {
        let home = scratch("unreachable");
        let plist = home.join("Library/LaunchAgents/dev.agync.agent-hub.plist");
        std::fs::create_dir_all(plist.parent().unwrap()).expect("create");
        std::fs::write(&plist, "<plist/>").expect("seed");
        let recorder = Recorder::default();
        recorder.exit("launchctl print", 113, "", "Could not find domain");
        let manager = launchd(&recorder, &home);
        assert!(manager.state().is_err(), "unknown, not stopped");
        let err = manager.remove().expect_err("refused");
        assert_eq!(err.code, 69);
        assert!(plist.exists());
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn a_failed_bootout_keeps_the_plist_unless_the_agent_is_gone() {
        let home = scratch("bootout");
        let plist = home.join("Library/LaunchAgents/dev.agync.agent-hub.plist");
        std::fs::create_dir_all(plist.parent().unwrap()).expect("create");
        std::fs::write(&plist, "<plist/>").expect("seed");
        let loaded = Recorder::default();
        loaded.exit(
            "launchctl bootout",
            5,
            "",
            "Boot-out failed: 5: Input/output error",
        );
        let err = launchd(&loaded, &home).remove().expect_err("refused");
        assert_eq!(err.code, 1);
        assert!(plist.exists(), "a loaded agent keeps its plist");

        let absent = Recorder::default();
        absent.exit("launchctl bootout", 3, "", "No such process");
        absent.exit(
            "launchctl print gui/501/dev",
            113,
            "",
            "Could not find service",
        );
        launchd(&absent, &home).remove().expect("removed");
        assert!(!plist.exists());
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn stop_unloads_and_a_forced_stop_kills_first() {
        let recorder = Recorder::default();
        launchd(&recorder, &PathBuf::from("/macusers/me"))
            .stop(true)
            .expect("stop");
        let calls = recorder.calls();
        let kill = calls
            .iter()
            .position(|call| call == "launchctl kill SIGKILL gui/501/dev.agync.agent-hub")
            .expect("killed");
        let bootout = calls
            .iter()
            .position(|call| call == "launchctl bootout gui/501/dev.agync.agent-hub")
            .expect("unloaded");
        assert!(kill < bootout, "{calls:?}");
    }

    #[test]
    fn the_state_reads_launchctl_print() {
        let running = Recorder::default();
        running.exit(
            "launchctl print gui/501/dev",
            0,
            "dev.agync.agent-hub = {\n\tstate = running\n\tpid = 612\n}\n",
            "",
        );
        running.exit(
            "launchctl print-disabled",
            0,
            "disabled services = {\n\t\"dev.agync.agent-hub\" => enabled\n}\n",
            "",
        );
        let state = launchd(&running, &PathBuf::from("/macusers/me"))
            .state()
            .expect("state");
        assert_eq!(state.active, Active::Running);
        assert!(state.enabled);
        assert_eq!(state.starts_at, "login");

        let failed = Recorder::default();
        failed.exit(
            "launchctl print gui/501/dev",
            0,
            "dev.agync.agent-hub = {\n\tstate = not running\n\tlast exit code = 78\n}\n",
            "",
        );
        let state = launchd(&failed, &PathBuf::from("/macusers/me"))
            .state()
            .expect("state");
        assert_eq!(state.active, Active::Failed);
        assert_eq!(state.detail.as_deref(), Some("exit 78"));

        let off = Recorder::default();
        off.exit(
            "launchctl print-disabled",
            0,
            "disabled services = {\n\t\"dev.agync.agent-hub\" => disabled\n}\n",
            "",
        );
        let state = launchd(&off, &PathBuf::from("/macusers/me"))
            .state()
            .expect("state");
        assert!(!state.enabled, "the human switched it off in Login Items");
        assert_eq!(state.starts_at, "manual");
    }

    #[test]
    fn removing_unloads_and_deletes_the_plist() {
        let home = scratch("remove");
        let plist = home.join("Library/LaunchAgents/dev.agync.agent-hub.plist");
        std::fs::create_dir_all(plist.parent().unwrap()).expect("create");
        std::fs::write(&plist, "<plist/>").expect("seed");
        let recorder = Recorder::default();
        launchd(&recorder, &home).remove().expect("remove");
        assert!(!plist.exists());
        assert!(
            recorder
                .calls()
                .contains(&"launchctl bootout gui/501/dev.agync.agent-hub".to_string())
        );
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn a_no_start_register_unloads_a_loaded_agent_whose_plist_changed_and_says_so() {
        let home = scratch("unload");
        let recorder = Recorder::default();
        let note = launchd(&recorder, &home)
            .register(&spec(), true, false)
            .expect("register");
        let calls = recorder.calls();
        assert!(
            calls.contains(&"launchctl bootout gui/501/dev.agync.agent-hub".to_string()),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .all(|call| !call.contains("bootstrap") && !call.contains("kickstart")),
            "{calls:?}"
        );
        assert!(note.expect("a note").contains("not running"));

        // Unchanged and loaded: left as it is, nothing to say.
        let same = Recorder::default();
        let note = launchd(&same, &home)
            .register(&spec(), false, false)
            .expect("register");
        assert!(note.is_none());
        assert!(same.calls().iter().all(|call| !call.contains("bootout")));
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn only_a_job_not_found_answer_means_not_loaded_and_any_other_failure_is_unreachable() {
        let home = PathBuf::from("/macusers/me");
        let missing = Recorder::default();
        missing.exit(
            "launchctl print gui/501/dev",
            113,
            "",
            "Could not find service \"dev.agync.agent-hub\" in domain for gui: 501",
        );
        assert_eq!(launchd(&missing, &home).main_pid(), Ok(None));

        for (code, stderr) in [(1, "Operation not permitted"), (5, "Input/output error")] {
            let broken = Recorder::default();
            broken.exit("launchctl print gui/501/dev", code, "", stderr);
            let manager = launchd(&broken, &home);
            assert!(manager.state().is_err(), "{stderr}");
            assert!(manager.main_pid().is_err(), "{stderr}");
            assert_eq!(manager.start().expect_err("69").code, 69, "{stderr}");
            assert_eq!(manager.stop(false).expect_err("69").code, 69, "{stderr}");
            assert_eq!(manager.job().expect_err("69").code, 69, "{stderr}");
        }
    }

    #[test]
    fn the_manager_says_whether_the_job_is_loaded_and_what_config_it_runs() {
        let loaded = Recorder::default();
        loaded.exit(
            "launchctl print gui/501/dev",
            0,
            "dev.agync.agent-hub = {\n\tenvironment = {\n\t\tHUB_CONFIG => /m/config.toml\n\t}\n}\n",
            "",
        );
        assert_eq!(
            launchd(&loaded, &PathBuf::from("/macusers/me"))
                .job()
                .expect("job"),
            Job::Loaded {
                config: Some(PathBuf::from("/m/config.toml"))
            }
        );
        let gone = Recorder::default();
        gone.exit(
            "launchctl print gui/501/dev",
            113,
            "",
            "Could not find service",
        );
        assert_eq!(
            launchd(&gone, &PathBuf::from("/macusers/me"))
                .job()
                .expect("job"),
            Job::Absent
        );
    }

    #[test]
    fn only_the_log_the_installed_plist_names_is_the_agents_own() {
        let home = scratch("logs");
        let manager = launchd(&Recorder::default(), &home);
        assert!(manager.manager_logs().is_empty(), "no plist, no log");
        let plist = manager.unit_path();
        std::fs::create_dir_all(plist.parent().unwrap()).expect("create");
        std::fs::write(&plist, "<plist/>").expect("seed");
        assert!(manager.manager_logs().is_empty(), "a plist naming none");
        let text = render::launchd_plist(&spec(), &PathBuf::from("/macusers/me/named.log"))
            .expect("render");
        std::fs::write(&plist, text).expect("seed");
        assert_eq!(
            manager.manager_logs(),
            vec![PathBuf::from("/macusers/me/named.log")]
        );
        std::fs::remove_dir_all(&home).expect("clean up");
    }
}
