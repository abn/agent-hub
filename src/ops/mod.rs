//! Offline data-lifecycle commands: backup, restore, and check.
//!
//! These commands run against the store on disk while no hub serves it. A
//! running hub holds the engine's exclusive file lock, so every command here
//! opens the store the same way the hub does and refuses with one clear message
//! when the lock is held; the online alternative is a filesystem snapshot of a
//! stopped or quiesced volume.
//!
//! The set a backup holds is the whole store: `hub.db`, every session brain
//! and project knowledge file under `sessions/` and `kb/`, and every artifact
//! blob under `artifacts/`. Engine files are copied through the engine's own
//! `VACUUM INTO`, so the result is one consistent file with no sidecar; only
//! when the engine refuses that is a byte copy of the file and its write-ahead
//! log taken instead, and the report says so.

pub mod backup;
pub mod check;
pub mod manifest;
pub mod restore;

use std::path::{Path, PathBuf};

pub use backup::{BackupReport, backup};
pub use check::{CheckReport, check};
pub use manifest::{Entry, Manifest};
pub use restore::{RestoreReport, restore};

use crate::error::{Error, Result};

/// The manifest a backup writes and restore and check read back.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The hub store's file name under the data directory.
pub const HUB_DB: &str = "hub.db";

/// The engine sidecars a byte-copied store file may have beside it.
pub(crate) const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// The one refusal every command gives when a hub holds the store.
pub(crate) fn locked() -> Error {
    Error::Conflict(
        "a hub is using this data directory; stop it, or snapshot the volume, before running \
         this offline command"
            .to_string(),
    )
}

/// Open the hub store and map a held lock to the shared refusal.
///
/// The engine's exclusive file lock is held by a serving hub, so the open, or
/// the first query behind it, is where a running hub is detected.
pub(crate) async fn open_hub_store(hub_db: &Path) -> Result<turso::Database> {
    match crate::store::open_engine(hub_db).await {
        Ok(db) => Ok(db),
        Err(err) if err.is_locked() => Err(locked()),
        Err(err) => Err(err),
    }
}

/// Map a held lock surfacing later (a query or a copy) to the shared refusal.
pub(crate) fn map_locked(err: Error) -> Error {
    if err.is_locked() { locked() } else { err }
}

/// Every regular file under `root`, recursively, in a stable order.
///
/// A missing root yields nothing: an empty store is copied as an empty tree,
/// not refused. Symlinks are skipped, since a blob is a regular file.
pub(crate) fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    if !root.exists() {
        return Ok(found);
    }
    walk(root, &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk(&entry.path(), found)?;
        } else if file_type.is_file() {
            found.push(entry.path());
        }
    }
    Ok(())
}

/// The `/`-joined path of `path` under `root`, as a manifest records it.
pub(crate) fn relative(root: &Path, path: &Path) -> Result<String> {
    let rel = path.strip_prefix(root).map_err(|_| {
        Error::Engine(format!(
            "{} is not under {}",
            path.display(),
            root.display()
        ))
    })?;
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
            _ => {
                return Err(Error::Engine(format!(
                    "unexpected path component in {}",
                    path.display()
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(Error::Engine(format!(
            "{} is not a file under {}",
            path.display(),
            root.display()
        )));
    }
    Ok(parts.join("/"))
}

/// Resolve a manifest's relative path under a root, refusing an escape.
pub(crate) fn join_rel(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.contains("..") || rel.starts_with('/') || rel.contains('\\') {
        return Err(Error::InvalidArgument(format!(
            "'{rel}' is not a safe relative path"
        )));
    }
    Ok(root.join(rel))
}

/// The size of a file in bytes.
pub(crate) fn file_size(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

/// The lowercase-hex sha256 of a file's bytes.
pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Copy one file, creating its parent directories, then harden the result.
pub(crate) fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to)?;
    harden_file(to);
    Ok(())
}

/// Restrict a file to its owner, ignoring a filesystem that will not.
pub(crate) fn harden_file(path: &Path) {
    if let Err(err) = crate::config::private_file(path) {
        tracing::warn!(path = %path.display(), error = %err, "could not restrict file permissions");
    }
}

/// A store file's path with a sidecar suffix appended, such as `hub.db-wal`.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Run `work` against an opened store, leaving no sidecar it did not already
/// have.
///
/// Opening an engine can create an empty write-ahead log and shared-memory file
/// beside the database. A read that does not need them, `check` above all, must
/// not write into what it reads, so a sidecar that was absent before the open is
/// removed once the handle is dropped.
pub(crate) async fn with_engine<T>(
    path: &Path,
    work: impl AsyncFnOnce(&turso::Database) -> Result<T>,
) -> Result<T> {
    let sidecars: Vec<PathBuf> = SIDECAR_SUFFIXES
        .iter()
        .map(|suffix| with_suffix(path, suffix))
        .collect();
    let existed: Vec<bool> = sidecars.iter().map(|side| side.exists()).collect();

    let outcome = async {
        let db = open_hub_store(path).await?;
        work(&db).await
    }
    .await;

    // The handle is dropped by now, so a sidecar this open created can go.
    for (side, existed) in sidecars.iter().zip(existed) {
        if !existed {
            let _ = std::fs::remove_file(side);
        }
    }
    outcome
}

/// Whether a directory exists and holds at least one entry.
pub(crate) fn dir_nonempty(path: &Path) -> Result<bool> {
    match std::fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}
