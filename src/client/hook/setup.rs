//! `agent-hub setup`: install the hub's plugin into a harness.
//!
//! The plugin files are embedded in the binary, so an install needs nothing
//! from the network and the files always match the build that wrote them.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

/// What `agent-hub setup` takes on its command line.
pub const SETUP_USAGE: &str = "\
usage:
  agent-hub setup antigravity [--workspace DIR]
                                 install the hub plugin into Antigravity

  --workspace DIR  install into DIR/.agents/plugins instead of the user's
                   Antigravity plugin directory

The plugin runs its hooks and its MCP server through the launcher it installs
in bin/, which fetches the agent-hub binary on first use when none matching
this version is on PATH. It reaches the hub through config.toml, which
agent-hub enrol writes.
";

const PLUGIN_NAME: &str = "agent-hub";

/// One embedded file: its path under the plugin directory and its bytes.
struct Embedded {
    rel: &'static str,
    bytes: &'static [u8],
    /// Installed mode 755, as the harness runs it.
    executable: bool,
}

macro_rules! embed {
    ($rel:literal, $path:literal) => {
        Embedded {
            rel: $rel,
            bytes: include_bytes!($path),
            executable: false,
        }
    };
    ($rel:literal, $path:literal, executable) => {
        Embedded {
            rel: $rel,
            bytes: include_bytes!($path),
            executable: true,
        }
    };
}

/// The plugin's MCP config. Its `command` is a path the install fills in, so
/// the file is written from this text and is not in [`PLUGIN_FILES`].
const MCP_CONFIG: &str = include_str!("../../../plugins/antigravity/mcp_config.json");
const MCP_CONFIG_REL: &str = "mcp_config.json";
const LAUNCHER_REL: &str = "bin/agent-hub-launcher";

/// Everything the release archive carries: `plugins/antigravity` less its
/// README, and the whole of `skills/agent-hub`. The `skill_tree_is_installed`
/// test walks the skill directory, so a file added there and missing here fails.
const PLUGIN_FILES: &[Embedded] = &[
    embed!("plugin.json", "../../../plugins/antigravity/plugin.json"),
    embed!("hooks.json", "../../../plugins/antigravity/hooks.json"),
    embed!(
        "bin/agent-hub-launcher",
        "../../../plugins/antigravity/bin/agent-hub-launcher",
        executable
    ),
    embed!(
        "rules/agent-hub.md",
        "../../../plugins/antigravity/rules/agent-hub.md"
    ),
    embed!("logo.png", "../../../plugins/antigravity/logo.png"),
    embed!(
        "skills/agent-hub/SKILL.md",
        "../../../skills/agent-hub/SKILL.md"
    ),
    embed!(
        "skills/agent-hub/bootstrap.md",
        "../../../skills/agent-hub/bootstrap.md"
    ),
    embed!(
        "skills/agent-hub/references/artifacts.md",
        "../../../skills/agent-hub/references/artifacts.md"
    ),
    embed!(
        "skills/agent-hub/references/errors.md",
        "../../../skills/agent-hub/references/errors.md"
    ),
    embed!(
        "skills/agent-hub/references/feed-inbox.md",
        "../../../skills/agent-hub/references/feed-inbox.md"
    ),
    embed!(
        "skills/agent-hub/references/harness.md",
        "../../../skills/agent-hub/references/harness.md"
    ),
    embed!(
        "skills/agent-hub/references/knowledge-base.md",
        "../../../skills/agent-hub/references/knowledge-base.md"
    ),
    embed!(
        "skills/agent-hub/references/notifications.md",
        "../../../skills/agent-hub/references/notifications.md"
    ),
    embed!(
        "skills/agent-hub/references/sessions.md",
        "../../../skills/agent-hub/references/sessions.md"
    ),
    embed!(
        "skills/agent-hub/references/subscriptions.md",
        "../../../skills/agent-hub/references/subscriptions.md"
    ),
    embed!(
        "skills/agent-hub/references/tools.md",
        "../../../skills/agent-hub/references/tools.md"
    ),
    embed!(
        "skills/agent-hub/references/urls.md",
        "../../../skills/agent-hub/references/urls.md"
    ),
];

