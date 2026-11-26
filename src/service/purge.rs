//! What `uninstall --purge` is allowed to delete, and the deleting.
//!
//! Purge never deletes a directory by its own path. In the data directory it
//! removes the entries the hub itself creates (the store and its sidecars,
//! the lock and its record, and the hub's own subdirectories), then removes the
//! directory only if that left it empty. Anything else stays and is named. The
//! same holds for the state directory and the logs: known names only, regular
//! files only.
//!
//! Every guard is checked before anything is removed, and the data
//! directory's lock is held exclusively for the whole removal, so a hub cannot
//! start into a directory that is being emptied. A bind mount nested inside one
//! of the hub's own subdirectories is not looked for: the removal would empty
//! it. That is an accepted risk; nothing the hub writes creates one.

use std::ffi::OsStr;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use agent_hub::config::{Config, Os};
use agent_hub::limits::{EventCeiling, InboxCaps};
use agent_hub::ops::restore::{HUB_LOCK, HUB_RECORD, lock_store_file};

use crate::daemon::lock::{self, Holder};
use crate::daemon::logfile;

/// The names a state directory may have, so a stray `XDG_STATE_HOME` cannot
/// point the removal at a directory that is not ours.
const STATE_NAMES: [&str; 3] = ["agent-hub", "agent-hub-state", "state"];

/// The previous binary the install script keeps, and the prefix of the
/// temporary name it writes it under first.
const PREVIOUS_BINARY: &str = "agent-hub.prev";
const PREVIOUS_BINARY_TEMP: &str = ".agent-hub.prev.";

/// Where the previous binary and the install receipt live: the XDG state
/// directory, or the platform equivalent on macOS and Windows.
pub fn state_dir(env: &dyn Fn(&str) -> Option<String>, os: Os) -> Option<PathBuf> {
    let absolute = |key: &str| {
        agent_hub::config::present(env(key))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    match os {
        Os::Linux => absolute("XDG_STATE_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".local").join("state")))
            .map(|base| base.join("agent-hub")),
        Os::Macos => absolute("HOME").map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("agent-hub-state")
        }),
        Os::Windows => absolute("LOCALAPPDATA").map(|base| base.join("agent-hub").join("state")),
    }
}

/// Paths a data directory to be purged may not hold, with what each one is.
#[derive(Default)]
pub struct Protected {
    pub home: Option<PathBuf>,
    pub paths: Vec<(&'static str, PathBuf)>,
}

/// A directory that may not be touched, whatever it holds.
fn refuse_dangerous(dir: &Path, home: Option<&Path>, what: &str) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err(format!(
            "the {what} {} is not an absolute path; refusing to guess what it means",
            dir.display()
        ));
    }
    if dir.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "the {what} {} holds '..'; name it by its real path",
            dir.display()
        ));
    }
    if dir.parent().is_none() {
        return Err(format!("the {what} is the filesystem root"));
    }
    // Every component is looked at without following it: a symlink anywhere on
    // the way could send the removal somewhere else.
    let mut walked = PathBuf::new();
    for part in dir.components() {
        walked.push(part);
        if walked.parent().is_none() {
            continue;
        }
        match std::fs::symlink_metadata(&walked) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(if walked == dir {
                    format!(
                        "the {what} {} is a symlink; remove it by hand if that is what you mean",
                        dir.display()
                    )
                } else {
                    format!(
                        "the {what} {} goes through a symlink at {}; set it to the real path \
                         in config.toml to purge it",
                        dir.display(),
                        walked.display()
                    )
                });
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => {
                return Err(format!(
                    "could not look at {} on the way to the {what}: {err}",
                    walked.display()
                ));
            }
        }
    }
    let canonical = std::fs::canonicalize(dir).ok();
    if let Some(home) = home {
        let canonical_home = std::fs::canonicalize(home).ok();
        let is_home = dir == home || (canonical.is_some() && canonical == canonical_home);
        let holds_home = home.starts_with(dir)
            || matches!((&canonical_home, &canonical), (Some(h), Some(c)) if h.starts_with(c));
        if is_home || holds_home {
            return Err(format!(
                "the {what} {} is, or holds, your home directory",
                dir.display()
            ));
        }
    }
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "could not look at the {what} {}: {err}",
                dir.display()
            ));
        }
    };
    if !metadata.is_dir() {
        return Err(format!("the {what} {} is not a directory", dir.display()));
    }
    #[cfg(unix)]
    if let Some(parent) = dir.parent()
        && let Ok(above) = std::fs::metadata(parent)
    {
        use std::os::unix::fs::MetadataExt;
        if above.dev() != metadata.dev() {
            return Err(format!(
                "the {what} {} is a mount point; unmount it or empty it by hand",
                dir.display()
            ));
        }
    }
    Ok(())
}

