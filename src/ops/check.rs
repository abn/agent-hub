//! Check store integrity, and verify a backup against its manifest.

use std::path::Path;

use crate::error::{Error, Result};

use super::manifest::Manifest;
use super::{HUB_DB, join_rel, relative, sha256_file, table_present, walk_files, with_engine};

/// What a check found.
#[derive(Debug, Default)]
pub struct CheckReport {
    /// Engine files `integrity_check` ran on.
    pub checked: usize,
    /// Every failure, already worded for the operator.
    pub problems: Vec<String>,
}

impl CheckReport {
    /// Whether the check is clean.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Check `dir`, whether it is a data directory or a backup.
///
/// A backup's manifest is verified member by member; every engine file in the
/// tree is then run through `PRAGMA integrity_check`. A data directory
/// additionally has its artifact blobs cross-checked against the paths the
/// store names, so a blob that is referenced but gone is reported, not just a
/// corrupt file.
pub async fn check(dir: &Path) -> Result<CheckReport> {
    let mut report = CheckReport::default();

    // A manifest makes this a backup set: verify each member's bytes.
    let manifest = match Manifest::read(dir) {
        Ok(manifest) => Some(manifest),
        Err(Error::NotFound(_)) => None,
        Err(err) => {
            report.problems.push(err.to_string());
            None
        }
    };
    if let Some(manifest) = &manifest {
        verify_manifest(dir, manifest, &mut report);
    }

    let hub_db = dir.join(HUB_DB);
    let mut db_paths = Vec::new();
    if hub_db.is_file() {
        db_paths.push(hub_db.clone());
    }
    for root in [dir.join("sessions"), crate::brain::knowledge_dir(dir)] {
        for path in walk_files(&root)? {
            if path.extension().and_then(|ext| ext.to_str()) == Some("db") {
                db_paths.push(path);
            }
        }
    }

    if db_paths.is_empty() && manifest.is_none() {
        report
            .problems
            .push(format!("missing the hub store {}", hub_db.display()));
    }

    for path in &db_paths {
        report.checked += 1;
        let rel = relative(dir, path).unwrap_or_else(|_| path.display().to_string());
        let outcome = with_engine(path, async |db| {
            let mut problems = Vec::new();
            match integrity(db).await {
                Ok(issues) => problems.extend(issues),
                Err(err) => problems.push(err.to_string()),
            }
            // A data directory's artifact rows name bytes the tree must hold;
            // a backup has no store connection to cross-check. A store older
            // than the version table has no rows to cross-check, so the query
            // is skipped rather than reported as a missing table.
            if manifest.is_none() && path == &hub_db {
                match table_present(db, "artifact_versions").await {
                    Ok(true) => match referenced_artifacts(db).await {
                        Ok(paths) => {
                            for blob in paths {
                                match join_rel(dir, &blob) {
                                    Ok(blob_path) if !blob_path.is_file() => {
                                        problems.push(format!("missing artifact blob {blob}"));
                                    }
                                    Err(err) => problems.push(err.to_string()),
                                    _ => {}
                                }
                            }
                        }
                        Err(err) => problems.push(err.to_string()),
                    },
                    Ok(false) => {}
                    Err(err) => problems.push(err.to_string()),
                }
            }
            Ok(problems)
        })
        .await;
        match outcome {
            Ok(problems) => {
                for problem in problems {
                    // A lock that surfaced mid-query is caught inside the
                    // closure as a problem string; the shared refusal is still
                    // a usage condition, so it propagates rather than counting
                    // as damage.
                    if problem.contains(super::LOCKED_MESSAGE) {
                        return Err(super::locked());
                    }
                    report.problems.push(format!("{rel}: {problem}"));
                }
            }
            // A held lock is a usage condition, not store damage: the hub is
            // serving this store, so the command cannot run. It propagates as
            // the one shared refusal the other offline commands give, rather
            // than being counted as an integrity problem under one file.
            Err(err) if super::is_locked_refusal(&err) => return Err(super::locked()),
            Err(err) => report.problems.push(format!("{rel}: {err}")),
        }
    }

    Ok(report)
}

/// Verify each manifest member's presence, size and checksum.
fn verify_manifest(dir: &Path, manifest: &Manifest, report: &mut CheckReport) {
    for entry in &manifest.files {
        let path = match join_rel(dir, &entry.path) {
            Ok(path) => path,
            Err(err) => {
                report.problems.push(err.to_string());
                continue;
            }
        };
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                report.problems.push(format!("missing {}", entry.path));
                continue;
            }
        };
        if metadata.len() != entry.size {
            report.problems.push(format!(
                "{}: size {} does not match the manifest's {}",
                entry.path,
                metadata.len(),
                entry.size
            ));
            continue;
        }
        match sha256_file(&path) {
            Ok(sum) if sum == entry.sha256 => {}
            Ok(_) => report.problems.push(format!(
                "{}: sha256 does not match the manifest",
                entry.path
            )),
            Err(err) => report.problems.push(format!("{}: {err}", entry.path)),
        }
    }
}

/// The engine's own integrity check, as the lines it reports besides `ok`.
async fn integrity(db: &turso::Database) -> Result<Vec<String>> {
    let conn = crate::store::connect(db)?;
    let mut rows = conn
        .query("PRAGMA integrity_check", ())
        .await
        .map_err(crate::store::engine)?;
    let mut issues = Vec::new();
    while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
        if let turso::Value::Text(line) = row.get_value(0).map_err(crate::store::engine)?
            && line != "ok"
        {
            issues.push(line);
        }
    }
    Ok(issues)
}

/// The blob paths the artifact history names.
pub(super) async fn referenced_artifacts(db: &turso::Database) -> Result<Vec<String>> {
    let conn = crate::store::connect(db)?;
    let mut rows = conn
        .query("SELECT path FROM artifact_versions", ())
        .await
        .map_err(crate::store::engine)?;
    let mut paths = Vec::new();
    while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
        if let turso::Value::Text(path) = row.get_value(0).map_err(crate::store::engine)? {
            paths.push(path);
        }
    }
    Ok(paths)
}
