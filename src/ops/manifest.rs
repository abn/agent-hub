//! The backup manifest: what a backup holds, and the checks restore verifies.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The record a backup writes beside the files it copied.
///
/// `schema_version` is the hub store's applied migration, so a restore can tell
/// a reader which binary wrote it; each file carries the size and sha256 that
/// restore and check verify before anything is replaced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: i64,
    pub created_at: String,
    pub files: Vec<Entry>,
}

/// One copied file as the manifest records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The path under the backup root, with `/` separators.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    /// Write the manifest into a backup directory.
    pub fn write(&self, out_dir: &Path) -> Result<()> {
        let path = out_dir.join(super::MANIFEST_FILE);
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|err| Error::Engine(format!("could not encode the manifest: {err}")))?;
        std::fs::write(&path, bytes)?;
        super::harden_file(&path);
        Ok(())
    }

    /// Read the manifest a backup directory holds.
    pub fn read(dir: &Path) -> Result<Self> {
        let path = dir.join(super::MANIFEST_FILE);
        let bytes = std::fs::read(&path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(format!(
                    "{} holds no {}",
                    dir.display(),
                    super::MANIFEST_FILE
                ))
            } else {
                Error::Io(err)
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|err| {
            Error::InvalidArgument(format!("{} is not a valid manifest: {err}", path.display()))
        })
    }
}
