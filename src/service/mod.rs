//! Running the hub under a per-user service manager.
//!
//! A [`Manager`] is one service manager's side of the work: systemd's user
//! instance on Linux, a launchd LaunchAgent on macOS, and a fake that keeps its
//! state in a directory for tests. [`backend::ServiceBackend`] puts a manager
//! behind the lifecycle verbs of [`crate::daemon::Backend`], so `start`, `stop`,
//! `restart`, `status` and `logs` act on the service whenever one is installed.
//! [`install`] holds `service install`, `service uninstall` and `uninstall`.
//!
//! Every command a manager runs goes through a [`runner::Runner`]. On Windows
//! the manager is a Task Scheduler task ([`schtasks`]) that runs a supervisor
//! ([`supervise`]) around `serve`.

pub mod backend;
pub mod fake;
pub mod install;
pub mod launchd;
pub mod purge;
pub mod render;
pub mod runner;
pub mod schtasks;
pub mod supervise;
pub mod systemd;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_hub::config::{LifecycleSettings, Os, present};

use crate::daemon::LogsOptions;

pub use backend::ServiceBackend;

/// What a `--no-start` install says when it had to unload a running agent.
pub const UNLOADED_NOTE: &str = "The loaded agent was stopped and unloaded because its definition \
     changed; it is not running now.";

/// Where a service starts: with the machine, or with the user's login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartsAt {
    Boot,
    Login,
}

impl StartsAt {
    pub fn word(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Login => "login",
        }
    }
}

/// What the operator asked for with `--boot` and `--login`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Neither flag: the platform's default (boot on Linux, login on macOS).
    Default,
    Boot,
    Login,
}

/// What a definition is rendered from.
#[derive(Debug, Clone)]
pub struct UnitSpec {
    /// The `agent-hub` binary the service runs.
    pub binary: PathBuf,
    /// The config file the service reads, pinned as `HUB_CONFIG`.
    pub config: PathBuf,
    pub home: PathBuf,
    pub starts_at: StartsAt,
}

/// What a manager settled on before anything is written.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub starts_at: StartsAt,
    /// The text of the "starts at" row.
    pub row: String,
    /// What the human still has to do, when it needs a privilege we never take.
    pub advice: Option<String>,
}

/// The manager cannot be used, with the exit code that says why.
#[derive(Debug, Clone)]
pub struct Unavailable {
    /// 69 when there is no manager to reach, 2 when the request cannot be met.
    pub code: u8,
    pub message: String,
}

impl Unavailable {
    pub fn none(message: impl Into<String>) -> Self {
        Self {
            code: 69,
            message: message.into(),
        }
    }

    /// A step failed after the manager was last checked: 69 when the manager
    /// no longer answers (the bus went away, the session ended), else 1.
    pub fn after_check(manager: &dyn Manager, message: impl Into<String>) -> Self {
        let message = message.into();
        match manager.check() {
            Ok(()) => Self::failed(message),
            Err(_) => Self::none(format!("{message}; the service manager no longer answers")),
        }
    }

    /// The manager answered and refused: exit 1.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Active {
    Running,
    /// Starting, or waiting to be restarted.
    Activating,
    Inactive,
    Failed,
}

/// What the manager says about the service.
#[derive(Debug, Clone)]
pub struct UnitState {
    pub active: Active,
    pub enabled: bool,
    /// `boot`, `login` or `manual`.
    pub starts_at: &'static str,
    /// Why it failed, or what the human should know.
    pub detail: Option<String>,
}

/// Whether the manager has the job loaded, whatever the definition file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// The manager does not have it, or there is no manager on this machine.
    Absent,
    /// The manager has it. `config` is the `HUB_CONFIG` the loaded job runs
    /// with, when the manager says.
    Loaded { config: Option<PathBuf> },
}

