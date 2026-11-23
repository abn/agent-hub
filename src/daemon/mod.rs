//! Running the hub as a background process: `start`, `stop`, `restart`,
//! `status`, `logs`, and the bare `agent-hub` that reports status.
//!
//! The verbs act on "the hub" through a [`Backend`]. Today the one backend is
//! [`process::Process`], a plain process that `hub.lock` identifies. A service
//! backend (systemd, launchd, a scheduled task) plugs in at [`backend`] and
//! takes over the verbs while a service is installed; `status` and the argument
//! handling stay here, so every backend reports the same way.

pub mod lock;
pub mod logfile;
pub mod process;

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use agent_hub::config::LifecycleSettings;
use serde_json::json;

use lock::{Holder, Mode, Record};

pub const START_USAGE: &str = "\
usage:
  agent-hub start

Start the hub in the background and wait until it answers. The hub is the
installed service when there is one, else a detached `agent-hub serve`. The
hub logs to the log file; what it prints before that, and what it fails with,
goes to the same path with .start appended. A hub that is already running is left alone: this
prints 'already running' and exits 0. The data directory and log file default
to the platform's own places when the configuration does not set them. Exits 1
when the hub did not come up, with the last lines of what it said or the
reason the port is taken, and when an embedded mcp session holds the store.
Exits 69 for a plain background process on Windows, where the service starts it.
";

pub const STOP_USAGE: &str = "\
usage:
  agent-hub stop [--timeout SECONDS] [--force]

Stop the hub. It drains in-flight requests, then folds the store's log, and
this returns once the process has let go of the data directory. Exits 0 when
the hub stopped or was not running, and 1 when it did not stop within the
timeout (20 seconds by default). --force kills the process at once, with no
drain and no checkpoint; run agent-hub check afterwards.
";

pub const RESTART_USAGE: &str = "\
usage:
  agent-hub restart

Stop the hub, then start it. A hub that was not running is just started.
";

pub const STATUS_USAGE: &str = "\
usage:
  agent-hub status [--json]

Say whether the hub is running, how it is managed and where it lives. Exits 0
when it is running and ready, 1 when it is running but not ready, and 3 when it
is not running. --json prints the same facts as one JSON object.
";

pub const LOGS_USAGE: &str = "\
usage:
  agent-hub logs [-f] [-n LINES]

Print the last LINES lines of the hub's log (50 by default). -f keeps
following the file as the hub writes it, across a rotation, until interrupted.
";

pub const VERSION_USAGE: &str = "\
usage:
  agent-hub --version
  agent-hub version

Print the version and the commit the binary was built from.
";

/// How a backend says the hub is managed, for `status`.
pub struct Managed {
    /// `systemd`, `launchd`, `task`, `process` or `none`.
    pub kind: &'static str,
    /// The sentence `status` prints beside "managed by".
    pub label: String,
    /// Whether a service is set to start by itself.
    pub enabled: bool,
    /// `boot`, `login` or `manual`.
    pub starts_at: &'static str,
}

pub struct StopOptions {
    pub timeout: Duration,
    pub force: bool,
}

pub struct LogsOptions {
    pub follow: bool,
    pub lines: usize,
}

/// What a verb does, for one way of running the hub.
pub trait Backend {
    /// How the hub is managed. `holder` is the record of the process holding
    /// `hub.lock`, when one does.
    fn managed(&self, settings: &LifecycleSettings, holder: Option<&Record>) -> Managed;
    /// Start the hub and wait for it. Running already is success.
    fn start(&self, settings: &LifecycleSettings) -> ExitCode;
    /// Stop the hub and wait for it. Not running is success.
    fn stop(&self, settings: &LifecycleSettings, options: &StopOptions) -> ExitCode;
    /// Print the hub's log.
    fn logs(&self, settings: &LifecycleSettings, options: &LogsOptions) -> ExitCode;
    /// Stop, then start.
    fn restart(&self, settings: &LifecycleSettings, options: &StopOptions) -> ExitCode {
        let stopped = self.stop(settings, options);
        if stopped != ExitCode::SUCCESS {
            return stopped;
        }
        self.start(settings)
    }
}

/// The backend that runs the verbs. A service backend returns here, ahead of
/// the plain process, when a service is installed.
pub fn backend(_settings: &LifecycleSettings) -> Box<dyn Backend> {
    Box::new(process::Process)
}