/// Run `agent-hub setup` and return its exit code.
pub fn run_setup(args: &[String]) -> u8 {
    let Some((target, rest)) = args.split_first() else {
        eprintln!("agent-hub setup: a target is needed\n{SETUP_USAGE}");
        return 2;
    };
    if target != "antigravity" {
        eprintln!("agent-hub setup: unknown target '{target}'\n{SETUP_USAGE}");
        return 2;
    }
    let mut workspace: Option<PathBuf> = None;
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        match (argument.as_str(), rest.next()) {
            ("--workspace", Some(dir)) if workspace.is_none() => {
                workspace = Some(PathBuf::from(dir));
            }
            _ => {
                eprintln!("agent-hub setup: unexpected argument '{argument}'\n{SETUP_USAGE}");
                return 2;
            }
        }
    }
    let Some(home) = home_os(&|key| std::env::var_os(key)) else {
        eprintln!("agent-hub setup: {}", home_missing());
        return 1;
    };
    match install_antigravity(Path::new(&home), workspace.as_deref()) {
        Ok(report) => {
            for line in report {
                println!("{line}");
            }
            println!("Restart Antigravity to load the plugin.");
            0
        }
        Err(message) => {
            eprintln!("agent-hub setup: {message}");
            1
        }
    }
}

/// The home directory as an OS string, so a home that is not UTF-8 still
/// works. The variables read are `HOME_VARS`.
fn home_os(get: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    super::state::HOME_VARS
        .iter()
        .find_map(|key| get(key).filter(|home| !home.is_empty()))
}

fn home_missing() -> &'static str {
    if cfg!(windows) {
        "neither HOME nor USERPROFILE is set"
    } else {
        "HOME is not set"
    }
}

/// Write the plugin and enable it. Returns the lines to show the operator.
///
/// Files that already hold the right bytes are left alone, and a file this
/// replaces is copied to a fresh backup directory first, so a second run
/// changes nothing and no run loses work. Every path is checked for symlinks
/// below its base before it is read, backed up or written.
fn install_antigravity(home: &Path, workspace: Option<&Path>) -> Result<Vec<String>, String> {
    let user_config = home.join(".gemini").join("config");
    let config_backup_root = user_config.join("agent-hub-backup");
    let (base, plugins_dir, backup_root) = match workspace {
        Some(dir) => (
            dir,
            dir.join(".agents").join("plugins"),
            dir.join(".agents").join("agent-hub-backup"),
        ),
        None => (
            home,
            user_config.join("plugins"),
            config_backup_root.clone(),
        ),
    };
    let plugin_dir = plugins_dir.join(PLUGIN_NAME);
    refuse_symlinks(base, &plugin_dir)?;
    let launcher = std::path::absolute(plugin_dir.join(LAUNCHER_REL))
        .map_err(|err| format!("resolve {}: {err}", plugin_dir.display()))?;
    let mcp_config = mcp_config_for(&launcher)?;
    let mut backup = Backup::new(base, backup_root);
    // The global config is never copied into a workspace.
    let mut config_backup = Backup::new(home, config_backup_root);

    let mut report = Vec::new();
    let mut written = 0;
    let mcp_target = plugin_dir.join(MCP_CONFIG_REL);
    if write_if_changed(
        base,
        &mcp_target,
        mcp_config.as_bytes(),
        &mut backup,
        Path::new(MCP_CONFIG_REL),
    )? {
        written += 1;
    }
    for file in PLUGIN_FILES {
        let target = plugin_dir.join(file.rel);
        if write_if_changed(base, &target, file.bytes, &mut backup, Path::new(file.rel))? {
            written += 1;
        }
        if file.executable {
            make_executable(&target)?;
        }
    }
    report.push(format!(
        "plugin: {} ({written} of {} files written)",
        plugin_dir.display(),
        PLUGIN_FILES.len() + 1
    ));
    report.push(format!("launcher: {}", launcher.display()));

    let config_path = user_config.join("config.json");
    let changed = enable_plugin(home, &config_path, &mut config_backup)?;
    report.push(format!(
        "config: {} ({})",
        config_path.display(),
        if changed {
            "plugin enabled"
        } else {
            "plugin already enabled"
        }
    ));
    Ok(report)
}