/// One service manager. It does not decide what to do, only how to do it.
pub trait Manager {
    /// `systemd` or `launchd`, as `status --json` names it.
    fn kind(&self) -> &'static str;
    /// The directory layout this manager's platform uses.
    fn os(&self) -> Os;
    /// The service definition on disk.
    fn unit_path(&self) -> PathBuf;
    /// Whether a service is installed: the definition is there.
    fn is_installed(&self) -> bool {
        self.unit_path().is_file()
    }
    /// The definition's text.
    fn render(&self, spec: &UnitSpec) -> Result<String, String>;
    /// The config file the installed definition pins as `HUB_CONFIG`. `Err`
    /// when the definition cannot be read or names none: the caller must not
    /// fall back to some other settings then.
    fn pinned_config(&self) -> Result<PathBuf, String> {
        let path = self.unit_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        let pinned = if self.kind() == "launchd" {
            render::launchd_pinned_config(&text)
        } else {
            render::systemd_pinned_config(&text)
        };
        pinned.ok_or_else(|| {
            format!(
                "{} names no HUB_CONFIG, so the settings the service runs with are unknown; \
                 re-run agent-hub service install to rewrite it, or remove it",
                path.display()
            )
        })
    }
    /// Make the manager reachable and settle boot against login.
    fn prepare(&self, want: Want) -> Result<Prepared, Unavailable>;
    /// Whether the manager answers at all. 69 when it does not.
    fn check(&self) -> Result<(), Unavailable>;
    /// Tell the manager about the definition just written: reload and enable.
    /// `changed` says the file's text differs from what the manager loaded.
    /// `start` false means the service must not start as a side effect. A
    /// returned note is something the human is told (a loaded agent that was
    /// stopped because its definition changed). A manager that stopped
    /// answering since the last check fails with 69, not 1.
    fn register(
        &self,
        spec: &UnitSpec,
        changed: bool,
        start: bool,
    ) -> Result<Option<String>, Unavailable>;
    /// Ask the manager to start the service. It returns when the manager has
    /// accepted the request, not when the hub answers. 69 when the manager
    /// cannot be reached.
    fn start(&self) -> Result<(), Unavailable>;
    /// Ask the manager to stop the service, without waiting for it. `force`
    /// kills it with no drain. 69 when the manager cannot be reached.
    fn stop(&self, force: bool) -> Result<(), Unavailable>;
    /// What the manager says about the service. `Err` when it cannot be
    /// asked, which is not the same as the service being stopped.
    fn state(&self) -> Result<UnitState, String>;
    /// Ask the manager itself whether the job is loaded, independent of the
    /// definition file. A manager that cannot be asked is 69; a machine with
    /// no manager program has nothing loaded.
    fn job(&self) -> Result<Job, Unavailable>;
    /// The job's name as the manager knows it, for a message.
    fn job_name(&self) -> String;
    /// The command the human runs to unload the job by hand.
    fn unload_hint(&self) -> String;
    /// The pid of the process the manager runs as the service, when it runs
    /// one. `Err` when the manager cannot be asked.
    fn main_pid(&self) -> Result<Option<u32>, String>;
    /// Stop and unregister the service, delete the definition, and leave
    /// everything else (linger, the config, the data) as found. It fails,
    /// leaving the definition, unless the manager let go of the service or
    /// says it never had it.
    fn remove(&self) -> Result<(), Unavailable>;
    /// Print the hub's log through the manager's own tool, or `None` when the
    /// log is the file the hub writes.
    fn logs(&self, options: &LogsOptions) -> Option<ExitCode>;
    /// The last lines the service said, for a start that failed.
    fn failure_output(&self) -> Vec<String>;
    /// How the `logs` row of the install summary reads.
    fn log_hint(&self) -> String;
    /// Log files the manager itself writes for the service, beside the hub's
    /// own log file.
    fn manager_logs(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// The environment the process runs in, as a lookup.
///
/// On Windows `HOME` is not set; it reads as the user profile there, so the
/// verbs that name the home directory work. Elsewhere the environment is
/// returned as it is.
pub fn process_env(key: &str) -> Option<String> {
    let value = std::env::var(key).ok();
    if cfg!(windows) && key == "HOME" {
        return agent_hub::config::present(value).or_else(|| std::env::var("USERPROFILE").ok());
    }
    value
}

/// The manager for this machine. `None` where there is none to drive: a
/// Windows account with no `LOCALAPPDATA`, a Linux one with no `HOME`.
///
/// `AGENT_HUB_SERVICE_FAKE=<dir>` swaps in the fake manager that keeps its
/// state in `<dir>`. It exists so the built binary can be driven through every
/// install, upgrade and uninstall sequence without touching a real service
/// manager; the real ones are never reached while it is set.
pub fn host_manager(env: &dyn Fn(&str) -> Option<String>) -> Option<Box<dyn Manager>> {
    if let Some(dir) = present(env("AGENT_HUB_SERVICE_FAKE")) {
        let launchd = present(env("AGENT_HUB_SERVICE_FAKE_KIND")).as_deref() == Some("launchd");
        return Some(Box::new(fake::Fake::new(PathBuf::from(dir), launchd)));
    }
    match Os::current() {
        Os::Windows => Some(Box::new(schtasks::Schtasks::host(env)?)),
        Os::Macos => Some(Box::new(launchd::Launchd::host(env)?)),
        Os::Linux => Some(Box::new(systemd::Systemd::host(env)?)),
    }
}

/// The environment as the installed service sees it: `HUB_CONFIG` names
/// `config`, and no `HUB_*` setting variable is set, as the unit and the plist
/// arrange. Every other variable is the caller's. With no config file, the
/// caller's `HUB_CONFIG` stands.
pub fn pinned_env<'a>(
    env: &'a dyn Fn(&str) -> Option<String>,
    config: Option<&'a Path>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| {
        if key == "HUB_CONFIG"
            && let Some(config) = config
        {
            return Some(config.display().to_string());
        }
        let setting = key.strip_prefix("HUB_").is_some_and(|rest| {
            agent_hub::config::HUB_KEYS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(rest))
        });
        if setting { None } else { env(key) }
    }
}