/// The entries the hub itself creates in its data directory, named by the
/// same helpers the hub opens them with. The lock comes last.
fn hub_entries(dir: &Path) -> Vec<PathBuf> {
    let config = Config {
        data_dir: dir.to_path_buf(),
        bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        public_url: None,
        admin_token: None,
        inbox_caps: InboxCaps::disabled(),
        events_per_project: EventCeiling::disabled(),
        active_window: Duration::from_secs(900),
        node_name: None,
        enrol_enabled: false,
        enrol_pending_max: 0,
        enrol_pending_ttl: Duration::ZERO,
        trusted_proxies: Vec::new(),
        notify: None,
        notify_delivery: Default::default(),
        backup_dir: None,
    };
    let store = config.hub_db_path();
    let sidecar = |suffix: &str| {
        let mut name = store.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    vec![
        sidecar("-wal"),
        sidecar("-shm"),
        store.clone(),
        config.sessions_dir(),
        config.knowledge_dir(),
        config.artifacts_dir(),
        // The tailnet's device state, the copies taken before a migration and
        // the session brains an upgrade merged away have no helper of their
        // own; the hub creates all three here.
        dir.join("tailnet"),
        dir.join("backups"),
        dir.join("merged"),
        dir.join(HUB_RECORD),
        dir.join(HUB_LOCK),
    ]
}

/// The identity of a directory, so the one removed is the one checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity(u64, u64);

fn identity(dir: &Path) -> Option<Identity> {
    let meta = std::fs::symlink_metadata(dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(Identity(meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Some(Identity(0, 0))
    }
}

/// A data directory that passed every guard.
#[derive(Debug)]
pub struct DataPlan {
    pub dir: PathBuf,
    /// The size of what would go: the hub's own entries only.
    pub bytes: u64,
    identity: Identity,
}

/// Check that the hub's entries in `dir` are safe to delete.
///
/// `Ok(None)` means there is nothing there. Otherwise the directory holds
/// `hub.db` or nothing but the lock and its record, is not a symlink and is
/// reached through none, is not the root or the home directory or above it,
/// and holds none of the protected paths. Anything else is a refusal with the
/// reason.
pub fn check_data_dir(dir: &Path, protected: &Protected) -> Result<Option<DataPlan>, String> {
    refuse_dangerous(dir, protected.home.as_deref(), "data directory")?;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("could not read {}: {err}", dir.display())),
    };
    let real = std::fs::canonicalize(dir)
        .map_err(|err| format!("could not resolve {}: {err}", dir.display()))?;
    for (what, path) in &protected.paths {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if path.starts_with(&real) {
            return Err(format!(
                "the data directory {} holds {what} ({}); move one of them first",
                dir.display(),
                path.display()
            ));
        }
    }
    let mut has_db = false;
    let mut other = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == OsStr::new(agent_hub::ops::HUB_DB)
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            has_db = true;
        } else if !agent_hub::ops::restore::is_process_file(&name) {
            other = true;
        }
    }
    if !has_db && other {
        return Err(format!(
            "{} holds files but no hub.db, so it does not look like a hub data directory",
            dir.display()
        ));
    }
    let identity = identity(dir).ok_or_else(|| format!("could not look at {}", dir.display()))?;
    let bytes = hub_entries(dir).iter().map(|path| entry_bytes(path)).sum();
    Ok(Some(DataPlan {
        dir: dir.to_path_buf(),
        bytes,
        identity,
    }))
}

