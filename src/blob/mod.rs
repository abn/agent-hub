//! Artifact blobs on the data volume.
//!
//! Blobs live at `artifacts/<project>/<artifact>/v<version>.<ext>`, relative
//! to the data directory. The path column stores that relative path.
//!
//! An update only learns its version number under the store's write lock, so
//! its content lands beside the versions under a `pending-<token>` name and is
//! renamed into place once the number is allocated. A pending file that never
//! made it that far is inert: no row points at it, and it is swept at the next
//! startup.
//!
//! The IO here is blocking and is called from async handlers. At the artifact
//! cap and the single-operator scale this is accepted: the worst case is one
//! Tokio worker stalled for the duration of a large transfer. Revisit with
//! `spawn_blocking` or `tokio::fs` if the node ever serves concurrent large
//! transfers.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Name prefix of content whose version number is not yet allocated.
const PENDING_PREFIX: &str = "pending-";

/// The relative path of a blob, without writing it.
pub fn blob_path(project_id: &str, artifact_id: &str, version: i64, kind: &str) -> Result<String> {
    let ext = extension(kind)?;
    Ok(format!(
        "artifacts/{project_id}/{artifact_id}/v{version}.{ext}"
    ))
}

/// Write a blob under the data directory and return its relative path.
pub fn write(
    data_dir: &Path,
    project_id: &str,
    artifact_id: &str,
    version: i64,
    kind: &str,
    bytes: &[u8],
) -> Result<String> {
    crate::limits::check_artifact(bytes.len())?;
    let rel = blob_path(project_id, artifact_id, version, kind)?;
    write_at(data_dir, &rel, bytes)?;
    Ok(rel)
}

/// Write a blob under a pending name and return its relative path.
///
/// The name is unique to the call, so a writer that does not yet know its
/// version number cannot collide with another writer or with a committed
/// version.
pub fn write_pending(
    data_dir: &Path,
    project_id: &str,
    artifact_id: &str,
    kind: &str,
    bytes: &[u8],
) -> Result<String> {
    crate::limits::check_artifact(bytes.len())?;
    let ext = extension(kind)?;
    let token = ulid::Ulid::generate();
    let rel = format!("artifacts/{project_id}/{artifact_id}/{PENDING_PREFIX}{token}.{ext}");
    write_at(data_dir, &rel, bytes)?;
    Ok(rel)
}

/// Rename a pending blob onto its version path and return that path.
///
/// Both names sit in the same directory, so the rename is one cheap atomic
/// step and is safe to run under the write lock. It replaces an orphan left
/// at that version path by an earlier write that never committed.
pub fn promote(
    data_dir: &Path,
    pending: &str,
    project_id: &str,
    artifact_id: &str,
    version: i64,
    kind: &str,
) -> Result<String> {
    let rel = blob_path(project_id, artifact_id, version, kind)?;
    let from = resolve(data_dir, pending)?;
    let to = resolve(data_dir, &rel)?;
    std::fs::rename(from, to)?;
    Ok(rel)
}

/// Read a blob by its relative path.
pub fn read(data_dir: &Path, rel: &str) -> Result<Vec<u8>> {
    let path = resolve(data_dir, rel)?;
    Ok(std::fs::read(path)?)
}

/// Remove a blob. A missing blob is not an error.
pub fn remove(data_dir: &Path, rel: &str) -> Result<()> {
    let path = resolve(data_dir, rel)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Remove a directory tree under the data directory.
///
/// Used for a whole project's artifacts, where every version's blob lives
/// under one directory. A tree that is already absent is not an error.
pub fn remove_tree(data_dir: &Path, rel: &str) -> Result<()> {
    let path = resolve(data_dir, rel)?;
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Remove every pending blob under the data directory, returning the count.
///
/// A pending name lives only between an update's write and its rename, both in
/// one process. Content whose rename never ran is referenced by nothing, is
/// counted by no storage figure, and only a delete of the whole artifact or
/// project would ever clear it. Called at startup, where no update is in
/// flight here and the engine's exclusive lock rules out a second hub over the
/// same directory, so every pending file on disk is dead and no age threshold
/// is needed.
pub fn reap_pending(data_dir: &Path) -> Result<usize> {
    let mut removed = 0;
    for project in child_dirs(&data_dir.join("artifacts"))? {
        for artifact in child_dirs(&project)? {
            for entry in std::fs::read_dir(&artifact)? {
                let entry = entry?;
                let pending = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(PENDING_PREFIX));
                if pending && entry.file_type()?.is_file() {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

/// The directories directly under `path`. A symlink is not one of them, and a
/// missing directory yields none.
fn child_dirs(path: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

fn write_at(data_dir: &Path, rel: &str, bytes: &[u8]) -> Result<()> {
    let path = resolve(data_dir, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, bytes)?;
    Ok(())
}

fn extension(kind: &str) -> Result<&'static str> {
    match kind {
        "html" => Ok("html"),
        "markdown" => Ok("md"),
        other => Err(Error::InvalidArgument(format!(
            "unknown artifact kind '{other}'"
        ))),
    }
}

/// Resolve a relative blob path, refusing anything that escapes the root.
fn resolve(data_dir: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.contains("..") || Path::new(rel).is_absolute() {
        return Err(Error::InvalidArgument(format!(
            "blob path '{rel}' is not a safe relative path"
        )));
    }
    Ok(data_dir.join(rel))
}