/// The version line: `agent-hub 1.13.0 (abc1234)`.
pub fn version_line() -> String {
    let commit = env!("GIT_COMMIT_SHORT");
    if commit.is_empty() {
        format!("agent-hub {}", env!("CARGO_PKG_VERSION"))
    } else {
        format!("agent-hub {} ({commit})", env!("CARGO_PKG_VERSION"))
    }
}

pub fn version_cmd() -> ExitCode {
    println!("{}", version_line());
    ExitCode::SUCCESS
}

/// The settings every verb reads, or the exit code of the failure to resolve
/// them.
fn settings_or_exit(command: &str) -> Result<LifecycleSettings, ExitCode> {
    LifecycleSettings::from_env().map_err(|err| {
        eprintln!("agent-hub {command}: {err}");
        ExitCode::from(78)
    })
}

fn usage_error(command: &str, message: &str, usage: &str) -> ExitCode {
    eprintln!("agent-hub {command}: {message}\n{usage}");
    ExitCode::from(2)
}

pub fn start_cmd(args: &[String]) -> ExitCode {
    if let Some(extra) = args.first() {
        return usage_error(
            "start",
            &format!("unexpected argument '{extra}'"),
            START_USAGE,
        );
    }
    let settings = match settings_or_exit("start") {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    backend(&settings).start(&settings)
}

pub fn restart_cmd(args: &[String]) -> ExitCode {
    if let Some(extra) = args.first() {
        return usage_error(
            "restart",
            &format!("unexpected argument '{extra}'"),
            RESTART_USAGE,
        );
    }
    let settings = match settings_or_exit("restart") {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let options = StopOptions {
        timeout: DEFAULT_STOP_TIMEOUT,
        force: false,
    };
    backend(&settings).restart(&settings, &options)
}

const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(20);

pub fn stop_cmd(args: &[String]) -> ExitCode {
    let mut options = StopOptions {
        timeout: DEFAULT_STOP_TIMEOUT,
        force: false,
    };
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--force" => options.force = true,
            "--timeout" => match args.next().and_then(|value| value.parse::<u64>().ok()) {
                Some(seconds) => options.timeout = Duration::from_secs(seconds),
                None => {
                    return usage_error(
                        "stop",
                        "--timeout needs a whole number of seconds",
                        STOP_USAGE,
                    );
                }
            },
            other => {
                return usage_error("stop", &format!("unknown option '{other}'"), STOP_USAGE);
            }
        }
    }
    let settings = match settings_or_exit("stop") {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    backend(&settings).stop(&settings, &options)
}

pub fn logs_cmd(args: &[String]) -> ExitCode {
    let mut options = LogsOptions {
        follow: false,
        lines: 50,
    };
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-f" | "--follow" => options.follow = true,
            "-n" | "--lines" => match args.next().and_then(|value| value.parse::<usize>().ok()) {
                Some(lines) => options.lines = lines,
                None => {
                    return usage_error("logs", "-n needs a whole number of lines", LOGS_USAGE);
                }
            },
            other => {
                return usage_error("logs", &format!("unknown option '{other}'"), LOGS_USAGE);
            }
        }
    }
    let settings = match settings_or_exit("logs") {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    backend(&settings).logs(&settings, &options)
}

