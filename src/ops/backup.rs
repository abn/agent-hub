//! Back up the whole store into a directory of its own, offline or by the
//! serving hub.

use std::path::{Path, PathBuf};

use crate::brain::BrainStore;
use crate::error::{Error, Result};

use super::manifest::{Entry, Manifest};
use super::{
    HUB_DB, SIDECAR_SUFFIXES, copy_file, file_size, harden_file, is_locked_refusal, locked,
    map_locked, open_hub_store, relative, sha256_file, walk_files, with_engine, with_suffix,
};

/// The refusal an offline backup gives while a hub holds the store. It names
/// the online backup, which is the one way to back up without stopping it.
pub(crate) const BACKUP_LOCKED_MESSAGE: &str = "a hub is using this data directory; run \
     `agent-hub backup --url URL` to have it take the backup, or stop it, or snapshot the \
     volume";

/// What a backup did.
#[derive(Debug)]
pub struct BackupReport {
    /// The hub store's applied schema version.
    pub schema_version: i64,
    /// When the manifest was written, as it records it.
    pub created_at: String,
    pub files: usize,
    pub bytes: u64,
    /// True when `VACUUM INTO` was refused for at least one engine file and a
    /// byte copy of the file and its sidecars stands in. Never true for an
    /// online backup, which has no safe byte copy to fall back to.
    pub used_fallback: bool,
}

/// The serving hub's own handles, which an online backup copies through.
#[derive(Clone, Copy)]
pub struct Serving<'a> {
    /// The hub store the process serves.
    pub db: &'a turso::Database,
    /// The session brains, under their per-file write locks.
    pub brain: &'a BrainStore,
    /// The project knowledge bases, under the same locks.
    pub knowledge: &'a BrainStore,
}

/// Where each engine file is read from.
#[derive(Clone, Copy)]
enum Source<'a> {
    /// No hub serves the store; this process takes the engine lock.
    Offline,
    /// The serving hub copies through its own handles.
    Serving(Serving<'a>),
}

/// Copy the whole store into `out_dir`.
///
/// The engine's lock is taken once, over `hub.db`, and every engine file is
/// then read while it is held: no writer exists in this process and a serving
/// hub is refused, so nothing changes under the copy.
pub async fn backup(data_dir: &Path, out_dir: &Path) -> Result<BackupReport> {
    match run(data_dir, out_dir, Source::Offline).await {
        Err(err) if is_locked_refusal(&err) => {
            Err(Error::Conflict(BACKUP_LOCKED_MESSAGE.to_string()))
        }
        outcome => outcome,
    }
}

/// Copy the whole store into `out_dir` from inside the hub that serves it.
///
/// The layout and the manifest are the offline backup's, so `check` and
/// `restore` read the result unchanged. Every store file removal is held off
/// for the whole run, then `hub.db` is copied first, then every brain and
/// knowledge file under its own write lock, then the artifact blobs and the
/// tailnet key state. Each engine file is one consistent snapshot of itself;
/// the store snapshot is the oldest of them, so every blob it names was on disk
/// before it and is still there when the blobs are copied.
pub async fn backup_serving(
    data_dir: &Path,
    serving: Serving<'_>,
    out_dir: &Path,
) -> Result<BackupReport> {
    let _frozen = crate::store::freeze_removals().await;
    run(data_dir, out_dir, Source::Serving(serving)).await
}

/// The fresh directory an online backup lands in, under `backup_dir`.
///
/// The configured directory must already exist, so a backup volume that is not
/// mounted is reported rather than filled in on the root filesystem, and it
/// must resolve to somewhere outside the data directory, symlinks followed.
/// The name is a UTC timestamp to the millisecond, so a listing sorts by age.
pub fn online_target(backup_dir: &Path, data_dir: &Path) -> Result<PathBuf> {
    let resolved = std::fs::canonicalize(backup_dir).map_err(|err| {
        Error::Unavailable(format!(
            "the backup directory {} is not there ({err}); create it, or mount its volume",
            backup_dir.display()
        ))
    })?;
    if !resolved.is_dir() {
        return Err(Error::Unavailable(format!(
            "the backup directory {} is not a directory",
            backup_dir.display()
        )));
    }
    let data = std::fs::canonicalize(data_dir)?;
    if resolved.starts_with(&data) {
        return Err(Error::Conflict(format!(
            "the backup directory {} resolves inside the data directory {}; point HUB_BACKUP_DIR \
             outside it",
            backup_dir.display(),
            data_dir.display()
        )));
    }
    let target = backup_dir.join(crate::store::filename_stamp());
    if target.exists() {
        return Err(Error::Conflict(format!(
            "{} already exists; try again",
            target.display()
        )));
    }
    Ok(target)
}

