//! `service install`, `service uninstall` and `uninstall`.

use std::io::{BufRead, IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use agent_hub::config::{
    HubRemoval, LifecycleSettings, Merged, Os, load_config_file, merge_missing_settings,
    platform_data_dir, platform_log_file, present, remove_hub_table, user_config_target,
};
use rand::RngCore;

use super::backend::ServiceBackend;
use super::{Job, Manager, UnitSpec, Want, host_manager, pinned_env, process_env, purge, tilde};
use crate::daemon::lock::{self, Holder};
use crate::daemon::{Backend, StopOptions};

pub const SERVICE_USAGE: &str = "\
usage:
  agent-hub service install [--bind ADDR] [--data-dir DIR] [--boot | --login]
                            [--no-start] [--yes] [--force]
  agent-hub service uninstall [--yes]
  agent-hub service run --config PATH

install registers the hub as a per-user service and starts it: a systemd user
unit on Linux, a launchd LaunchAgent on macOS. It writes the unit, creates
config.toml with the settings the service needs (bind, data_dir, a generated
admin token, and the client url) and never changes a setting the file already
holds: a --bind or --data-dir that disagrees with the file is refused. Running
it again is safe; it says what it kept. --boot (the Linux default) turns on
linger so the hub starts with the machine and falls back to login when the
system refuses; --login leaves linger alone. macOS starts at login only.
--no-start installs without starting. --yes is accepted so a script can pass
it. --force installs over a container install's Quadlet.

uninstall stops the hub and removes the service. The binary, the config and the
data stay. Linger is left as it was found.

run is what the Windows scheduled task executes; it has no use elsewhere.

Exits 69 when there is no service manager to reach (no systemd user instance,
no launchd session, Windows for now), 2 for a usage error or a refusal that
needs another flag, and 78 for an invalid configuration.
";

pub const UNINSTALL_USAGE: &str = "\
usage:
  agent-hub uninstall [--purge] [--keep-binary] [--yes]

Stop the hub, remove the service and delete the binary and the previous binary
kept for a rollback. The config and the data stay.

--purge also deletes the hub's data, its logs, the previous binary and the
[hub] table of config.toml (the file goes only when nothing else is in it). The
[client] table stays: agents and plugins on this machine use it. Only what the
hub itself writes is deleted: a directory is removed only once that leaves it
empty, and anything else in it is named and kept. The paths come from the
installed service's config file, never from this shell's HUB_* settings.
Before it deletes anything, --purge refuses a data directory that is the
filesystem root or the home directory, a symlink or reached through one, a
mount point, one holding no hub.db, one holding the config file, the binary or
the service definition, or one a hub still holds. A container install is never
touched.

--keep-binary leaves the binary. Without a terminal, --yes is required: this
exits 2 and removes nothing.
";

/// Why a verb stopped, with the exit code it ends with.
pub struct Failure {
    pub code: u8,
    pub message: String,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn usage_error(command: &str, message: &str, usage: &str) -> ExitCode {
    eprintln!("agent-hub {command}: {message}\n{usage}");
    ExitCode::from(2)
}

fn report(command: &str, failure: &Failure) -> ExitCode {
    eprintln!("agent-hub {command}: {}", failure.message);
    ExitCode::from(failure.code)
}

fn no_manager(command: &str) -> ExitCode {
    eprintln!(
        "agent-hub {command}: there is no service manager to drive on this system{}. \
         Run the hub with: agent-hub start",
        if cfg!(windows) {
            " yet (the Windows service is a separate piece of work)"
        } else {
            ""
        }
    );
    ExitCode::from(69)
}

/// `agent-hub service ...`.
pub fn service_cmd(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("install") => install_cmd(&args[1..]),
        Some("uninstall") => service_uninstall_cmd(&args[1..]),
        Some("run") => run_cmd(),
        Some(other) => usage_error(
            "service",
            &format!("unknown subcommand '{other}'"),
            SERVICE_USAGE,
        ),
        None => usage_error("service", "a subcommand is required", SERVICE_USAGE),
    }
}

/// The Windows supervisor's entry. The seam is here; the supervisor is not.
fn run_cmd() -> ExitCode {
    if cfg!(windows) {
        eprintln!("agent-hub service run: the Windows supervisor is not available yet");
        ExitCode::from(69)
    } else {
        eprintln!(
            "agent-hub service run: this is what the Windows scheduled task runs; it does nothing here"
        );
        ExitCode::from(2)
    }
}

struct InstallArgs {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    want: Want,
    no_start: bool,
    force: bool,
}

fn parse_install(args: &[String]) -> Result<InstallArgs, String> {
    let mut parsed = InstallArgs {
        bind: None,
        data_dir: None,
        want: Want::Default,
        no_start: false,
        force: false,
    };
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (argument.as_str(), None),
        };
        let mut value = |flag: &str| {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match name {
            "--bind" => parsed.bind = Some(value("--bind")?),
            "--data-dir" => {
                let dir = PathBuf::from(value("--data-dir")?);
                parsed.data_dir = Some(
                    std::path::absolute(&dir)
                        .map_err(|err| format!("--data-dir {}: {err}", dir.display()))?,
                );
            }
            "--boot" | "--login" => {
                let want = if name == "--boot" {
                    Want::Boot
                } else {
                    Want::Login
                };
                if parsed.want != Want::Default && parsed.want != want {
                    return Err("--boot and --login exclude each other".to_string());
                }
                parsed.want = want;
            }
            "--no-start" => parsed.no_start = true,
            "--yes" | "-y" => {}
            "--force" => parsed.force = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(parsed)
}

/// 32 random bytes as 64 hex characters.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn mask(token: &str) -> String {
    let head: String = token.chars().take(4).collect();
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}...{tail}")
}