pub fn status_cmd(args: &[String]) -> ExitCode {
    let mut json = false;
    for argument in args {
        match argument.as_str() {
            "--json" => json = true,
            other => {
                return usage_error("status", &format!("unknown option '{other}'"), STATUS_USAGE);
            }
        }
    }
    let settings = match settings_or_exit("status") {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let status = gather(&settings, backend(&settings).as_ref());
    if json {
        println!("{}", status.to_json());
    } else {
        print!("{}", status.render(home().as_deref()));
    }
    ExitCode::from(status.exit_code())
}

/// A bare `agent-hub`: the status block and the usage, exit 0.
///
/// It never serves. Run in the wrong directory it would otherwise create a
/// second, empty store beside the real one.
pub fn bare(usage: &str) -> ExitCode {
    match LifecycleSettings::from_env() {
        Ok(settings) => {
            let status = gather(&settings, backend(&settings).as_ref());
            print!("{}", status.render(home().as_deref()));
        }
        Err(err) => {
            println!("{}\n  config       {err}", version_line());
        }
    }
    println!();
    print!("{usage}");
    ExitCode::SUCCESS
}

fn home() -> Option<String> {
    agent_hub::config::present(std::env::var("HOME").ok())
}

/// Everything `status` reports.
pub struct Status {
    version: String,
    /// `running`, `starting`, `stopped`, or `failed` (a service backend's).
    state: &'static str,
    ready: bool,
    record: Option<Record>,
    managed: Managed,
    url: String,
    data_dir: std::path::PathBuf,
    data_bytes: Option<u64>,
    config: Option<std::path::PathBuf>,
    log: std::path::PathBuf,
    note: Option<String>,
}

/// Read the lock, the hub's own answer and the data directory.
pub fn gather(settings: &LifecycleSettings, backend: &dyn Backend) -> Status {
    let holder = lock::inspect(&settings.data_dir).unwrap_or(Holder::Free);
    let (record, held) = match holder {
        Holder::Held(record) => (record, true),
        Holder::Free => (None, false),
    };

    let mut note = None;
    let (state, ready) = if !held {
        if let Some(()) = process::probe_hub(settings.bind) {
            note = Some(format!(
                "another hub answers on {}; a container?",
                lock::display_address(settings.bind)
            ));
        }
        ("stopped", false)
    } else if let Some(mcp) = record.as_ref().filter(|record| record.mode == Mode::Mcp) {
        // An embedded session has no listener, so there is nothing to probe.
        note = Some(format!(
            "an embedded agent-hub mcp session (pid {}) holds the store; it is not a hub",
            mcp.pid
        ));
        ("running", false)
    } else {
        // Only the address the holder recorded is asked. With no current
        // record (a hub still starting, or a restore holding the lock) there is
        // no hub to ask yet: whatever answers on the configured port may be
        // another process.
        match record.as_ref().and_then(process::probe_record) {
            Some((200, _)) => ("running", true),
            Some(_) => ("running", false),
            None => ("starting", false),
        }
    };

    let url = match record.as_ref().and_then(|record| record.url.clone()) {
        Some(url) if held => url,
        _ => format!("http://{}", lock::display_address(settings.bind)),
    };
    let managed = backend.managed(settings, record.as_ref());
    Status {
        version: env!("CARGO_PKG_VERSION").to_string(),
        state,
        ready,
        record,
        managed,
        url,
        data_dir: settings.data_dir.clone(),
        data_bytes: dir_bytes(&settings.data_dir),
        config: settings.config_files.first().cloned(),
        log: settings.log_file.clone(),
        note,
    }
}

impl Status {
    /// 0 running and ready, 1 running and not ready, 3 not running.
    pub fn exit_code(&self) -> u8 {
        match (self.state, self.ready) {
            (_, true) => 0,
            ("stopped", _) => 3,
            _ => 1,
        }
    }

    pub fn to_json(&self) -> String {
        let record = self.record.as_ref();
        json!({
            "version": self.version,
            "state": self.state,
            "ready": self.ready,
            "pid": record.map(|record| record.pid),
            "started_at": record.map(|record| record.started_at.as_str()),
            "managed_by": self.managed.kind,
            "enabled": self.managed.enabled,
            "starts_at": self.managed.starts_at,
            "url": self.url,
            "data_dir": self.data_dir,
            "data_bytes": self.data_bytes,
            "config": self.config,
            "log": self.log,
        })
        .to_string()
    }

    /// The block `status` prints, with paths under `home` shown as `~/...`.
    pub fn render(&self, home: Option<&str>) -> String {
        let mut state = match (self.state, self.ready) {
            (_, true) => "running, ready".to_string(),
            ("running", false) => "running, not ready".to_string(),
            ("starting", _) => "starting".to_string(),
            (other, _) => other.to_string(),
        };
        if let Some(record) = &self.record {
            let up = uptime(&record.started_at);
            match up {
                Some(up) => state.push_str(&format!(" (pid {}, up {up})", record.pid)),
                None => state.push_str(&format!(" (pid {})", record.pid)),
            }
        }
        let data = match self.data_bytes {
            Some(bytes) => format!("{} ({})", tilde(&self.data_dir, home), human_bytes(bytes)),
            None => tilde(&self.data_dir, home),
        };
        let config = match &self.config {
            Some(path) => tilde(path, home),
            None => "none found (defaults and the environment)".to_string(),
        };
        let mut out = format!("agent-hub {}\n", self.version);
        let mut row = |label: &str, value: &str| {
            out.push_str(&format!("  {label:<12} {value}\n"));
        };
        row("state", &state);
        row("managed by", &self.managed.label);
        row("address", &format!("{}/", self.url));
        row("data", &data);
        row("config", &config);
        row("logs", "agent-hub logs");
        if let Some(note) = &self.note {
            row("note", note);
        }
        out
    }
}

/// The size of every regular file under `dir`, or `None` when it is absent.
/// The lock file and its record at the top are not stored data and are not
/// counted.
fn dir_bytes(dir: &Path) -> Option<u64> {
    fn walk(dir: &Path, top: bool) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path(), false),
                Ok(_) if top && agent_hub::ops::restore::is_process_file(&entry.file_name()) => 0,
                Ok(kind) if kind.is_file() => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    dir.is_dir().then(|| walk(dir, true))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// How long ago an RFC 3339 time was, as `3h12m`, `5m` or `42s`.
fn uptime(started_at: &str) -> Option<String> {
    let started =
        time::OffsetDateTime::parse(started_at, &time::format_description::well_known::Rfc3339)
            .ok()?;
    let seconds = (time::OffsetDateTime::now_utc() - started)
        .whole_seconds()
        .max(0);
    Some(match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86399 => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d{}h", seconds / 86400, seconds % 86400 / 3600),
    })
}

