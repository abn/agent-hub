//! Artifact blobs on the data volume.
//!
//! Blobs live at `artifacts/<project>/<artifact>/v<version>.<ext>`, relative
//! to the data directory. The path column stores that relative path.
//!
//! An update only learns its version number under the store's write lock, so
//! its content lands beside the versions under a `pending-<token>` name and is
//! renamed into place once the number is allocated. A pending file that never
//! made it that far is inert: no row points at it, and it goes with the tree
//! when the artifact or the project is deleted.
//!
//! The IO here is blocking and is called from async handlers. At the artifact
//! cap and the single-operator scale this is accepted: the worst case is one
//! Tokio worker stalled for the duration of a large transfer. Revisit with
//! `spawn_blocking` or `tokio::fs` if the node ever serves concurrent large
//! transfers.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

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
    let rel = format!("artifacts/{project_id}/{artifact_id}/pending-{token}.{ext}");
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