fn install_cmd(args: &[String]) -> ExitCode {
    let parsed = match parse_install(args) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error("service install", &message, SERVICE_USAGE),
    };
    let Some(manager) = host_manager(&process_env) else {
        return no_manager("service install");
    };
    match install(&process_env, manager, &parsed) {
        Ok(code) => code,
        Err(failure) => report("service install", &failure),
    }
}

fn config_failure(err: agent_hub::Error) -> Failure {
    Failure::new(78, err.to_string())
}

fn install(
    env: &dyn Fn(&str) -> Option<String>,
    manager: Box<dyn Manager>,
    args: &InstallArgs,
) -> Result<ExitCode, Failure> {
    let os = manager.os();
    let home = present(env("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| Failure::new(78, "HOME is not set, so there is nowhere to install to"))?;
    let config_path = user_config_target(env)
        .ok_or_else(|| Failure::new(78, "no config location: set HUB_CONFIG or HOME"))?;
    let config_path = std::path::absolute(&config_path)
        .map_err(|err| Failure::new(78, format!("{}: {err}", config_path.display())))?;
    let config_existed = config_path.is_file();
    let existing = load_config_file(&config_path).map_err(config_failure)?;
    let file_hub = existing.as_ref().map(|file| &file.hub);
    let file_value = |key: &str| file_hub.and_then(|hub| hub.get(key)).cloned();

    // A setting the file already holds is the service's. A flag that disagrees
    // is a mistake to be told about, not silently dropped or applied.
    let bind_text = match (&args.bind, file_value("bind")) {
        (Some(flag), Some(file)) => {
            let same = flag.parse::<SocketAddr>().ok() == file.parse::<SocketAddr>().ok();
            if !same {
                return Err(Failure::new(
                    2,
                    format!(
                        "{} already sets bind = \"{file}\", which differs from --bind {flag}; \
                         edit the file to change it. Nothing was changed",
                        config_path.display()
                    ),
                ));
            }
            file
        }
        (Some(flag), None) => flag.clone(),
        (None, Some(file)) => file,
        (None, None) => "127.0.0.1:8080".to_string(),
    };
    let bind: SocketAddr = bind_text.parse().map_err(|err| {
        Failure::new(
            2,
            format!("the bind address '{bind_text}' is not valid: {err}"),
        )
    })?;
    let data_dir = match (&args.data_dir, file_value("data_dir")) {
        (Some(flag), Some(file)) => {
            if Path::new(&file) != flag.as_path() {
                return Err(Failure::new(
                    2,
                    format!(
                        "{} already sets data_dir = \"{file}\", which differs from --data-dir {}; \
                         edit the file to change it. Nothing was changed",
                        config_path.display(),
                        flag.display()
                    ),
                ));
            }
            flag.clone()
        }
        (Some(flag), None) => flag.clone(),
        (None, Some(file)) => PathBuf::from(file),
        (None, None) => platform_data_dir(env, os).ok_or_else(|| {
            Failure::new(78, "no data directory: set HUB_DATA_DIR or pass --data-dir")
        })?,
    };
    if !data_dir.is_absolute() {
        return Err(Failure::new(
            78,
            format!(
                "data_dir {} must be an absolute path for a service",
                data_dir.display()
            ),
        ));
    }

    if os == Os::Linux && !args.force {
        let quadlet = present(env("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
            .join("containers")
            .join("systemd")
            .join("agent-hub.container");
        if quadlet.exists() {
            return Err(Failure::new(
                1,
                format!(
                    "a container install is set up at {}: the unit it generates is also called \
                     agent-hub.service, and ours would shadow it silently. Migrate it (see the \
                     native service guide), or re-run with --force to install anyway",
                    quadlet.display()
                ),
            ));
        }
    }

    // A free lock means no hub is running here, so whatever answers on the
    // port is somebody else.
    if matches!(lock::inspect(&data_dir), Ok(Holder::Free) | Err(_))
        && let Some(problem) = super::port_problem(bind, &data_dir)
    {
        return Err(Failure::new(1, problem));
    }

    let prepared = manager.prepare(args.want).map_err(|unavailable| Failure {
        code: unavailable.code,
        message: unavailable.message,
    })?;

    // A token the hub would already resolve (the environment, then the config
    // layers) is kept, not replaced. The service reads only the pinned file,
    // so a token found anywhere else is carried into it.
    // An empty pinned value is unset, the same as no key.
    let inherited = if file_value("admin_token").is_some_and(|token| !token.is_empty()) {
        None
    } else {
        agent_hub::config::Config::resolve_admin_token(env)
            .map_err(config_failure)?
            .map(|token| {
                let from = if present(env("HUB_ADMIN_TOKEN")).is_some() {
                    "HUB_ADMIN_TOKEN"
                } else {
                    "another config file"
                };
                (token, from)
            })
    };
    let token = match &inherited {
        Some((token, _)) => token.clone(),
        None => new_token(),
    };
    let mut hub_keys: Vec<(&str, String)> = vec![
        ("bind", bind_text.clone()),
        ("data_dir", data_dir.display().to_string()),
        ("admin_token", token.clone()),
    ];
    if os == Os::Macos
        && let Some(log) = platform_log_file(env, Os::Macos)
    {
        hub_keys.push(("log_file", log.display().to_string()));
    }
    let hub_refs: Vec<(&str, &str)> = hub_keys.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let merged = merge_missing_settings(&config_path, "hub", &hub_refs)
        .map_err(|err| Failure::new(78, format!("{}: {err}", config_path.display())))?;
    let url = format!("http://{}", lock::display_address(bind));
    merge_missing_settings(&config_path, "client", &[("url", url.as_str())])
        .map_err(|err| Failure::new(78, format!("{}: {err}", config_path.display())))?;
    let added: Vec<&str> = merged
        .iter()
        .filter(|(_, how)| *how == Merged::Added)
        .map(|(key, _)| key.as_str())
        .collect();
    let token_added = merged
        .iter()
        .any(|(key, how)| key == "admin_token" && *how == Merged::Added);
    let token_created = token_added && inherited.is_none();

    let binary = std::env::current_exe()
        .map_err(|err| Failure::new(1, format!("could not find the agent-hub binary: {err}")))?;
    let spec = UnitSpec {
        binary,
        config: config_path.clone(),
        home: home.clone(),
        starts_at: prepared.starts_at,
    };
    let text = manager
        .render(&spec)
        .map_err(|message| Failure::new(1, message))?;
    let unit_path = manager.unit_path();
    let previous = std::fs::read_to_string(&unit_path).ok();
    let changed = previous.as_deref() != Some(text.as_str());
    if changed {
        super::write_definition(&unit_path, &text).map_err(|err| {
            Failure::new(1, format!("could not write {}: {err}", unit_path.display()))
        })?;
    }
    let register_note = match manager.register(&spec, changed, !args.no_start) {
        Ok(note) => note,
        Err(unavailable) => {
            // Put the old definition back, so a retry sees the new one as a change
            // and loads it again rather than taking it as already loaded.
            if changed {
                let _ = match &previous {
                    Some(text) => super::write_definition(&unit_path, text),
                    None => std::fs::remove_file(&unit_path),
                };
            }
            return Err(Failure::new(unavailable.code, unavailable.message));
        }
    };

    let home_ref = Some(home.as_path());
    let unit_note = match (&previous, changed) {
        (None, _) => "",
        (Some(_), true) => " (updated)",
        (Some(_), false) => " (kept)",
    };
    let config_note = if !config_existed {
        " (created)".to_string()
    } else if added.is_empty() {
        " (kept)".to_string()
    } else {
        format!(" (added {})", added.join(", "))
    };
    let heading = if previous.is_none() {
        "Installed the agent-hub service."
    } else if changed || !added.is_empty() {
        "Updated the agent-hub service."
    } else {
        "The agent-hub service is already installed and up to date."
    };
    let log_hint = manager.log_hint();
    let file_token = existing
        .as_ref()
        .and_then(|file| file.hub.get("admin_token"))
        .filter(|token| !token.is_empty())
        .cloned();
    let shown_token = if token_created {
        Some(token.as_str())
    } else {
        file_token.as_deref()
    };
    let token_row = match (token_created, shown_token, &inherited) {
        (true, Some(shown), _) => format!(
            "{} ({} characters, saved in config.toml)",
            mask(shown),
            shown.len()
        ),
        (false, _, Some((_, from))) if token_added => {
            format!("kept (from {from}, now saved in config.toml)")
        }
        _ => "kept (in config.toml)".to_string(),
    };

    if token_created {
        println!("Admin token (shown once): {token}");
        println!();
    }
    println!("{heading}");
    let row = |label: &str, value: &str| println!("  {label:<12} {value}");
    row(
        "unit",
        &format!("{}{unit_note}", tilde(&unit_path, home_ref)),
    );
    row(
        "config",
        &format!("{}{config_note}", tilde(&config_path, home_ref)),
    );
    row("data", &tilde(&data_dir, home_ref));
    row("logs", &log_hint);
    row("starts at", &prepared.row);
    row("address", &format!("{url}/"));
    row("admin token", &token_row);
    if let Some(advice) = &prepared.advice {
        println!("{advice}");
    }

    let backend = ServiceBackend::new(manager);
    if args.no_start {
        if let Some(note) = &register_note {
            println!("{note}");
        }
        println!("Not started (--no-start). Start it with: agent-hub start");
    } else {
        let env = pinned_env(env, Some(&config_path));
        let settings = LifecycleSettings::resolve(&env, os).map_err(config_failure)?;
        let code = backend.start(&settings);
        if code != ExitCode::SUCCESS {
            eprintln!("The service is installed but the hub did not come up; see agent-hub logs.");
            return Ok(code);
        }
    }
    println!();
    if token_created {
        println!("Open {url}/ and paste the admin token to log in.");
    } else {
        println!("Open {url}/ and paste the admin token from config.toml to log in.");
    }
    println!("Manage it with: agent-hub status | stop | start | logs | uninstall");
    Ok(ExitCode::SUCCESS)
}

/// Whether to ask the human, and how to answer a missing terminal.
fn confirmed_or_refused(yes: bool, verb: &str) -> Result<bool, ExitCode> {
    if yes {
        return Ok(false);
    }
    if std::io::stdin().is_terminal() {
        return Ok(true);
    }
    eprintln!("agent-hub {verb}: refusing to remove without --yes on a non-interactive terminal");
    Err(ExitCode::from(2))
}

/// One answer from the terminal, trimmed. `None` at end of input.
fn ask(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

fn said_yes(answer: Option<String>) -> bool {
    answer.is_some_and(|answer| matches!(answer.to_lowercase().as_str(), "y" | "yes"))
}

fn stop_options() -> StopOptions {
    StopOptions {
        timeout: Duration::from_secs(20),
        force: false,
    }
}

/// The settings the installed service runs with: the config file its
/// definition pins, read with none of the caller's `HUB_*` overrides. With no
/// service, the config file this user's tools read, the same way.
fn installed_settings(
    env: &dyn Fn(&str) -> Option<String>,
    manager: Option<&dyn Manager>,
) -> Result<(LifecycleSettings, Option<PathBuf>), Failure> {
    let installed = manager.filter(|manager| manager.is_installed());
    // A definition that cannot be read is a refusal: the shell's settings are
    // not the service's, and acting on them could remove the wrong store.
    let config = match installed {
        Some(manager) => Some(
            manager
                .pinned_config()
                .map_err(|message| Failure::new(78, format!("{message}. Nothing was removed")))?,
        ),
        None => user_config_target(env),
    }
    .map(|path| std::path::absolute(&path).unwrap_or(path));
    let os = manager.map_or(Os::current(), |manager| manager.os());
    let settings = LifecycleSettings::resolve(&pinned_env(env, config.as_deref()), os)
        .map_err(config_failure)?;
    Ok((settings, config))
}

/// The job is loaded in the manager and its definition is missing, or the
/// loaded job runs with a config file other than the definition's: nothing may
/// be removed, since the binary and the data belong to a job that still runs.
/// `Ok` when the manager's job and the definition agree (or neither exists),
/// and a 69 when the manager cannot be asked.
fn refuse_stray_job(manager: &dyn Manager) -> Result<(), Failure> {
    let job = manager.job().map_err(|unavailable| {
        Failure::new(
            unavailable.code,
            format!("{}. Nothing was removed", unavailable.message),
        )
    })?;
    let Job::Loaded { config } = job else {
        return Ok(());
    };
    let definition = manager.is_installed();
    let differs = if definition {
        match (&config, manager.pinned_config()) {
            (Some(loaded), Ok(defined)) => *loaded != defined,
            _ => false,
        }
    } else {
        true
    };
    if !differs {
        return Ok(());
    }
    let why = if definition {
        format!(
            "it runs with a config file other than the one {} names",
            manager.unit_path().display()
        )
    } else {
        format!(
            "its definition {} is missing",
            manager.unit_path().display()
        )
    };
    Err(Failure::new(
        1,
        format!(
            "the {kind} job {job} is loaded in {kind} but {why}, so the binary, the config and \
             the data are left alone. Unload it first with: {hint}. Nothing was removed",
            kind = manager.kind(),
            job = manager.job_name(),
            hint = manager.unload_hint()
        ),
    ))
}

/// A stop result as a failure that says nothing was removed. A manager that
/// cannot be asked keeps its 69.
fn stopped_or_refused(code: ExitCode) -> Result<(), Failure> {
    if code == ExitCode::SUCCESS {
        Ok(())
    } else if code == ExitCode::from(69) {
        Err(Failure::new(
            69,
            "the service manager does not answer, so nothing was removed",
        ))
    } else {
        Err(Failure::new(
            1,
            "the hub did not stop, so nothing was removed",
        ))
    }
}

fn service_uninstall_cmd(args: &[String]) -> ExitCode {
    let mut yes = false;
    for argument in args {
        match argument.as_str() {
            "--yes" | "-y" => yes = true,
            other => {
                return usage_error(
                    "service uninstall",
                    &format!("unknown option '{other}'"),
                    SERVICE_USAGE,
                );
            }
        }
    }
    let Some(manager) = host_manager(&process_env) else {
        return no_manager("service uninstall");
    };
    if !manager.is_installed() {
        return match refuse_stray_job(manager.as_ref()) {
            Ok(()) => {
                println!("no agent-hub service is installed");
                ExitCode::SUCCESS
            }
            Err(failure) => report("service uninstall", &failure),
        };
    }
    let ask_first = match confirmed_or_refused(yes, "service uninstall") {
        Ok(ask) => ask,
        Err(code) => return code,
    };
    match service_uninstall(&process_env, manager, ask_first) {
        Ok(code) => code,
        Err(failure) => report("service uninstall", &failure),
    }
}

fn service_uninstall(
    env: &dyn Fn(&str) -> Option<String>,
    manager: Box<dyn Manager>,
    ask_first: bool,
) -> Result<ExitCode, Failure> {
    manager.check().map_err(|unavailable| {
        Failure::new(
            unavailable.code,
            format!("{}. Nothing was removed", unavailable.message),
        )
    })?;
    refuse_stray_job(manager.as_ref())?;
    if ask_first {
        println!(
            "This stops the hub and removes {}.",
            manager.unit_path().display()
        );
        println!("The binary, the config and the data stay.");
        if !said_yes(ask("Continue? [y/N] ")) {
            return Err(Failure::new(1, "not confirmed, nothing was removed"));
        }
    }
    let (settings, _) = installed_settings(env, Some(manager.as_ref()))?;
    let backend = ServiceBackend::new(manager);
    stopped_or_refused(backend.stop(&settings, &stop_options()))?;
    let path = backend.manager().unit_path();
    backend.manager().remove().map_err(|unavailable| {
        Failure::new(
            unavailable.code,
            format!("{}; the service was left in place", unavailable.message),
        )
    })?;
    println!("Removed the agent-hub service ({}).", path.display());
    println!("Kept: the binary, the config and the data. agent-hub uninstall removes the rest.");
    Ok(ExitCode::SUCCESS)
}

struct UninstallArgs {
    purge: bool,
    keep_binary: bool,
    yes: bool,
}

pub fn uninstall_cmd(args: &[String]) -> ExitCode {
    let mut parsed = UninstallArgs {
        purge: false,
        keep_binary: false,
        yes: false,
    };
    for argument in args {
        match argument.as_str() {
            "--purge" => parsed.purge = true,
            "--keep-binary" => parsed.keep_binary = true,
            "--yes" | "-y" => parsed.yes = true,
            other => {
                return usage_error(
                    "uninstall",
                    &format!("unknown option '{other}'"),
                    UNINSTALL_USAGE,
                );
            }
        }
    }
    // Before any config is read: a script without --yes learns that first.
    let ask_first = match confirmed_or_refused(parsed.yes, "uninstall") {
        Ok(ask) => ask,
        Err(code) => return code,
    };
    let manager = host_manager(&process_env);
    if manager.is_none() && cfg!(windows) {
        // The Windows service is not here yet, so there is nothing safe to drive.
        return no_manager("uninstall");
    }
    match uninstall(&process_env, manager, ask_first, &parsed) {
        Ok(code) => code,
        Err(failure) => report("uninstall", &failure),
    }
}

fn refusing(message: String) -> Failure {
    Failure::new(
        1,
        format!("refusing to purge: {message}. Nothing was removed"),
    )
}

fn uninstall(
    env: &dyn Fn(&str) -> Option<String>,
    manager: Option<Box<dyn Manager>>,
    ask_first: bool,
    args: &UninstallArgs,
) -> Result<ExitCode, Failure> {
    let os = manager
        .as_ref()
        .map_or(Os::current(), |manager| manager.os());
    let home = present(env("HOME")).map(PathBuf::from);
    let home_ref = home.as_deref();
    let service = manager.as_deref().filter(|manager| manager.is_installed());
    if let Some(service) = service {
        service.check().map_err(|unavailable| {
            Failure::new(
                unavailable.code,
                format!("{}. Nothing was removed", unavailable.message),
            )
        })?;
    }
    // Whether a definition is on disk or not, the manager's own word on the
    // job decides: a loaded job whose definition is gone, or runs elsewhere,
    // keeps its binary and its data.
    if let Some(manager) = manager.as_deref() {
        refuse_stray_job(manager)?;
    }
    // What the installed service runs with is what goes; this shell's own
    // settings name what a process hub started from it uses, which is stopped.
    let (target, config_path) = installed_settings(env, manager.as_deref())?;
    let stop_settings = if service.is_some() {
        target.clone()
    } else {
        LifecycleSettings::resolve(env, os).map_err(config_failure)?
    };
    let binary = std::env::current_exe().ok();
    let protected = purge::Protected {
        home: home.clone(),
        paths: [
            config_path.clone().map(|path| ("the config file", path)),
            binary.clone().map(|path| ("the agent-hub binary", path)),
            manager
                .as_ref()
                .map(|manager| ("the service definition", manager.unit_path())),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };

    // Every guard runs before anything is stopped or removed.
    let data_plan = if args.purge {
        purge::check_data_dir(&target.data_dir, &protected).map_err(refusing)?
    } else {
        None
    };
    let state_dir = purge::state_dir(env, os);
    if args.purge
        && let Some(dir) = &state_dir
    {
        purge::check_state_dir(dir, home_ref).map_err(refusing)?;
    }
    let binary = binary.filter(|_| !args.keep_binary);
    let previous = state_dir
        .as_ref()
        .map(|dir| dir.join("agent-hub.prev"))
        .filter(|path| path.is_file() && !args.keep_binary);

    println!("This removes the agent-hub service and binary:");
    if let Some(service) = service {
        println!("  {}", tilde(&service.unit_path(), home_ref));
    }
    if let Some(binary) = &binary {
        println!("  {}", tilde(binary, home_ref));
    }
    if let Some(previous) = &previous {
        println!("  {}", tilde(previous, home_ref));
    }
    if service.is_none() && binary.is_none() && previous.is_none() {
        println!("  (nothing: no service, and the binary stays)");
    }
    if !args.purge {
        let config = config_path
            .as_ref()
            .map(|path| tilde(path, home_ref))
            .unwrap_or_else(|| "none".to_string());
        println!(
            "Kept: config {config}, data {}",
            tilde(&target.data_dir, home_ref)
        );
    }
    if ask_first {
        if !said_yes(ask("Continue? [y/N] ")) {
            eprintln!("agent-hub uninstall: not confirmed, nothing was removed");
            return Ok(ExitCode::FAILURE);
        }
        if args.purge {
            println!();
            match &data_plan {
                Some(plan) => println!(
                    "--purge also deletes the hub's data in {}: {}.",
                    tilde(&target.data_dir, home_ref),
                    human(plan.bytes)
                ),
                None => println!("--purge also deletes the hub's config table, logs and state."),
            }
            println!("This cannot be undone. Take a backup first with: agent-hub backup --out DIR");
            if ask("Type delete to confirm: ").as_deref() != Some("delete") {
                eprintln!("agent-hub uninstall: not confirmed, nothing was removed");
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    // Stop the hub, whoever runs it, and take the store's lock so no hub can
    // start into a directory about to be emptied.
    let backend = crate::daemon::backend(&stop_settings);
    stopped_or_refused(backend.stop(&stop_settings, &stop_options()))?;
    let held = match &data_plan {
        Some(plan) => {
            Some(purge::lock_data_dir(&plan.dir).map_err(|message| Failure::new(1, message))?)
        }
        None => None,
    };

    // The log file the installed plist names, read before the plist is deleted.
    let manager_logs = service
        .map(|service| service.manager_logs())
        .unwrap_or_default();

    // The service goes first, and nothing else goes unless it did: a job left
    // registered without its binary or its data would fail at every login.
    if let Some(service) = service {
        service.remove().map_err(|unavailable| {
            Failure::new(
                unavailable.code,
                format!(
                    "{}; the service, the binary, the config and the data were left in place",
                    unavailable.message
                ),
            )
        })?;
    }

    let mut problems: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut left: Vec<String> = Vec::new();
    if let Some(service) = service {
        removed.push(format!(
            "the service ({})",
            tilde(&service.unit_path(), home_ref)
        ));
    }
    if let Some(previous) = &previous {
        match std::fs::remove_file(previous) {
            Ok(()) => removed.push(tilde(previous, home_ref)),
            Err(err) => problems.push(format!("could not remove {}: {err}", previous.display())),
        }
    }
    if let Some(binary) = &binary {
        match remove_binary(binary) {
            Ok(note) => removed.push(note.unwrap_or_else(|| tilde(binary, home_ref))),
            Err(message) => problems.push(message),
        }
    }

    if args.purge {
        if let Some(path) = &config_path {
            match remove_hub_table(path) {
                Ok(HubRemoval::TableRemoved) => {
                    removed.push(format!("[hub] in {}", tilde(path, home_ref)));
                }
                Ok(HubRemoval::FileDeleted) => {
                    removed.push(tilde(path, home_ref));
                }
                Ok(HubRemoval::Absent) => {}
                Err(err) => problems.push(format!("{}: {err}", path.display())),
            }
        }
        let mut logs = purge::log_files(&target.log_file);
        logs.extend(manager_logs);
        let merge = |outcome: purge::Removed,
                     removed: &mut Vec<String>,
                     left: &mut Vec<String>,
                     problems: &mut Vec<String>| {
            removed.extend(outcome.removed.iter().map(|path| tilde(path, home_ref)));
            left.extend(outcome.left);
            problems.extend(outcome.problems);
        };
        merge(
            purge::remove_regular_files(&logs),
            &mut removed,
            &mut left,
            &mut problems,
        );
        if !target.log_file_set
            && let Some(dir) = target.log_file.parent()
        {
            // Empty directories only: the default log directory, if nothing
            // else lives there.
            let _ = std::fs::remove_dir(dir);
        }
        if let (Some(plan), Some(held)) = (&data_plan, held) {
            match purge::remove_data_dir(plan, &protected, held) {
                Ok(kept) if kept.is_empty() => removed.push(tilde(&plan.dir, home_ref)),
                Ok(kept) => {
                    removed.push(format!("the hub's data in {}", tilde(&plan.dir, home_ref)));
                    left.extend(kept);
                }
                Err(message) => problems.push(message),
            }
        }
        if let Some(dir) = &state_dir {
            match purge::remove_state_dir(dir, home_ref) {
                Ok(outcome) => merge(outcome, &mut removed, &mut left, &mut problems),
                Err(message) => problems.push(message),
            }
        }
    }

    if !removed.is_empty() {
        println!("Removed:");
        for item in &removed {
            println!("  {item}");
        }
    }
    if !left.is_empty() {
        println!("Left in place (not written by the hub):");
        for item in &left {
            println!("  {item}");
        }
    }
    if !problems.is_empty() {
        eprintln!("Not everything could be removed:");
        for problem in &problems {
            eprintln!("  {problem}");
        }
        return Ok(ExitCode::FAILURE);
    }
    if !args.purge {
        println!(
            "The config and the data were kept. agent-hub uninstall --purge removes them too."
        );
    }
    println!("A container install, its volume and its Quadlet were not touched.");
    Ok(ExitCode::SUCCESS)
}

/// Delete the running binary. `Some(note)` says what was done instead.
fn remove_binary(binary: &Path) -> Result<Option<String>, String> {
    if cfg!(windows) {
        return Ok(Some(format!(
            "{} was left in place: a running Windows binary cannot delete itself yet",
            binary.display()
        )));
    }
    match std::fs::remove_file(binary) {
        Ok(()) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("could not remove {}: {err}", binary.display())),
    }
}

fn human(bytes: u64) -> String {
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
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_64_hex_characters_and_never_repeats() {
        let first = new_token();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, new_token());
    }

    #[test]
    fn the_masked_token_shows_the_two_ends() {
        assert_eq!(mask(&"a".repeat(30)), "aaaa...aaaa");
    }

    #[test]
    fn install_flags_parse_both_forms_and_refuse_the_unknown() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let parsed = parse_install(&args(&[
            "--bind=0.0.0.0:9",
            "--login",
            "--no-start",
            "--yes",
        ]))
        .expect("parse");
        assert_eq!(parsed.bind.as_deref(), Some("0.0.0.0:9"));
        assert_eq!(parsed.want, Want::Login);
        assert!(parsed.no_start);
        assert!(parse_install(&args(&["--boot", "--login"])).is_err());
        assert!(parse_install(&args(&["--bind"])).is_err());
        assert!(parse_install(&args(&["--frobnicate"])).is_err());
    }

    #[test]
    fn the_service_sees_its_pinned_config_and_not_the_shells_overrides() {
        let shell = |key: &str| match key {
            "HUB_BIND" => Some("0.0.0.0:1".to_string()),
            "HUB_DATA_DIR" => Some("/elsewhere".to_string()),
            "HUB_NODE_NAME" => Some("shell".to_string()),
            "HUB_CONFIG" => Some("/shell/config.toml".to_string()),
            "HOME" => Some("/hubhome/u".to_string()),
            _ => None,
        };
        let config = Path::new("/hubhome/u/.config/agent-hub/config.toml");
        let seen = pinned_env(&shell, Some(config));
        assert_eq!(seen("HUB_BIND"), None);
        assert_eq!(seen("HUB_DATA_DIR"), None);
        assert_eq!(seen("HUB_NODE_NAME"), None);
        assert_eq!(
            seen("HUB_CONFIG").as_deref(),
            Some("/hubhome/u/.config/agent-hub/config.toml")
        );
        assert_eq!(seen("HOME").as_deref(), Some("/hubhome/u"));
        let no_file = pinned_env(&shell, None);
        assert_eq!(no_file("HUB_CONFIG").as_deref(), Some("/shell/config.toml"));
        assert_eq!(no_file("HUB_DATA_DIR"), None);
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(human(10), "10 B");
        assert_eq!(human(412 * 1024 * 1024), "412.0 MiB");
    }
}