/// The plugin's MCP config with `command` set to the installed launcher's
/// absolute path. Antigravity expands no variable in a command it runs, so the
/// path is written out.
fn mcp_config_for(launcher: &Path) -> Result<String, String> {
    let mut config: Value = serde_json::from_str(MCP_CONFIG)
        .map_err(|err| format!("the embedded {MCP_CONFIG_REL} is not JSON: {err}"))?;
    let command = launcher
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", launcher.display()))?;
    let server = config
        .pointer_mut(&format!("/mcpServers/{PLUGIN_NAME}"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("the embedded {MCP_CONFIG_REL} has no {PLUGIN_NAME} server"))?;
    server.insert("command".to_string(), json!(command));
    let mut text = serde_json::to_string_pretty(&config)
        .map_err(|err| format!("encode {MCP_CONFIG_REL}: {err}"))?;
    text.push('\n');
    Ok(text)
}

/// Set mode 755 on a file this install wrote, which an archive or a copy may
/// have left without the execute bit.
fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::symlink_metadata(path)
            .map_err(|err| format!("inspect {}: {err}", path.display()))?;
        if meta.permissions().mode() & 0o777 != 0o755 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .map_err(|err| format!("chmod {}: {err}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Refuse when any component of `path` below `base` is a symlink. A component
/// that does not exist ends the walk, since nothing below it can be one.
fn refuse_symlinks(base: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(base)
        .map_err(|_| format!("{} is outside {}", path.display(), base.display()))?;
    let mut current = base.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink; remove it or install into a real directory",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("inspect {}: {err}", current.display())),
        }
    }
    Ok(())
}

/// Read `path` only when it is a regular file. `None` means it does not
/// exist; a directory, a device or anything else is refused.
fn read_regular(base: &Path, path: &Path) -> Result<Option<Vec<u8>>, String> {
    refuse_symlinks(base, path)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => fs::read(path)
            .map(Some)
            .map_err(|err| format!("read {}: {err}", path.display())),
        Ok(_) => Err(format!("{} is not a regular file", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("inspect {}: {err}", path.display())),
    }
}

/// A backup directory made on first use, fresh for each run.
struct Backup {
    base: PathBuf,
    root: PathBuf,
    dir: Option<PathBuf>,
}

impl Backup {
    fn new(base: &Path, root: PathBuf) -> Self {
        Self {
            base: base.to_path_buf(),
            root,
            dir: None,
        }
    }

    fn save(&mut self, relative: &Path, bytes: &[u8]) -> Result<(), String> {
        let dir = match &self.dir {
            Some(dir) => dir.clone(),
            None => {
                refuse_symlinks(&self.base, &self.root)?;
                let parent = self.root.parent().unwrap_or(&self.root);
                fs::create_dir_all(parent)
                    .map_err(|err| format!("create {}: {err}", parent.display()))?;
                let _ = fs::create_dir(&self.root);
                refuse_symlinks(&self.base, &self.root)?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|elapsed| format!("{}-{:09}", elapsed.as_secs(), elapsed.subsec_nanos()))
                    .unwrap_or_default();
                let dir = self.root.join(stamp);
                fs::create_dir(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
                self.dir = Some(dir.clone());
                dir
            }
        };
        let target = dir.join(relative);
        create_parent(&target)?;
        fs::write(&target, bytes).map_err(|err| format!("back up to {}: {err}", target.display()))
    }
}

/// Write `bytes` to `target` unless it already holds them. An existing file
/// with other content is copied to the backup first. Returns whether it wrote.
fn write_if_changed(
    base: &Path,
    target: &Path,
    bytes: &[u8],
    backup: &mut Backup,
    relative: &Path,
) -> Result<bool, String> {
    match read_regular(base, target)? {
        Some(existing) if existing == bytes => return Ok(false),
        Some(existing) => backup.save(relative, &existing)?,
        None => {}
    }
    replace_file(target, bytes)?;
    Ok(true)
}

/// Set `plugins.agent-hub.enabled` to true in the Antigravity config, keeping
/// every other key. A config that is not a JSON object is left untouched.
fn enable_plugin(home: &Path, config_path: &Path, backup: &mut Backup) -> Result<bool, String> {
    let existing = read_regular(home, config_path)?;
    let mut config = match &existing {
        Some(bytes) => match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(map)) => map,
            _ => {
                return Err(format!(
                    "{} is not a JSON object; fix or remove it and run setup again",
                    config_path.display()
                ));
            }
        },
        None => Map::new(),
    };
    let plugins = config
        .entry("plugins")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(plugins) = plugins else {
        return Err(format!(
            "{}: \"plugins\" is not an object; fix it and run setup again",
            config_path.display()
        ));
    };
    let entry = plugins.entry(PLUGIN_NAME).or_insert_with(|| json!({}));
    let Value::Object(entry) = entry else {
        return Err(format!(
            "{}: \"plugins.{PLUGIN_NAME}\" is not an object; fix it and run setup again",
            config_path.display()
        ));
    };
    if entry.get("enabled") == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    entry.insert("enabled".to_owned(), Value::Bool(true));

    if let Some(bytes) = existing {
        backup.save(Path::new("config.json"), &bytes)?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(config))
        .map_err(|err| format!("encode {}: {err}", config_path.display()))?;
    text.push('\n');
    replace_file(config_path, text.as_bytes())?;
    Ok(true)
}

fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => {
            fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))
        }
        None => Ok(()),
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create `path` exclusively, never following a symlink, and write `bytes`.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("create {}: {err}", path.display()))?;
    let result = file
        .write_all(bytes)
        .map_err(|err| format!("write {}: {err}", path.display()));
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

/// Write through a sibling temporary file and rename it into place, so a
/// reader never sees half a file.
fn replace_file(target: &Path, bytes: &[u8]) -> Result<(), String> {
    create_parent(target)?;
    let name = target
        .components()
        .rfind(|part| matches!(part, Component::Normal(_)))
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let temporary = target.with_file_name(format!(
        ".{name}.{}-{nanos}-{unique}.agent-hub-tmp",
        std::process::id()
    ));
    write_new(&temporary, bytes)?;
    fs::rename(&temporary, target).map_err(|err| {
        let _ = fs::remove_file(&temporary);
        format!("write {}: {err}", target.display())
    })
}

#[cfg(test)]
mod tests {
    use super::home_os;
    use std::ffi::OsString;

    #[test]
    fn home_is_read_as_an_os_string_and_empty_is_unset() {
        let get = |key: &str| (key == "HOME").then(|| OsString::from("/users/a"));
        assert_eq!(home_os(&get), Some(OsString::from("/users/a")));
        let empty = |key: &str| (key == "HOME").then(OsString::new);
        assert_eq!(home_os(&empty), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_home_that_is_not_utf8_is_accepted_and_userprofile_is_not_read() {
        use super::home_missing;
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(b"/users/\xff".to_vec());
        let get = |key: &str| (key == "HOME").then(|| raw.clone());
        assert_eq!(home_os(&get), Some(raw.clone()));
        let profile_only = |key: &str| (key == "USERPROFILE").then(|| OsString::from("/p"));
        assert_eq!(home_os(&profile_only), None);
        assert_eq!(home_missing(), "HOME is not set");
    }
}