/// The size of a file, or of every regular file under a directory; a symlink
/// is not followed.
fn entry_bytes(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry_bytes(&entry.path()))
                    .sum()
            })
            .unwrap_or(0),
        Ok(meta) if meta.is_file() => meta.len(),
        _ => 0,
    }
}

/// Take the store's lock for the whole removal. Refused when a hub holds it.
pub fn lock_data_dir(dir: &Path) -> Result<File, String> {
    match lock_store_file(&lock::path_in(dir), Duration::from_secs(2)) {
        Ok(Some(file)) => Ok(file),
        Ok(None) => {
            let record = match lock::inspect(dir) {
                Ok(Holder::Held(record)) => record,
                _ => None,
            };
            Err(format!(
                "{}; nothing was deleted",
                lock::held_message(dir, record.as_ref())
            ))
        }
        Err(err) => Err(format!("could not lock {}: {err}", dir.display())),
    }
}

/// Remove one entry without following it: a symlink is unlinked, a directory
/// is removed with what it holds (a symlink inside is unlinked, not followed).
fn remove_entry(path: &Path) -> Result<bool, String> {
    let removed = match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("could not look at {}: {err}", path.display())),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    };
    match removed {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("could not delete {}: {err}", path.display())),
    }
}

/// What is left in a directory, by name.
fn left_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| dir.join(entry.file_name()).display().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Delete the hub's entries in the data directory under the lock taken by
/// [`lock_data_dir`], then the directory if that emptied it. The guards run
/// again, and the directory must still be the one checked. Returns what was
/// left because it is not the hub's.
pub fn remove_data_dir(
    plan: &DataPlan,
    protected: &Protected,
    held: File,
) -> Result<Vec<String>, String> {
    let again = check_data_dir(&plan.dir, protected)?
        .ok_or_else(|| format!("{} is gone", plan.dir.display()))?;
    if again.identity != plan.identity {
        return Err(format!(
            "{} was replaced after it was checked; nothing was deleted",
            plan.dir.display()
        ));
    }
    let lock_path = lock::path_in(&plan.dir);
    for entry in hub_entries(&plan.dir) {
        if entry != lock_path {
            remove_entry(&entry)?;
        }
    }
    // Windows cannot delete a file that is open; elsewhere the lock is held
    // until its file is gone.
    #[cfg(windows)]
    drop(held);
    let removed = remove_entry(&lock_path);
    #[cfg(not(windows))]
    drop(held);
    removed?;
    match std::fs::remove_dir(&plan.dir) {
        Ok(()) => Ok(Vec::new()),
        Err(_) => Ok(left_in(&plan.dir)),
    }
}

/// Every file the log occupies: the live file, its rotations and its startup
/// file.
pub fn log_files(log_file: &Path) -> Vec<PathBuf> {
    let mut files = vec![log_file.to_path_buf()];
    files.extend((1..=logfile::KEEP).map(|index| logfile::numbered(log_file, index)));
    files.push(logfile::startup_path(log_file));
    files
}

/// What a removal of named files did.
#[derive(Debug, Default)]
pub struct Removed {
    pub removed: Vec<PathBuf>,
    /// Paths left because they are not regular files, or not ours.
    pub left: Vec<String>,
    pub problems: Vec<String>,
}

