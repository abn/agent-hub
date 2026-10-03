//! Back up the whole store, offline, into a directory of its own.

use std::path::Path;

use crate::error::{Error, Result};

use super::manifest::{Entry, Manifest};
use super::{
    HUB_DB, SIDECAR_SUFFIXES, copy_file, file_size, harden_file, locked, map_locked,
    open_hub_store, relative, sha256_file, walk_files, with_suffix,
};

/// What a backup did.
#[derive(Debug)]
pub struct BackupReport {
    /// The hub store's applied schema version.
    pub schema_version: i64,
    pub files: usize,
    pub bytes: u64,
    /// True when `VACUUM INTO` was refused for at least one engine file and a
    /// byte copy of the file and its sidecars stands in.
    pub used_fallback: bool,
}

/// Copy the whole store into `out_dir`.
///
/// The engine's lock is taken once, over `hub.db`, and every engine file is
/// then read while it is held: no writer exists in this process and a serving
/// hub is refused, so nothing changes under the copy.
pub async fn backup(data_dir: &Path, out_dir: &Path) -> Result<BackupReport> {
    let hub_db = data_dir.join(HUB_DB);
    if !hub_db.is_file() {
        return Err(Error::NotFound(format!(
            "no hub store at {}",
            hub_db.display()
        )));
    }
    // A backup written inside the tree it copies would be walked as content.
    if out_dir == data_dir || out_dir.starts_with(data_dir) {
        return Err(Error::InvalidArgument(format!(
            "the backup output {} must be outside the data directory {}",
            out_dir.display(),
            data_dir.display()
        )));
    }
    let created = prepare_out_dir(out_dir)?;

    match backup_inner(data_dir, out_dir, &hub_db).await {
        Ok(report) => Ok(report),
        Err(err) => {
            // Never leave a half-written set where a complete backup is
            // expected. A directory that already existed is left alone.
            if created {
                let _ = std::fs::remove_dir_all(out_dir);
            }
            Err(err)
        }
    }
}

