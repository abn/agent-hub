//! Restore a data directory from a verified backup, offline.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::manifest::Manifest;
use super::{HUB_DB, copy_file, dir_nonempty, join_rel, open_hub_store, sha256_file};

/// What a restore did.
#[derive(Debug)]
pub struct RestoreReport {
    pub files: usize,
    pub bytes: u64,
}

/// Replace `data_dir` with the contents of the backup in `from`.
///
/// Every member is verified against the manifest before anything is written;
/// a hub holding the store, and a non-empty destination without `--force`, are
/// both refused. The copy lands in a staging directory beside the destination
/// and is moved into place in one rename, so a failure leaves the original
/// untouched.
pub async fn restore(from: &Path, data_dir: &Path, force: bool) -> Result<RestoreReport> {
    let manifest = Manifest::read(from)?;

    // Verify every source file before touching the destination: a checksum
    // mismatch must not cost the operator the data they still have.
    let mut bytes = 0u64;
    for entry in &manifest.files {
        let source = join_rel(from, &entry.path)?;
        let metadata = std::fs::metadata(&source).map_err(|err| {
            Error::NotFound(format!("the backup is missing {}: {err}", entry.path))
        })?;
        if metadata.len() != entry.size {
            return Err(Error::InvalidArgument(format!(
                "{}: size {} does not match the manifest's {}",
                entry.path,
                metadata.len(),
                entry.size
            )));
        }
        let sum = sha256_file(&source)?;
        if sum != entry.sha256 {
            return Err(Error::InvalidArgument(format!(
                "{}: sha256 does not match the manifest",
                entry.path
            )));
        }
        bytes += entry.size;
    }

    // A serving hub holds the engine lock over the directory being replaced.
    let hub_db = data_dir.join(HUB_DB);
    if hub_db.is_file() {
        // The handle is dropped immediately; the refusal is what matters.
        let _held = open_hub_store(&hub_db).await?;
    }

    if !force && dir_nonempty(data_dir)? {
        return Err(Error::Conflict(format!(
            "{} is not empty; pass --force to replace it",
            data_dir.display()
        )));
    }

    let parent = data_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    // Build the whole tree beside the destination so the move is one rename,
    // and both live on the same filesystem even for a mount point.
    let token = ulid::Ulid::generate();
    let stage = parent.join(format!(".agent-hub-restore-{token}"));
    if let Err(err) = fill_stage(from, &manifest, &stage) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(err);
    }
    move_into_place(&stage, data_dir, parent, &token)?;

    Ok(RestoreReport {
        files: manifest.files.len(),
        bytes,
    })
}

/// Copy every manifest member into the staging tree.
fn fill_stage(from: &Path, manifest: &Manifest, stage: &Path) -> Result<()> {
    crate::config::private_dir(stage)?;
    for entry in &manifest.files {
        let source = join_rel(from, &entry.path)?;
        let dest = join_rel(stage, &entry.path)?;
        copy_file(&source, &dest)?;
    }
    Ok(())
}

/// Move the staging tree onto the destination, replacing it when asked.
fn move_into_place(stage: &Path, data_dir: &Path, parent: &Path, token: &ulid::Ulid) -> Result<()> {
    if !data_dir.exists() {
        std::fs::rename(stage, data_dir)?;
        return Ok(());
    }

    // Move the current tree aside first: the two renames cannot interleave, so
    // the destination is either the old tree or the new one, never half of
    // each. If the second rename fails the first is undone.
    let aside: PathBuf = parent.join(format!(".agent-hub-replaced-{token}"));
    std::fs::rename(data_dir, &aside)?;
    if let Err(err) = std::fs::rename(stage, data_dir) {
        let _ = std::fs::rename(&aside, data_dir);
        return Err(err.into());
    }
    // The replaced tree is now the operator's to remove, and failing to remove
    // it is not a failed restore.
    if let Err(err) = std::fs::remove_dir_all(&aside) {
        tracing::warn!(
            path = %aside.display(),
            error = %err,
            "the restored data directory is in place, but the replaced one could not be removed"
        );
    }
    Ok(())
}