/// Delete each of `files` that is a regular file. A symlink, a directory or
/// anything else at one of those names is left and reported.
pub fn remove_regular_files(files: &[PathBuf]) -> Removed {
    let mut outcome = Removed::default();
    for file in files {
        match std::fs::symlink_metadata(file) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => outcome
                .problems
                .push(format!("could not look at {}: {err}", file.display())),
            Ok(meta) if meta.file_type().is_file() => match std::fs::remove_file(file) {
                Ok(()) => outcome.removed.push(file.clone()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => outcome
                    .problems
                    .push(format!("could not remove {}: {err}", file.display())),
            },
            Ok(_) => outcome
                .left
                .push(format!("{} (not a regular file)", file.display())),
        }
    }
    outcome
}

/// Check a directory the install keeps for itself (state, logs).
pub fn check_state_dir(dir: &Path, home: Option<&Path>) -> Result<bool, String> {
    refuse_dangerous(dir, home, "state directory")?;
    if cfg!(unix) {
        let named = dir
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| STATE_NAMES.contains(&name));
        if !named {
            return Err(format!(
                "the state directory {} is not named agent-hub; refusing to delete it",
                dir.display()
            ));
        }
    }
    Ok(dir.is_dir())
}

/// Delete the files the install keeps in the state directory (the previous
/// binary and its temporary copies), then the directory if that emptied it.
pub fn remove_state_dir(dir: &Path, home: Option<&Path>) -> Result<Removed, String> {
    if !check_state_dir(dir, home)? {
        return Ok(Removed::default());
    }
    let known: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|err| format!("could not read {}: {err}", dir.display()))?
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| {
            name.to_str().is_some_and(|name| {
                name == PREVIOUS_BINARY || name.starts_with(PREVIOUS_BINARY_TEMP)
            })
        })
        .map(|name| dir.join(name))
        .collect();
    let mut outcome = remove_regular_files(&known);
    match std::fs::remove_dir(dir) {
        Ok(()) => outcome.removed.push(dir.to_path_buf()),
        Err(_) => outcome.left.extend(left_in(dir)),
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let root = PathBuf::from("target/tmp");
        std::fs::create_dir_all(&root).expect("create the scratch root");
        let dir = std::fs::canonicalize(root)
            .expect("scratch root")
            .join(format!("purge-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    fn open() -> Protected {
        Protected::default()
    }

    fn home(home: &Path) -> Protected {
        Protected {
            home: Some(home.to_path_buf()),
            paths: Vec::new(),
        }
    }

    #[test]
    fn a_relative_directory_is_refused() {
        let err = check_data_dir(Path::new("data"), &open()).expect_err("refused");
        assert!(err.contains("not an absolute path"), "{err}");
    }

    #[test]
    fn the_root_and_the_home_directory_are_refused() {
        assert!(check_data_dir(Path::new("/"), &open()).is_err());
        let dir = scratch("home");
        std::fs::write(dir.join("hub.db"), "x").unwrap();
        let err = check_data_dir(&dir, &home(&dir)).expect_err("home");
        assert!(err.contains("home directory"), "{err}");
        let parent = dir.parent().unwrap().to_path_buf();
        let err = check_data_dir(&parent, &home(&dir)).expect_err("a directory that holds home");
        assert!(err.contains("home directory"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_refused_and_its_target_is_untouched() {
        let dir = scratch("link");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("hub.db"), "x").unwrap();
        let link = dir.join("data");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = check_data_dir(&link, &open()).expect_err("symlink");
        assert!(err.contains("symlink"), "{err}");
        assert!(real.join("hub.db").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_on_the_way_to_the_directory_is_refused() {
        let dir = scratch("via");
        let real = dir.join("real");
        std::fs::create_dir_all(real.join("data")).unwrap();
        std::fs::write(real.join("data").join("hub.db"), "x").unwrap();
        std::os::unix::fs::symlink(&real, dir.join("via")).unwrap();
        let err = check_data_dir(&dir.join("via").join("data"), &open()).expect_err("refused");
        assert!(err.contains("goes through a symlink"), "{err}");
        assert!(real.join("data").join("hub.db").exists());
        let err = check_data_dir(&real.join("..").join("real").join("data"), &open())
            .expect_err("a path with .. is refused");
        assert!(err.contains("'..'"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_that_holds_a_protected_path_is_refused() {
        let dir = scratch("protected");
        std::fs::write(dir.join("hub.db"), "x").unwrap();
        std::fs::write(dir.join("config.toml"), "[hub]\n").unwrap();
        let protected = Protected {
            home: None,
            paths: vec![("the config file", dir.join("config.toml"))],
        };
        let err = check_data_dir(&dir, &protected).expect_err("refused");
        assert!(err.contains("holds the config file"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_without_hub_db_is_refused_unless_it_is_empty() {
        let dir = scratch("nodb");
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        let err = check_data_dir(&dir, &open()).expect_err("not a data directory");
        assert!(err.contains("no hub.db"), "{err}");

        let empty = scratch("empty");
        std::fs::write(empty.join("hub.lock"), "").unwrap();
        std::fs::write(empty.join("hub.pid.json"), "{}").unwrap();
        assert_eq!(check_data_dir(&empty, &open()).unwrap().unwrap().bytes, 2);

        let hub = scratch("hub");
        std::fs::write(hub.join("hub.db"), vec![0u8; 100]).unwrap();
        std::fs::create_dir_all(hub.join("sessions")).unwrap();
        std::fs::write(hub.join("sessions").join("a.db"), vec![0u8; 50]).unwrap();
        std::fs::write(hub.join("notes.txt"), vec![0u8; 1000]).unwrap();
        assert_eq!(
            check_data_dir(&hub, &open()).unwrap().unwrap().bytes,
            150,
            "only the hub's own entries count"
        );

        let gone = hub.join("missing");
        assert!(check_data_dir(&gone, &open()).unwrap().is_none());
        for path in [dir, empty, hub] {
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn a_held_lock_refuses_and_nothing_is_deleted() {
        let dir = scratch("held");
        std::fs::write(dir.join("hub.db"), "x").unwrap();
        let holder = lock_data_dir(&dir).expect("the first lock");
        let err = lock_data_dir(&dir).expect_err("a second is refused");
        assert!(err.contains("nothing was deleted"), "{err}");
        assert!(dir.join("hub.db").exists());
        drop(holder);
        let plan = check_data_dir(&dir, &open()).unwrap().unwrap();
        let again = lock_data_dir(&dir).expect("free again");
        let left = remove_data_dir(&plan, &open(), again).expect("removed under the lock");
        assert!(left.is_empty(), "{left:?}");
        assert!(!dir.exists());
    }

    #[test]
    fn only_the_hubs_own_entries_go_and_the_rest_is_named() {
        let dir = scratch("mixed");
        let data = dir.join("data");
        for sub in [
            "sessions",
            "kb",
            "artifacts",
            "tailnet",
            "backups",
            "merged",
        ] {
            std::fs::create_dir_all(data.join(sub)).unwrap();
            std::fs::write(data.join(sub).join("f"), "x").unwrap();
        }
        for file in ["hub.db", "hub.db-wal", "hub.db-shm", "hub.pid.json"] {
            std::fs::write(data.join(file), "x").unwrap();
        }
        std::fs::write(data.join("notes.txt"), "mine").unwrap();
        std::fs::create_dir_all(dir.join("sibling")).unwrap();
        std::fs::write(dir.join("sibling").join("keep"), "keep").unwrap();
        let plan = check_data_dir(&data, &open()).unwrap().unwrap();
        let held = lock_data_dir(&data).unwrap();
        let left = remove_data_dir(&plan, &open(), held).expect("removed");
        assert_eq!(left, vec![data.join("notes.txt").display().to_string()]);
        let names: Vec<_> = std::fs::read_dir(&data)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("notes.txt")]);
        assert!(dir.join("sibling").join("keep").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_replaced_after_the_check_is_not_touched() {
        let dir = scratch("swap");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("hub.db"), "x").unwrap();
        let plan = check_data_dir(&data, &open()).unwrap().unwrap();
        std::fs::rename(&data, dir.join("old")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("hub.db"), "other").unwrap();
        let held = lock_data_dir(&data).unwrap();
        let err = remove_data_dir(&plan, &open(), held).expect_err("refused");
        assert!(err.contains("replaced"), "{err}");
        assert_eq!(
            std::fs::read_to_string(data.join("hub.db")).unwrap(),
            "other"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_a_hub_directory_is_unlinked_not_followed() {
        let dir = scratch("inner");
        let outside = scratch("outside");
        std::fs::write(outside.join("keep.txt"), "keep").unwrap();
        std::fs::write(dir.join("hub.db"), "x").unwrap();
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("sessions").join("escape")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("artifacts")).unwrap();
        let plan = check_data_dir(&dir, &open()).unwrap().unwrap();
        let held = lock_data_dir(&dir).unwrap();
        remove_data_dir(&plan, &open(), held).expect("removed");
        assert!(!dir.exists());
        assert!(
            outside.join("keep.txt").exists(),
            "the link's target survives"
        );
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn the_state_directory_is_only_ever_one_of_ours_and_keeps_what_is_not() {
        let dir = scratch("state");
        let err = check_state_dir(&dir, None).expect_err("not named for the hub");
        assert!(err.contains("not named agent-hub"), "{err}");
        let ours = dir.join("agent-hub");
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(ours.join("agent-hub.prev"), "old").unwrap();
        std::fs::write(ours.join(".agent-hub.prev.Ab12"), "half").unwrap();
        let removed = remove_state_dir(&ours, None).expect("removed");
        assert!(removed.left.is_empty(), "{:?}", removed.left);
        assert!(!ours.exists());

        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(ours.join("agent-hub.prev"), "old").unwrap();
        std::fs::write(ours.join("mine.txt"), "mine").unwrap();
        let removed = remove_state_dir(&ours, None).expect("removed");
        assert!(!ours.join("agent-hub.prev").exists());
        assert!(ours.join("mine.txt").exists());
        assert_eq!(
            removed.left,
            vec![ours.join("mine.txt").display().to_string()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn only_regular_files_at_the_log_names_are_removed() {
        let dir = scratch("logs");
        let outside = scratch("logs-outside");
        std::fs::write(outside.join("precious"), "keep").unwrap();
        let log = dir.join("hub.log");
        std::fs::write(&log, "a").unwrap();
        std::fs::write(logfile::numbered(&log, 1), "b").unwrap();
        std::os::unix::fs::symlink(outside.join("precious"), logfile::numbered(&log, 2)).unwrap();
        let outcome = remove_regular_files(&log_files(&log));
        assert_eq!(outcome.removed.len(), 2, "{outcome:?}");
        assert_eq!(outcome.left.len(), 1, "{outcome:?}");
        assert!(outside.join("precious").exists());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn the_state_dir_follows_the_platform() {
        let env = |key: &str| match key {
            "HOME" => Some("/hubhome/u".to_string()),
            _ => None,
        };
        assert_eq!(
            state_dir(&env, Os::Linux),
            Some(PathBuf::from("/hubhome/u/.local/state/agent-hub"))
        );
        assert_eq!(
            state_dir(&env, Os::Macos),
            Some(PathBuf::from(
                "/hubhome/u/Library/Application Support/agent-hub-state"
            ))
        );
    }

    #[test]
    fn the_log_set_is_the_file_its_rotations_and_the_startup_file() {
        let files = log_files(Path::new("/x/hub.log"));
        let names: Vec<_> = files
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "/x/hub.log",
                "/x/hub.log.1",
                "/x/hub.log.2",
                "/x/hub.log.3",
                "/x/hub.log.start"
            ]
        );
    }
}