async fn backup_inner(data_dir: &Path, out_dir: &Path, hub_db: &Path) -> Result<BackupReport> {
    let db = open_hub_store(hub_db).await?;
    let schema_version = crate::store::schema_version(&db)
        .await
        .map_err(map_locked)?;

    let mut entries: Vec<Entry> = Vec::new();
    let mut used_fallback = false;

    // The hub store: the already-open engine is used so its lock covers the
    // copy, and the same engine copies every brain and knowledge file after.
    let hub_rel = relative(data_dir, hub_db)?;
    collect_engine(
        &db,
        hub_db,
        &out_dir.join(&hub_rel),
        &hub_rel,
        &mut entries,
        &mut used_fallback,
    )
    .await?;

    for root in [
        data_dir.join("sessions"),
        crate::brain::knowledge_dir(data_dir),
    ] {
        for source in walk_files(&root)? {
            if source.extension().and_then(|ext| ext.to_str()) != Some("db") {
                continue;
            }
            let rel = relative(data_dir, &source)?;
            let db = open_hub_store(&source).await?;
            collect_engine(
                &db,
                &source,
                &out_dir.join(&rel),
                &rel,
                &mut entries,
                &mut used_fallback,
            )
            .await?;
        }
    }

    // Artifact blobs are opaque bytes: copied verbatim, never through the
    // engine. The store's own reconcile has already judged which are live.
    for source in walk_files(&data_dir.join("artifacts"))? {
        let rel = relative(data_dir, &source)?;
        let dest = out_dir.join(&rel);
        copy_file(&source, &dest)?;
        entries.push(entry_for(&dest, &rel)?);
    }

    // The embedded tailnet keeps its device identity and private key in
    // `tailnet/keys.json`, under the data directory. It is not engine state and
    // the store does not name it, but a restore that dropped it would leave the
    // node re-registering with a new identity, so it is part of the set when
    // present.
    for source in walk_files(&data_dir.join("tailnet"))? {
        let rel = relative(data_dir, &source)?;
        let dest = out_dir.join(&rel);
        copy_file(&source, &dest)?;
        entries.push(entry_for(&dest, &rel)?);
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let bytes = entries.iter().map(|entry| entry.size).sum();
    let manifest = Manifest {
        schema_version,
        created_at: crate::store::now_rfc3339(),
        files: entries,
    };
    manifest.write(out_dir)?;

    Ok(BackupReport {
        schema_version,
        files: manifest.files.len(),
        bytes,
        used_fallback,
    })
}

/// Copy one engine file through `VACUUM INTO`, falling back to a byte copy.
///
/// `VACUUM INTO` folds the write-ahead log into one self-contained file and
/// takes no sidecar with it. When the engine refuses it for a reason other than
/// a held lock, the file is copied byte for byte while the engine lock is held,
/// together with any `-wal` and `-shm` beside it, so the set is still
/// consistent; [`BackupReport::used_fallback`] records that this happened.
async fn collect_engine(
    db: &turso::Database,
    source: &Path,
    dest: &Path,
    rel: &str,
    entries: &mut Vec<Entry>,
    used_fallback: &mut bool,
) -> Result<()> {
    match vacuum_into(db, dest).await {
        Ok(()) => {
            entries.push(entry_for(dest, rel)?);
            Ok(())
        }
        Err(err) if err.is_locked() => Err(locked()),
        Err(err) => {
            tracing::warn!(
                path = %source.display(),
                error = %err,
                "the engine refused VACUUM INTO; copying the store file and its sidecars byte for byte"
            );
            *used_fallback = true;
            copy_with_sidecars(source, dest, rel, entries)
        }
    }
}

/// Run the engine's own copy into `dest`.
async fn vacuum_into(db: &turso::Database, dest: &Path) -> Result<()> {
    // VACUUM INTO refuses a destination that already exists.
    match std::fs::remove_file(dest) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The destination is a string literal to the engine, so a quote in the path
    // is doubled rather than ending the statement.
    let target = dest
        .to_str()
        .ok_or_else(|| Error::Engine("backup destination is not valid UTF-8".to_string()))?
        .replace('\'', "''");
    let conn = crate::store::connect(db)?;
    conn.execute(&format!("VACUUM INTO '{target}'"), ())
        .await
        .map_err(crate::store::engine)?;
    // The engine may leave an empty write-ahead log beside the copy, because
    // the source is in WAL mode. VACUUM INTO has already written one
    // self-contained file, so the sidecar is spurious and not in the manifest.
    for suffix in SIDECAR_SUFFIXES {
        let _ = std::fs::remove_file(with_suffix(dest, suffix));
    }
    harden_file(dest);
    Ok(())
}

/// Copy a store file and whichever of its sidecars exist.
fn copy_with_sidecars(
    source: &Path,
    dest: &Path,
    rel: &str,
    entries: &mut Vec<Entry>,
) -> Result<()> {
    copy_file(source, dest)?;
    entries.push(entry_for(dest, rel)?);
    for suffix in SIDECAR_SUFFIXES {
        let side = with_suffix(source, suffix);
        if side.is_file() {
            let side_rel = format!("{rel}{suffix}");
            let side_dest = with_suffix(dest, suffix);
            copy_file(&side, &side_dest)?;
            entries.push(entry_for(&side_dest, &side_rel)?);
        }
    }
    Ok(())
}

fn entry_for(path: &Path, rel: &str) -> Result<Entry> {
    Ok(Entry {
        path: rel.to_string(),
        size: file_size(path)?,
        sha256: sha256_file(path)?,
    })
}

/// Create or check the output directory, reporting whether it was created.
fn prepare_out_dir(out_dir: &Path) -> Result<bool> {
    match std::fs::metadata(out_dir) {
        Ok(metadata) if metadata.is_dir() => {
            if std::fs::read_dir(out_dir)?.next().is_some() {
                return Err(Error::Conflict(format!(
                    "the backup output {} is not empty",
                    out_dir.display()
                )));
            }
            Ok(false)
        }
        Ok(_) => Err(Error::Conflict(format!(
            "the backup output {} is not a directory",
            out_dir.display()
        ))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            crate::config::private_dir(out_dir)?;
            Ok(true)
        }
        Err(err) => Err(err.into()),
    }
}