/// A path with the home directory shown as `~`.
fn tilde(path: &Path, home: Option<&str>) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &'static str, ready: bool, record: Option<Record>) -> Status {
        Status {
            version: "1.13.0".into(),
            state,
            ready,
            record,
            managed: Managed {
                kind: "none",
                label: "nothing (no service installed)".into(),
                enabled: false,
                starts_at: "manual",
            },
            url: "http://127.0.0.1:8080".into(),
            data_dir: "/hubhome/.local/share/agent-hub".into(),
            data_bytes: Some(412 * 1024 * 1024),
            config: None,
            log: "/hubhome/.local/state/agent-hub/hub.log".into(),
            note: None,
        }
    }

    #[test]
    fn the_exit_code_follows_the_state() {
        assert_eq!(status("running", true, None).exit_code(), 0);
        assert_eq!(status("running", false, None).exit_code(), 1);
        assert_eq!(status("starting", false, None).exit_code(), 1);
        assert_eq!(status("stopped", false, None).exit_code(), 3);
    }

    #[test]
    fn the_stopped_block_says_so_and_shortens_the_home() {
        let text = status("stopped", false, None).render(Some("/hubhome"));
        assert!(text.starts_with("agent-hub 1.13.0\n"), "{text}");
        assert!(text.contains("state        stopped\n"), "{text}");
        assert!(
            text.contains("data         ~/.local/share/agent-hub (412 MiB)"),
            "{text}"
        );
        assert!(
            text.contains("address      http://127.0.0.1:8080/"),
            "{text}"
        );
    }

    #[test]
    fn the_json_carries_every_documented_key() {
        let value: serde_json::Value =
            serde_json::from_str(&status("stopped", false, None).to_json()).unwrap();
        for key in [
            "version",
            "state",
            "ready",
            "pid",
            "started_at",
            "managed_by",
            "enabled",
            "starts_at",
            "url",
            "data_dir",
            "data_bytes",
            "config",
            "log",
        ] {
            assert!(value.get(key).is_some(), "missing {key}: {value}");
        }
        assert_eq!(value["state"], "stopped");
        assert_eq!(value["pid"], serde_json::Value::Null);
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(human_bytes(10), "10 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(412 * 1024 * 1024), "412 MiB");
    }

    #[test]
    fn the_size_walk_does_not_count_the_lock_file_or_its_record() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("daemon-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("kb")).expect("create");
        std::fs::write(root.join("hub.lock"), vec![0u8; 300]).expect("lock");
        std::fs::write(root.join("hub.pid.json"), vec![0u8; 200]).expect("record");
        assert_eq!(
            dir_bytes(&root),
            Some(0),
            "a directory holding only the lock and its record holds no data"
        );
        std::fs::write(root.join("hub.db"), vec![0u8; 100]).expect("db");
        std::fs::write(root.join("kb").join("hub.lock"), vec![0u8; 7]).expect("nested");
        assert_eq!(
            dir_bytes(&root),
            Some(107),
            "only the top-level lock and record are skipped"
        );
        std::fs::remove_dir_all(&root).expect("clean up");
    }
}