async fn run(data_dir: &Path, out_dir: &Path, source: Source<'_>) -> Result<BackupReport> {
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

    match backup_inner(data_dir, out_dir, &hub_db, source).await {
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

async fn backup_inner(
    data_dir: &Path,
    out_dir: &Path,
    hub_db: &Path,
    source: Source<'_>,
) -> Result<BackupReport> {
    // Offline, the engine opened here holds the lock that covers every later
    // copy, so it lives until the manifest is written.
    let offline;
    let db = match source {
        Source::Offline => {
            offline = open_hub_store(hub_db).await?;
            &offline
        }
        Source::Serving(serving) => serving.db,
    };
    let schema_version = crate::store::schema_version(db).await.map_err(map_locked)?;

    let mut entries: Vec<Entry> = Vec::new();
    // Online engine copies, sized and hashed together once every copy is done.
    let mut copied: Vec<(PathBuf, String)> = Vec::new();
    let mut used_fallback = false;

    // The hub store first: online, its snapshot is the oldest file in the set,
    // which is what keeps every blob it names in the copy below.
    let hub_rel = relative(data_dir, hub_db)?;
    let hub_dest = out_dir.join(&hub_rel);
    match source {
        Source::Offline => {
            collect_engine(
                db,
                hub_db,
                &hub_dest,
                &hub_rel,
                &mut entries,
                &mut used_fallback,
            )
            .await?
        }
        Source::Serving(_) => {
            let conn = crate::store::connect(db)?;
            vacuum_into(&conn, &hub_dest).await?;
            copied.push((hub_dest.clone(), hub_rel));
        }
    }

    for (root, store) in [
        (data_dir.join("sessions"), source_store(source, false)),
        (
            crate::brain::knowledge_dir(data_dir),
            source_store(source, true),
        ),
    ] {
        let walked = root.clone();
        for file in blocking(move || walk_files(&walked)).await? {
            if file.extension().and_then(|ext| ext.to_str()) != Some("db")
                || in_quarantine(&root, &file)
            {
                continue;
            }
            let rel = relative(data_dir, &file)?;
            let dest = out_dir.join(&rel);
            match store {
                None => {
                    let db = open_hub_store(&file).await?;
                    collect_engine(&db, &file, &dest, &rel, &mut entries, &mut used_fallback)
                        .await?;
                }
                Some(store) => {
                    if copy_brain(store, &root, &file, &dest).await? {
                        copied.push((dest, rel));
                    }
                }
            }
        }
    }
    // Sizing and hashing the engine copies reads every byte of them, so it
    // runs off the runtime's workers too.
    entries.extend(
        blocking(move || {
            copied
                .iter()
                .map(|(dest, rel)| entry_for(dest, rel))
                .collect::<Result<Vec<_>>>()
        })
        .await?,
    );

    // Artifact blobs are opaque bytes: copied verbatim, never through the
    // engine. The store's own reconcile has already judged which are live.
    // The embedded tailnet keeps its device identity and private key in
    // `tailnet/keys.json`, under the data directory. It is not engine state and
    // the store does not name it, but a restore that dropped it would leave the
    // node re-registering with a new identity, so it is part of the set when
    // present.
    // Walking, copying and hashing every blob is blocking file work, so it
    // runs off the runtime's workers and the hub keeps serving meanwhile.
    let online = matches!(source, Source::Serving(_));
    let (from, to) = (data_dir.to_path_buf(), out_dir.to_path_buf());
    let blobs = blocking(move || copy_blobs(&from, &to, online)).await?;
    entries.extend(blobs);
    if online {
        verify_blobs(out_dir, &hub_dest).await?;
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
        created_at: manifest.created_at,
        files: manifest.files.len(),
        bytes,
        used_fallback,
    })
}

/// Copy the artifact blobs and the tailnet key state verbatim, returning their
/// manifest entries.
fn copy_blobs(data_dir: &Path, out_dir: &Path, online: bool) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for root in [data_dir.join("artifacts"), data_dir.join("tailnet")] {
        for file in walk_files(&root)? {
            if in_quarantine(&root, &file) {
                continue;
            }
            let rel = relative(data_dir, &file)?;
            let dest = out_dir.join(&rel);
            match copy_file(&file, &dest) {
                Ok(()) => entries.push(entry_for(&dest, &rel)?),
                // Online, a pending upload renamed onto its version, or a file
                // written after the store snapshot and removed again, can go
                // between the walk and the copy. Neither is named by the
                // snapshot, which the removal gate guarantees for every file
                // that is; the cross-check after the copy holds it to that.
                Err(Error::Io(err)) if online && err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(entries)
}

/// The brain store an online backup copies a tree's files through, or `None`
/// offline, where each file is opened under the engine lock directly.
fn source_store(source: Source<'_>, knowledge: bool) -> Option<&BrainStore> {
    match source {
        Source::Offline => None,
        Source::Serving(serving) if knowledge => Some(serving.knowledge),
        Source::Serving(serving) => Some(serving.brain),
    }
}

/// Copy one brain or knowledge file through the serving hub's own handle,
/// under the file's write lock. Returns whether there was a file to copy.
///
/// Only a file at `<project>/<id>.db` is one the hub serves. Anything deeper
/// under the tree is not part of the store the hub has open, so it is left out
/// rather than opened.
async fn copy_brain(store: &BrainStore, root: &Path, file: &Path, dest: &Path) -> Result<bool> {
    let Some((project_id, file_id)) = brain_ids(root, file) else {
        tracing::warn!(path = %file.display(), "left a file the hub does not serve out of the backup");
        return Ok(false);
    };
    let Some(brain) = store.open_existing(&project_id, &file_id).await? else {
        return Ok(false);
    };
    match brain
        .with_locked_connection(async |conn| vacuum_into(conn, dest).await)
        .await
    {
        Ok(()) => Ok(true),
        // Removed between the open and the lock: nothing to copy.
        Err(Error::Conflict(_)) if !file.exists() => Ok(false),
        Err(err) => Err(err),
    }
}

/// Whether `file` sits in a project delete's quarantine under `root`.
///
/// The quarantine is no longer part of the store: its project's rows are gone
/// or going, and startup recovery removes it, so neither backup carries it.
fn in_quarantine(root: &Path, file: &Path) -> bool {
    file.strip_prefix(root)
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|first| first.as_os_str().to_str())
        .is_some_and(crate::store::projects::is_quarantine)
}

/// Run blocking file work off the runtime's workers.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(std::io::Error::other)?
}

/// The project and file id of a brain at `<root>/<project>/<id>.db`.
fn brain_ids(root: &Path, file: &Path) -> Option<(String, String)> {
    let rel = file.strip_prefix(root).ok()?;
    let mut parts = rel.components();
    let project = parts.next()?.as_os_str().to_str()?.to_string();
    let name = parts.next()?.as_os_str().to_str()?;
    if parts.next().is_some() {
        return None;
    }
    let id = name.strip_suffix(".db")?.to_string();
    Some((project, id))
}

/// Refuse an online backup whose store snapshot names a blob it does not hold.
///
/// The removal gate is what makes this hold; reading the copy back is what
/// proves it did, so a set `check` would fail after a restore is never left
/// behind as a finished backup.
async fn verify_blobs(out_dir: &Path, hub_copy: &Path) -> Result<()> {
    let missing = with_engine(hub_copy, async |db| {
        if !super::table_present(db, "artifact_versions").await? {
            return Ok(Vec::new());
        }
        let mut missing = Vec::new();
        for blob in super::check::referenced_artifacts(db).await? {
            if !super::join_rel(out_dir, &blob)?.is_file() {
                missing.push(blob);
            }
        }
        Ok(missing)
    })
    .await?;
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::Engine(format!(
            "the store snapshot names artifact blobs the backup does not hold: {}",
            missing.join(", ")
        )))
    }
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
    let conn = crate::store::connect(db)?;
    match vacuum_into(&conn, dest).await {
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

/// Run the engine's own copy of the database behind `conn` into `dest`.
///
/// `VACUUM INTO` reads inside one read transaction, so the copy is a snapshot
/// of what was committed when it began, and writers on other connections to the
/// same database carry on while it runs.
async fn vacuum_into(conn: &turso::Connection, dest: &Path) -> Result<()> {
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
