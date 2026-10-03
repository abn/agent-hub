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
//! The IO here is blocking, so every operation runs it on the Tokio blocking
//! pool through [`blocking`]. A transfer up to the artifact cap holds a
//! blocking thread, not an async worker, so the runtime stays free to answer
//! other requests while a large blob moves. On a current-thread runtime (the
//! test harness) there is no other worker to hand off to and no concurrent
//! caller, so the call runs inline.

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
    let data_dir = data_dir.to_path_buf();
    let bytes = bytes.to_vec();
    blocking(move || -> Result<String> {
        write_at(&data_dir, &rel, &bytes)?;
        Ok(rel)
    })
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
    let data_dir = data_dir.to_path_buf();
    let bytes = bytes.to_vec();
    blocking(move || -> Result<String> {
        write_at(&data_dir, &rel, &bytes)?;
        Ok(rel)
    })
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
    blocking(move || -> Result<String> {
        std::fs::rename(from, to)?;
        Ok(rel)
    })
}

/// Read a blob by its relative path.
pub fn read(data_dir: &Path, rel: &str) -> Result<Vec<u8>> {
    let path = resolve(data_dir, rel)?;
    blocking(move || -> Result<Vec<u8>> { Ok(std::fs::read(path)?) })
}

/// Read a blob by its relative path, reporting an absent file as a not-found
/// naming `label` rather than as an IO fault.
///
/// A committed row whose file is gone is an inconsistent store, which `check`
/// reports; the read that hits it names what is missing and says so plainly
/// instead of surfacing an internal error a caller could only retry.
pub fn read_named(data_dir: &Path, rel: &str, label: &str) -> Result<Vec<u8>> {
    let label = label.to_string();
    let path = resolve(data_dir, rel)?;
    blocking(move || -> Result<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound(
                format!("{label} is not on the data volume; the store is inconsistent"),
            )),
            Err(err) => Err(err.into()),
        }
    })
}

/// Remove a blob. A missing blob is not an error.
pub fn remove(data_dir: &Path, rel: &str) -> Result<()> {
    let path = resolve(data_dir, rel)?;
    blocking(move || -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    })
}

/// Remove a directory tree under the data directory.
///
/// Used for a whole project's artifacts, where every version's blob lives
/// under one directory. A tree that is already absent is not an error.
pub fn remove_tree(data_dir: &Path, rel: &str) -> Result<()> {
    let path = resolve(data_dir, rel)?;
    blocking(move || -> Result<()> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    })
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

/// Reconcile on-disk version and pending files against committed artifact versions,
/// removing any unreferenced orphan files left by interrupted writes or crashes.
pub async fn reconcile(db: &turso::Database, data_dir: &Path) -> Result<usize> {
    let conn = crate::store::connect(db)?;
    let mut rows = conn
        .query("SELECT path FROM artifact_versions", ())
        .await
        .map_err(|err| Error::Engine(err.to_string()))?;
    let mut committed = std::collections::HashSet::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| Error::Engine(err.to_string()))?
    {
        if let turso::Value::Text(p) = row
            .get_value(0)
            .map_err(|err| Error::Engine(err.to_string()))?
        {
            committed.insert(p);
        }
    }

    let mut removed = 0;
    for project in child_dirs(&data_dir.join("artifacts"))? {
        let project_name = match project.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        for artifact in child_dirs(&project)? {
            let artifact_name = match artifact.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            for entry in std::fs::read_dir(&artifact)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let file_name = match entry.file_name().into_string() {
                    Ok(name) => name,
                    Err(_) => continue,
                };
                let is_pending = file_name.starts_with(PENDING_PREFIX);
                let rel = format!("artifacts/{project_name}/{artifact_name}/{file_name}");
                if is_pending || (is_version_file(&file_name) && !committed.contains(&rel)) {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
            if is_dir_empty(&artifact)? {
                let _ = std::fs::remove_dir(&artifact);
            }
        }
        if is_dir_empty(&project)? {
            let _ = std::fs::remove_dir(&project);
        }
    }
    Ok(removed)
}

fn is_version_file(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('v') else {
        return false;
    };
    let Some((ver, _ext)) = rest.split_once('.') else {
        return false;
    };
    !ver.is_empty() && ver.chars().all(|ch| ch.is_ascii_digit())
}

fn is_dir_empty(path: &Path) -> Result<bool> {
    match std::fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(err) => Err(err.into()),
    }
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

/// Run a blocking closure off the async worker.
///
/// The serving runtime is multi-threaded, so the closure is submitted to the
/// Tokio blocking pool and the worker is handed off while it runs: the runtime
/// keeps answering other requests during a transfer up to the artifact cap. A
/// current-thread runtime (the test harness) has no other worker to hand off
/// to and no concurrent caller, so the closure runs inline. Every argument the
/// closure reads is cloned to an owned value first, so it is `Send + 'static`
/// and no borrow of the caller's data crosses the await.
fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return f();
    };
    if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
        return f();
    }
    tokio::task::block_in_place(|| match handle.block_on(tokio::task::spawn_blocking(f)) {
        Ok(result) => result,
        Err(err) => Err(Error::Engine(format!("blocking task failed: {err}"))),
    })
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
