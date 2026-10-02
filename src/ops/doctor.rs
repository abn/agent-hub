//! Diagnose a hub store's health in one report.
//!
//! Doctor is the offline "is this store healthy" command. It reads the store
//! and the tree around it without migrating or writing, and answers with the
//! schema version, whether the store is newer than this binary, the data
//! directory's device and inode, the volume's free space, the write-ahead log
//! size, the persisted id high-water mark, and the engine's integrity check
//! with any artifact blob the store names but the tree is missing.
//!
//! A running hub holds the engine's exclusive lock, so doctor refuses with the
//! same message as the other offline commands. A store too broken to open is a
//! failure, not a panic.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::check::check;
use super::{HUB_DB, with_engine, with_suffix};

/// What a doctor run found.
#[derive(Debug, Default)]
pub struct DoctorReport {
    /// The data directory that was inspected.
    pub data_dir: PathBuf,
    /// The store's applied schema version.
    pub schema_version: i64,
    /// The highest schema this binary knows how to run.
    pub supported_max: i64,
    /// True when the store was written by a newer binary.
    pub newer_than_binary: bool,
    /// The data directory's device and inode, or the platform's fallback line.
    pub identity: String,
    /// Free bytes on the volume the data directory sits on, when measurable.
    pub free_bytes: Option<u64>,
    /// The size of `hub.db-wal` on disk, or zero when there is none.
    pub wal_bytes: u64,
    /// The persisted id high-water mark, when the store carries one.
    pub id_high_water: Option<u64>,
    /// Engine files the integrity check ran on.
    pub checked: usize,
    /// Artifact blobs the store names but the tree does not hold.
    pub missing_blobs: Vec<String>,
    /// Every failure, already worded for the operator.
    pub problems: Vec<String>,
}

impl DoctorReport {
    /// Whether the store is healthy: nothing failed.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Diagnose the store under `data_dir`.
///
/// Opening the store for its schema and high-water mark comes before the full
/// check, so a running hub is refused with the shared message rather than
/// reported as a set of integrity problems.
pub async fn doctor(data_dir: &Path) -> Result<DoctorReport> {
    let hub_db = data_dir.join(HUB_DB);
    if !hub_db.is_file() {
        return Err(Error::NotFound(format!(
            "no hub store at {}",
            hub_db.display()
        )));
    }

    // The write-ahead log is read before the open, which is the size the store
    // was left with: a read that creates and then removes an absent sidecar
    // must not make the report claim one.
    let wal_bytes = wal_size(&hub_db)?;

    let (schema_version, id_high_water) = with_engine(&hub_db, async |db| {
        let schema_version = crate::store::schema_version(db).await?;
        let id_high_water = crate::store::read_id_high_water(db).await?;
        Ok((schema_version, id_high_water))
    })
    .await?;

    let supported_max = crate::store::schema::SUPPORTED_MAX;
    let newer_than_binary = schema_version > supported_max;

    let checked = check(data_dir).await?;
    let missing_blobs = checked
        .problems
        .iter()
        .filter(|problem| problem.contains("missing artifact blob"))
        .cloned()
        .collect::<Vec<_>>();

    let mut problems = checked.problems;
    if newer_than_binary {
        problems.insert(
            0,
            format!(
                "the store is at schema version {schema_version}, newer than this binary supports \
                 ({supported_max}); upgrade the binary before opening it"
            ),
        );
    }

    Ok(DoctorReport {
        data_dir: data_dir.to_path_buf(),
        schema_version,
        supported_max,
        newer_than_binary,
        identity: data_dir_identity(data_dir),
        free_bytes: free_space(data_dir),
        wal_bytes,
        id_high_water,
        checked: checked.checked,
        missing_blobs,
        problems,
    })
}

/// The size of a store file's write-ahead log, or zero when it has none.
fn wal_size(hub_db: &Path) -> Result<u64> {
    match std::fs::metadata(with_suffix(hub_db, "-wal")) {
        Ok(meta) => Ok(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err.into()),
    }
}

/// Free bytes on the data directory's volume, when the platform reports it.
fn free_space(data_dir: &Path) -> Option<u64> {
    crate::store::free_space_bytes(data_dir).and_then(|free| u64::try_from(free).ok())
}

/// The data directory's device and inode, for tying a report to one tree.
#[cfg(unix)]
fn data_dir_identity(data_dir: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(data_dir) {
        Ok(meta) => format!("device {}, inode {}", meta.dev(), meta.ino()),
        Err(err) => format!("unreadable: {err}"),
    }
}

/// No device or inode off Unix; the line says so rather than inventing one.
#[cfg(not(unix))]
fn data_dir_identity(_data_dir: &Path) -> String {
    "not reported on this platform".to_string()
}
