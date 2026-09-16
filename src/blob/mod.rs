//! Artifact blobs on the data volume.
//!
//! Blobs live at `artifacts/<project>/<artifact>/v<version>.<ext>`, relative
//! to the data directory. The path column stores that relative path.

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
    let rel = blob_path(project_id, artifact_id, version, kind)?;
    let path = resolve(data_dir, &rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, bytes)?;
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