/// The settings the lifecycle verbs act on.
///
/// With a service installed they are the service's own: the config file its
/// definition pins, and none of the caller's `HUB_*` overrides, so `status` or
/// `restart` from a shell with other settings still find the service's store.
/// A definition that cannot be read or parsed is an error, not a reason to use
/// the shell's settings. Without a service they are the caller's, as `serve`
/// and `start` read them.
pub fn verb_settings() -> agent_hub::Result<LifecycleSettings> {
    let env: &dyn Fn(&str) -> Option<String> = &process_env;
    if let Some(manager) = host_manager(env)
        && manager.is_installed()
    {
        let config = manager.pinned_config().map_err(agent_hub::Error::Config)?;
        return LifecycleSettings::resolve(&pinned_env(env, Some(&config)), manager.os());
    }
    LifecycleSettings::from_env()
}

/// The service backend for the lifecycle verbs, when a service is installed.
/// A machine with no definition on disk runs no command to find that out.
pub fn backend_for(_settings: &LifecycleSettings) -> Option<Box<dyn crate::daemon::Backend>> {
    let manager = host_manager(&process_env)?;
    manager
        .is_installed()
        .then(|| Box::new(ServiceBackend::new(manager)) as Box<dyn crate::daemon::Backend>)
}

/// The user id, for the manager's own domain names.
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        rustix::process::getuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Write a service definition: the parent directories, then the file at 0644.
pub fn write_definition(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".tmp-{}", std::process::id()));
    let temp = PathBuf::from(temp);
    let written = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// `path` with the home directory shown as `~`.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// Whether the configured address is free to bind, and who has it when not.
///
/// `None` means free. A hub that answers there is named, so the operator can
/// tell a container from some other program.
pub fn port_problem(bind: std::net::SocketAddr, data_dir: &Path) -> Option<String> {
    match std::net::TcpListener::bind(bind) {
        Ok(listener) => {
            drop(listener);
            None
        }
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            let port = bind.port();
            let fix = "set [hub] bind in config.toml to use another address";
            Some(match crate::daemon::process::probe_hub(bind) {
                Some(()) => format!(
                    "port {port} is held by another hub, not one serving {}; a container? {fix}",
                    data_dir.display()
                ),
                None => format!("port {port} is in use by another program; {fix}"),
            })
        }
        Err(err) => Some(format!("cannot bind {bind}: {err}")),
    }
}
