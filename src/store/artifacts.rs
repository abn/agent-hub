//! Artifacts: metadata in the hub store, blobs on the data volume.
//!
//! A protected artifact carries an encryption envelope and ciphertext; the
//! server never sees its plaintext. Publishing or updating appends a feed
//! event and refreshes the search corpus.
//!
//! Every publish and update records one `artifact_versions` row, so any
//! version stays addressable after the current pointer moves on. The row
//! carries the per-version envelope, which is key-derivation parameters,
//! not plaintext, so history never weakens the encryption posture.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::blob;
use crate::error::{Error, Result};
use crate::limits;
use crate::store::events::{self, NewEvent};
use crate::store::idempotency;
use crate::store::search::{SearchDoc, index_doc};

/// Maximum characters of an artifact title.
pub const TITLE_CHARS_MAX: usize = 500;
/// Maximum characters of an artifact description.
pub const DESCRIPTION_CHARS_MAX: usize = 2000;
/// Maximum characters of an artifact favicon. Lenient on purpose: a single
/// emoji is one code point, a compound one is several, and the render
/// escapes it either way.
pub const FAVICON_CHARS_MAX: usize = 8;
/// Maximum UTF-8 bytes of a version label.
pub const LABEL_BYTES_MAX: usize = 60;

/// Artifact metadata. The blob path stays internal.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub actor: Option<String>,
    pub title: String,
    pub description: String,
    pub favicon: String,
    pub label: Option<String>,
    pub kind: String,
    pub version: i64,
    pub protected: bool,
    pub envelope: Option<serde_json::Value>,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip)]
    pub path: String,
    pub comments_count: i64,
    pub comments_open: i64,
}

/// One immutable version of an artifact.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactVersion {
    pub version: i64,
    pub title: String,
    pub description: String,
    pub favicon: String,
    pub kind: String,
    pub label: Option<String>,
    pub protected: bool,
    pub envelope: Option<serde_json::Value>,
    pub size_bytes: i64,
    pub created_at: String,
}

/// A new artifact to publish.
pub struct NewArtifact<'a> {
    pub actor: &'a str,
    pub project_id: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    pub favicon: &'a str,
    pub label: Option<&'a str>,
    pub kind: &'a str,
    pub content: &'a [u8],
    /// The encryption envelope when the artifact is protected.
    pub envelope: Option<serde_json::Value>,
    pub session_id: Option<&'a str>,
}

/// What a new version does with the artifact's protection.
///
/// An update that says nothing keeps what the artifact carries, whatever the
/// project's policy is: an agent relying on that sends ciphertext, and storing
/// it as plaintext because a policy changed would corrupt the artifact. Moving
/// a version into the clear is therefore something the caller says out loud.
#[derive(Debug, Clone, Default)]
pub enum EnvelopeUpdate {
    /// Carry the current envelope forward, if there is one.
    #[default]
    Keep,
    /// Publish this version protected under this envelope.
    Set(serde_json::Value),
    /// Publish this version in the clear: the content sent is plaintext.
    Clear,
}

impl EnvelopeUpdate {
    /// The envelope the new version carries, given the current one.
    fn resolve(&self, current: Option<&serde_json::Value>) -> Option<serde_json::Value> {
        match self {
            Self::Keep => current.cloned(),
            Self::Set(envelope) => Some(envelope.clone()),
            Self::Clear => None,
        }
    }
}

/// Options for publishing a new version of an artifact.
#[derive(Debug, Clone, Default)]
pub struct UpdateOptions<'a> {
    /// When set, the update applies only if the artifact is still at this
    /// version, unless `force` is set. A mismatch is a conflict naming the
    /// current version.
    pub base_version: Option<i64>,
    /// Overwrite a version mismatch instead of conflicting.
    pub force: bool,
    /// Absent keeps the current label; an explicit null or empty string clears
    /// it; a string sets a new label.
    pub label: Option<Option<&'a str>>,
    /// The session performing the update, when one is in scope.
    pub session_id: Option<&'a str>,
}

/// What the metadata transaction did with the blob written before it opened.
enum Written {
    /// The transaction committed, and the blob is the version it recorded.
    Kept(Artifact),
    /// An idempotent replay answered the call, so the blob is dead weight.
    Dropped(Artifact),
}

/// Publish an artifact: write the blob, record the metadata and its first
/// version row, append a feed event, and index it.
///
/// The blob is written before the transaction opens, so the store's write lock
/// is never held across a transfer of up to the artifact cap. The id is minted
/// here and no row names it yet, so nothing else can reach that path, and a
/// transaction that does not commit takes the blob with it.
pub async fn publish(
    db: &Database,
    data_dir: &Path,
    artifact: NewArtifact<'_>,
    idempotency_key: Option<&str>,
) -> Result<Artifact> {
    publish_for_principal(db, data_dir, None, artifact, idempotency_key).await
}

pub async fn publish_for_principal(
    db: &Database,
    data_dir: &Path,
    principal: Option<&crate::principal::Principal>,
    artifact: NewArtifact<'_>,
    idempotency_key: Option<&str>,
) -> Result<Artifact> {
    limits::check_artifact(artifact.content.len())?;
    let title = resolve_title(artifact.title, artifact.kind, artifact.content)?;
    let description = check_description(artifact.description)?;
    let favicon = check_favicon(artifact.favicon)?;
    let label = check_label(artifact.label)?;

    let mut conn = super::connect(db)?;

    // A retry with the same key returns what the first call produced, so a
    // dropped response does not leave a duplicate artifact. The lookup runs
    // here so a retry writes no blob at all, and again inside the transaction,
    // where the first call's record is certain to be visible.
    if let Some(key) = idempotency_key
        && let Some(entry) = idempotency::lookup_entry(
            &conn,
            artifact.project_id,
            idempotency::OP_ARTIFACT_PUBLISH,
            key,
        )
        .await?
    {
        return replay(&conn, &entry, None).await;
    }

    let id = crate::store::next_id();
    let created_at = crate::store::now_rfc3339();
    let envelope_json = artifact.envelope.as_ref().map(|value| value.to_string());
    let protected = envelope_json.is_some();

    let mut created_rel: Option<String> = None;

    let write = async {
        let tx = conn
            .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await
            .map_err(engine)?;

        let mut p_rows = tx
            .query(
                "SELECT status FROM projects WHERE id = ?1",
                vec![Value::Text(artifact.project_id.to_string())],
            )
            .await
            .map_err(engine)?;
        let p_row = p_rows.next().await.map_err(engine)?.ok_or_else(|| {
            Error::NotFound(format!("project {} not found", artifact.project_id))
        })?;
        let status: String = match p_row.get_value(0).map_err(engine)? {
            Value::Text(s) => s,
            _ => "active".to_string(),
        };
        if status != "active" {
            return Err(Error::NotFound(format!("project {} not found", artifact.project_id)));
        }

        if let Some(p) = principal {
            crate::policy::authorize_in_tx(&tx, p, artifact.project_id, crate::policy::Access::Write).await?;
        }

        if let Some(key) = idempotency_key
            && let Some(entry) =
                idempotency::lookup_entry(&tx, artifact.project_id, idempotency::OP_ARTIFACT_PUBLISH, key).await?
        {
            return Ok(Written::Dropped(replay(&tx, &entry, None).await?));
        }

        let rel = blob::write(
            data_dir,
            artifact.project_id,
            &id,
            1,
            artifact.kind,
            artifact.content,
        )?;
        created_rel = Some(rel.clone());

        let published = Artifact {
            id: id.clone(),
            project_id: artifact.project_id.to_string(),
            session_id: artifact.session_id.map(str::to_string),
            actor: Some(artifact.actor.to_string()),
            title: title.clone(),
            description: description.clone(),
            favicon: favicon.clone(),
            label: label.clone(),
            kind: artifact.kind.to_string(),
            version: 1,
            protected,
            envelope: artifact.envelope.clone(),
            size_bytes: artifact.content.len() as i64,
            created_at: created_at.clone(),
            updated_at: created_at.clone(),
            path: rel.clone(),
            comments_count: 0,
            comments_open: 0,
        };

        tx.execute(
            "INSERT INTO artifacts(id, project_id, title, description, favicon, label, kind, current_ver, envelope, path, size_bytes, created_at, updated_at, session_id, actor)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?11, ?12, ?13)",
            vec![
                Value::Text(id.clone()),
                Value::Text(artifact.project_id.to_string()),
                Value::Text(title.clone()),
                Value::Text(description.clone()),
                Value::Text(favicon.clone()),
                optional_text(label.as_deref()),
                Value::Text(artifact.kind.to_string()),
                optional_text(envelope_json.as_deref()),
                Value::Text(rel.clone()),
                Value::Integer(artifact.content.len() as i64),
                Value::Text(created_at.clone()),
                optional_text(artifact.session_id),
                Value::Text(artifact.actor.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
        insert_version(
            &tx,
            &id,
            1,
            &title,
            &description,
            &favicon,
            artifact.kind,
            label.as_deref(),
            protected,
            envelope_json.as_deref(),
            artifact.content.len() as i64,
            &rel,
            &created_at,
        )
        .await?;
        index_doc(
            &tx,
            SearchDoc {
                doc_id: &artifact_doc_id(&id),
                project_id: artifact.project_id,
                kind: "artifact",
                ref_id: &id,
                session_id: artifact.session_id,
                title: Some(&title),
                body: searchable_body(artifact.content, protected),
                updated_at: &created_at,
            },
        )
        .await?;
        let event_id = append_event(
            &tx,
            artifact.actor,
            artifact.project_id,
            "published",
            &id,
            &title,
            artifact.kind,
            1,
            protected,
            label.as_deref(),
            artifact.session_id,
        )
        .await?;
        if let Some(key) = idempotency_key {
            idempotency::record_artifact_publish(&tx, artifact.project_id, key, &event_id, &id, 1, &created_at)
                .await?;
        }
        tx.commit().await.map_err(engine)?;
        Ok(Written::Kept(published))
    }
    .await;

    match write {
        Ok(Written::Kept(published)) => Ok(published),
        Ok(Written::Dropped(replayed)) => {
            if let Some(rel) = created_rel {
                let _ = blob::remove(data_dir, &rel);
            }
            Ok(replayed)
        }
        Err(err) => {
            if let Some(rel) = created_rel {
                let _ = blob::remove(data_dir, &rel);
            }
            Err(err)
        }
    }
}

/// Publish a new version of an existing artifact.
///
/// The version bump is read inside the immediate transaction, so concurrent
/// updates serialise and each writes a distinct version file. A stale
/// `base_version` without `force` is a conflict, not an overwrite.
///
/// The content is written under a pending name before the transaction opens,
/// so the store's write lock is never held across a transfer of up to the
/// artifact cap, and is renamed onto its version path once the number is
/// allocated. A transaction that does not commit takes the blob with it.
#[allow(clippy::too_many_arguments)]
pub async fn update(
    db: &Database,
    data_dir: &Path,
    actor: &str,
    artifact_id: &str,
    content: &[u8],
    envelope: EnvelopeUpdate,
    opts: UpdateOptions<'_>,
    idempotency_key: Option<&str>,
) -> Result<Artifact> {
    update_for_principal(
        db,
        data_dir,
        None,
        actor,
        artifact_id,
        content,
        envelope,
        opts,
        idempotency_key,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_for_principal(
    db: &Database,
    data_dir: &Path,
    principal: Option<&crate::principal::Principal>,
    actor: &str,
    artifact_id: &str,
    content: &[u8],
    envelope: EnvelopeUpdate,
    opts: UpdateOptions<'_>,
    idempotency_key: Option<&str>,
) -> Result<Artifact> {
    limits::check_artifact(content.len())?;
    if let Some(Some(l)) = opts.label {
        check_label(Some(l))?;
    }

    let mut conn = super::connect(db)?;
    // Only the project and the kind are taken from this read, and neither ever
    // changes for an artifact, so it is enough to name the pending file. Every
    // decision below is made on the row the transaction reads.
    let current = row_on(&conn, artifact_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("artifact {artifact_id} not found")))?;

    // A retry with the same key returns the version the first call granted.
    // As in publish, the lookup runs before the blob write and again inside
    // the transaction.
    if let Some(key) = idempotency_key
        && let Some(entry) = idempotency::lookup_entry(
            &conn,
            &current.project_id,
            idempotency::OP_ARTIFACT_UPDATE,
            key,
        )
        .await?
    {
        return replay(&conn, &entry, Some(artifact_id)).await;
    }

    let pending = blob::write_pending(
        data_dir,
        &current.project_id,
        artifact_id,
        &current.kind,
        content,
    )?;
    let updated_at = crate::store::now_rfc3339();
    let mut promoted = None;

    let write = async {
        let tx = conn
            .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await
            .map_err(engine)?;
        let existing = row_on(&tx, artifact_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("artifact {artifact_id} not found")))?;

        let mut p_rows = tx
            .query(
                "SELECT status FROM projects WHERE id = ?1",
                vec![Value::Text(existing.project_id.clone())],
            )
            .await
            .map_err(engine)?;
        let p_row = p_rows.next().await.map_err(engine)?.ok_or_else(|| {
            Error::NotFound(format!("project {} not found", existing.project_id))
        })?;
        let status: String = match p_row.get_value(0).map_err(engine)? {
            Value::Text(s) => s,
            _ => "active".to_string(),
        };
        if status != "active" {
            return Err(Error::NotFound(format!("project {} not found", existing.project_id)));
        }

        if let Some(p) = principal {
            crate::policy::authorize_in_tx(&tx, p, &existing.project_id, crate::policy::Access::Write).await?;
        }

        if let Some(key) = idempotency_key
            && let Some(entry) =
                idempotency::lookup_entry(&tx, &existing.project_id, idempotency::OP_ARTIFACT_UPDATE, key).await?
        {
            return Ok(Written::Dropped(
                replay(&tx, &entry, Some(artifact_id)).await?,
            ));
        }

        if let Some(base) = opts.base_version
            && base != existing.version
            && !opts.force
        {
            return Err(Error::Conflict(format!(
                "artifact {artifact_id} is at version {}, not base version {base}",
                existing.version
            )));
        }

        let version = existing.version + 1;
        let rel = blob::promote(
            data_dir,
            &pending,
            &existing.project_id,
            artifact_id,
            version,
            &existing.kind,
        )?;
        promoted = Some(rel.clone());
        let envelope = envelope.resolve(existing.envelope.as_ref());
        let envelope_json = envelope.as_ref().map(|value| value.to_string());
        let protected = envelope_json.is_some();
        let label = match opts.label {
            None => existing.label.clone(),
            Some(None) | Some(Some("")) => None,
            Some(Some(l)) => check_label(Some(l))?,
        };

        tx.execute(
            "UPDATE artifacts SET current_ver = ?1, label = ?2, envelope = ?3, path = ?4, size_bytes = ?5, updated_at = ?6 WHERE id = ?7",
            vec![
                Value::Integer(version),
                optional_text(label.as_deref()),
                optional_text(envelope_json.as_deref()),
                Value::Text(rel.clone()),
                Value::Integer(content.len() as i64),
                Value::Text(updated_at.clone()),
                Value::Text(artifact_id.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
        insert_version(
            &tx,
            artifact_id,
            version,
            &existing.title,
            &existing.description,
            &existing.favicon,
            &existing.kind,
            label.as_deref(),
            protected,
            envelope_json.as_deref(),
            content.len() as i64,
            &rel,
            &updated_at,
        )
        .await?;
        index_doc(
            &tx,
            SearchDoc {
                doc_id: &artifact_doc_id(artifact_id),
                project_id: &existing.project_id,
                kind: "artifact",
                ref_id: artifact_id,
                session_id: opts.session_id,
                title: Some(&existing.title),
                body: searchable_body(content, protected),
                updated_at: &updated_at,
            },
        )
        .await?;
        let event_id = append_event(
            &tx,
            actor,
            &existing.project_id,
            "updated",
            artifact_id,
            &existing.title,
            &existing.kind,
            version,
            protected,
            label.as_deref(),
            opts.session_id,
        )
        .await?;
        if let Some(key) = idempotency_key {
            idempotency::record_artifact_update(
                &tx,
                &existing.project_id,
                key,
                &event_id,
                artifact_id,
                version,
                &updated_at,
            )
            .await?;
        }
        tx.commit().await.map_err(engine)?;
        Ok(Written::Kept(Artifact {
            id: artifact_id.to_string(),
            project_id: existing.project_id,
            session_id: existing.session_id,
            actor: existing.actor,
            title: existing.title,
            description: existing.description,
            favicon: existing.favicon,
            label,
            kind: existing.kind,
            version,
            protected,
            envelope,
            size_bytes: content.len() as i64,
            created_at: existing.created_at,
            updated_at,
            path: rel,
            comments_count: existing.comments_count,
            comments_open: existing.comments_open,
        }))
    }
    .await;

    // The write lock is gone by now, so only the pending name is safe to
    // remove: nobody else can name it. Once the rename ran, the version path
    // is left alone even on failure, because another update may already have
    // been granted the same number, renamed its own content onto that path and
    // committed. An unreferenced version file is harmless, and the next update
    // renames over it.
    let cleanup = || {
        if promoted.is_none() {
            let _ = blob::remove(data_dir, &pending);
        }
    };
    match write {
        Ok(Written::Kept(updated)) => Ok(updated),
        Ok(Written::Dropped(replayed)) => {
            cleanup();
            Ok(replayed)
        }
        Err(err) => {
            cleanup();
            Err(err)
        }
    }
}

/// Delete an artifact and its history.
///
/// The version rows, comment rows, search row, and idempotency rows go in the
/// same transaction as a `deleted` feed event, so a replay after the delete
/// records fresh instead of resolving to a missing row. The blob tree is
/// removed best effort after the commit; a missing tree is not an error.
pub async fn delete(
    db: &Database,
    data_dir: &Path,
    actor: &str,
    artifact_id: &str,
) -> Result<Artifact> {
    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let existing = row_on(&tx, artifact_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("artifact {artifact_id} not found")))?;

    tx.execute(
        "DELETE FROM artifact_versions WHERE artifact_id = ?1",
        vec![Value::Text(artifact_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM idempotency WHERE comment_id IN (SELECT id FROM comments WHERE artifact_id = ?1)",
        vec![Value::Text(artifact_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM comments WHERE artifact_id = ?1",
        vec![Value::Text(artifact_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM search_docs WHERE doc_id = ?1",
        vec![Value::Text(artifact_doc_id(artifact_id))],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM idempotency WHERE artifact_id = ?1",
        vec![Value::Text(artifact_id.to_string())],
    )
    .await
    .map_err(engine)?;
    tx.execute(
        "DELETE FROM artifacts WHERE id = ?1",
        vec![Value::Text(artifact_id.to_string())],
    )
    .await
    .map_err(engine)?;
    append_event(
        &tx,
        actor,
        &existing.project_id,
        "deleted",
        artifact_id,
        &existing.title,
        &existing.kind,
        existing.version,
        existing.protected,
        existing.label.as_deref(),
        None,
    )
    .await?;
    tx.commit().await.map_err(engine)?;

    let tree = format!("artifacts/{}/{}", existing.project_id, artifact_id);
    if let Err(err) = blob::remove_tree(data_dir, &tree) {
        tracing::warn!(artifact = artifact_id, error = %err, "artifact tree removal failed");
    }
    Ok(existing)
}

/// Read an artifact's metadata without its blob.
///
/// Reads that must authorize before touching up to the artifact cap use this,
/// then read the blob once access is granted.
pub async fn metadata(db: &Database, artifact_id: &str) -> Result<Artifact> {
    row(db, artifact_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("artifact {artifact_id} not found")))
}

/// Read an artifact's metadata and its current blob.
pub async fn get(db: &Database, data_dir: &Path, artifact_id: &str) -> Result<(Artifact, Vec<u8>)> {
    let artifact = metadata(db, artifact_id).await?;
    let bytes = blob::read(data_dir, &artifact.path)?;
    Ok((artifact, bytes))
}

/// Read one version of an artifact.
///
/// The returned metadata reflects the requested version row, not the current
/// pointer. A version below 1 is rejected; an unknown version is not found.
pub async fn get_at_version(
    db: &Database,
    data_dir: &Path,
    artifact_id: &str,
    version: i64,
) -> Result<(Artifact, Vec<u8>)> {
    if version < 1 {
        return Err(Error::InvalidArgument(format!(
            "artifact version must be 1 or more, got {version}"
        )));
    }
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT a.project_id,
                    v.title, v.description, v.favicon, v.kind, v.label,
                    v.encrypted, v.envelope, v.size_bytes, v.created_at, v.path,
                    a.session_id, a.actor,
                    (SELECT COUNT(*) FROM comments WHERE artifact_id = ?1) AS comments_count,
                    (SELECT COUNT(*) FROM comments WHERE artifact_id = ?1 AND done = 0) AS comments_open
             FROM artifact_versions v JOIN artifacts a ON a.id = v.artifact_id
             WHERE v.artifact_id = ?1 AND v.version = ?2",
            vec![
                Value::Text(artifact_id.to_string()),
                Value::Integer(version),
            ],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        // A missing parent and a missing version read the same: the surface
        // conceals the difference for callers without access.
        return Err(Error::NotFound(format!(
            "artifact {artifact_id} has no version {version}"
        )));
    };
    let project_id = required_text(&row, 0)?;
    let title = required_text(&row, 1)?;
    let description = text_at(&row, 2)?.unwrap_or_default();
    let favicon = text_at(&row, 3)?.unwrap_or_default();
    let kind = required_text(&row, 4)?;
    let label = text_at(&row, 5)?;
    let protected = int_at(&row, 6)? != 0;
    let envelope = match text_at(&row, 7)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored envelope is not JSON: {err}")))?,
        ),
        None => None,
    };
    let size_bytes = int_at(&row, 8)?;
    let created_at = required_text(&row, 9)?;
    let path = required_text(&row, 10)?;
    let session_id = text_at(&row, 11)?;
    let actor = text_at(&row, 12)?;
    let comments_count = int_at(&row, 13)?;
    let comments_open = int_at(&row, 14)?;
    let bytes = blob::read(data_dir, &path)?;
    Ok((
        Artifact {
            id: artifact_id.to_string(),
            project_id,
            session_id,
            actor,
            title,
            description,
            favicon,
            label,
            kind,
            version,
            protected,
            envelope,
            size_bytes,
            created_at: created_at.clone(),
            updated_at: created_at,
            path,
            comments_count,
            comments_open,
        },
        bytes,
    ))
}

/// List an artifact's versions, oldest first.
pub async fn list_versions(db: &Database, artifact_id: &str) -> Result<Vec<ArtifactVersion>> {
    let conn = super::connect(db)?;
    let has_parent = {
        let mut rows = conn
            .query(
                "SELECT id FROM artifacts WHERE id = ?1",
                vec![Value::Text(artifact_id.to_string())],
            )
            .await
            .map_err(engine)?;
        rows.next().await.map_err(engine)?.is_some()
    };
    if !has_parent {
        return Err(Error::NotFound(format!("artifact {artifact_id} not found")));
    }
    let mut rows = conn
        .query(
            "SELECT version, title, description, favicon, kind, label, encrypted, envelope, size_bytes, created_at
             FROM artifact_versions WHERE artifact_id = ?1 ORDER BY version ASC",
            vec![Value::Text(artifact_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut versions = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        versions.push(version_from_row(&row)?);
    }
    Ok(versions)
}

/// Read an artifact's blob, once its metadata has been authorized.
pub async fn read_blob(data_dir: &Path, artifact: &Artifact) -> Result<Vec<u8>> {
    blob::read(data_dir, &artifact.path)
}

/// List a project's artifacts, optionally filtered by session, most recently updated first.
pub async fn list_with_session(
    db: &Database,
    project_id: &str,
    session_id: Option<&str>,
) -> Result<Vec<Artifact>> {
    if let Some(sid) = session_id {
        let is_valid = match crate::store::sessions::get(db, sid).await? {
            Some(s) => s.deleted_at.is_none(),
            None => false,
        };
        if !is_valid {
            return Ok(Vec::new());
        }
    }
    let conn = super::connect(db)?;
    let (sql, params) = match session_id {
        Some(sid) => (
            "SELECT a.id, a.project_id, a.title, a.description, a.favicon, a.label, a.kind,
                    a.current_ver, a.envelope, a.size_bytes, a.created_at, a.updated_at, a.path, a.session_id, a.actor,
                    COUNT(c.id) AS comments_count,
                    COUNT(CASE WHEN c.done = 0 THEN 1 END) AS comments_open
             FROM artifacts a
             LEFT JOIN comments c ON c.artifact_id = a.id
             WHERE a.project_id = ?1 AND a.session_id = ?2
             GROUP BY a.id
             ORDER BY a.updated_at DESC",
            vec![Value::Text(project_id.to_string()), Value::Text(sid.to_string())],
        ),
        None => (
            "SELECT a.id, a.project_id, a.title, a.description, a.favicon, a.label, a.kind,
                    a.current_ver, a.envelope, a.size_bytes, a.created_at, a.updated_at, a.path, a.session_id, a.actor,
                    COUNT(c.id) AS comments_count,
                    COUNT(CASE WHEN c.done = 0 THEN 1 END) AS comments_open
             FROM artifacts a
             LEFT JOIN comments c ON c.artifact_id = a.id
             WHERE a.project_id = ?1
             GROUP BY a.id
             ORDER BY a.updated_at DESC",
            vec![Value::Text(project_id.to_string())],
        ),
    };
    let mut rows = conn.query(sql, params).await.map_err(engine)?;
    let mut artifacts = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        artifacts.push(artifact_from_row(&row)?);
    }
    Ok(artifacts)
}

/// List artifacts across all projects for a specific session.
pub async fn list_for_session(db: &Database, session_id: &str) -> Result<Vec<Artifact>> {
    let is_valid = match crate::store::sessions::get(db, session_id).await? {
        Some(s) => s.deleted_at.is_none(),
        None => false,
    };
    if !is_valid {
        return Ok(Vec::new());
    }
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT a.id, a.project_id, a.title, a.description, a.favicon, a.label, a.kind,
                    a.current_ver, a.envelope, a.size_bytes, a.created_at, a.updated_at, a.path, a.session_id, a.actor,
                    COUNT(c.id) AS comments_count,
                    COUNT(CASE WHEN c.done = 0 THEN 1 END) AS comments_open
             FROM artifacts a
             LEFT JOIN comments c ON c.artifact_id = a.id
             WHERE a.session_id = ?1
             GROUP BY a.id
             ORDER BY a.updated_at DESC",
            vec![Value::Text(session_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut artifacts = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        artifacts.push(artifact_from_row(&row)?);
    }
    Ok(artifacts)
}

/// List a project's artifacts, most recently updated first.
pub async fn list(db: &Database, project_id: &str) -> Result<Vec<Artifact>> {
    list_with_session(db, project_id, None).await
}

async fn row(db: &Database, artifact_id: &str) -> Result<Option<Artifact>> {
    let conn = super::connect(db)?;
    row_on(&conn, artifact_id).await
}

async fn row_on(conn: &turso::Connection, artifact_id: &str) -> Result<Option<Artifact>> {
    let mut rows = conn
        .query(
            "SELECT a.id, a.project_id, a.title, a.description, a.favicon, a.label, a.kind,
                    a.current_ver, a.envelope, a.size_bytes, a.created_at, a.updated_at, a.path, a.session_id, a.actor,
                    COUNT(c.id) AS comments_count,
                    COUNT(CASE WHEN c.done = 0 THEN 1 END) AS comments_open
             FROM artifacts a
             LEFT JOIN comments c ON c.artifact_id = a.id
             WHERE a.id = ?1
             GROUP BY a.id",
            vec![Value::Text(artifact_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(artifact_from_row(&row)?)),
        None => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_version(
    tx: &turso::transaction::Transaction<'_>,
    artifact_id: &str,
    version: i64,
    title: &str,
    description: &str,
    favicon: &str,
    kind: &str,
    label: Option<&str>,
    protected: bool,
    envelope_json: Option<&str>,
    size_bytes: i64,
    path: &str,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO artifact_versions(artifact_id, version, title, description, favicon, kind, label, encrypted, envelope, size_bytes, path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        vec![
            Value::Text(artifact_id.to_string()),
            Value::Integer(version),
            Value::Text(title.to_string()),
            Value::Text(description.to_string()),
            Value::Text(favicon.to_string()),
            Value::Text(kind.to_string()),
            optional_text(label),
            Value::Integer(i64::from(protected)),
            optional_text(envelope_json),
            Value::Integer(size_bytes),
            Value::Text(path.to_string()),
            Value::Text(created_at.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn append_event(
    tx: &turso::transaction::Transaction<'_>,
    actor: &str,
    project_id: &str,
    action: &str,
    artifact_id: &str,
    title: &str,
    kind: &str,
    version: i64,
    protected: bool,
    label: Option<&str>,
    session_id: Option<&str>,
) -> Result<String> {
    events::append_in_tx(
        tx,
        actor,
        None,
        NewEvent {
            project_id: project_id.to_string(),
            kind: "artifact".to_string(),
            summary: format!("{action} {title}"),
            payload: Some(serde_json::json!({
                "action": action,
                "artifact_id": artifact_id,
                "title": title,
                "kind": kind,
                "version": version,
                "protected": protected,
                "label": label,
            })),
            needs_action: false,
            thread_id: None,
            session_id: session_id.map(str::to_string),
        },
    )
    .await
}

/// Resolve a recorded key to the artifact it produced, so a retry returns the
/// original result. A key that named a different write is refused.
///
/// Only the id and version are authoritative on a replay; the other fields are
/// the artifact's current metadata, which a later update may have moved on.
async fn replay(
    conn: &turso::Connection,
    entry: &idempotency::Entry,
    expected: Option<&str>,
) -> Result<Artifact> {
    let artifact_id = entry.artifact_id.as_deref().ok_or_else(|| {
        Error::InvalidArgument("idempotency key was used for a different write".to_string())
    })?;
    if let Some(expected) = expected
        && expected != artifact_id
    {
        return Err(Error::InvalidArgument(
            "idempotency key was used for a different artifact".to_string(),
        ));
    }
    if let Some(target) = entry.target_id.as_deref()
        && target != artifact_id
    {
        return Err(Error::InvalidArgument(
            "idempotency key was used for a different artifact".to_string(),
        ));
    }
    let mut artifact = row_on(conn, artifact_id).await?.ok_or_else(|| {
        Error::Engine(format!(
            "an idempotency record points at missing artifact {artifact_id}"
        ))
    })?;
    if let Some(version) = entry.version {
        artifact.version = version;
    }
    Ok(artifact)
}

/// Resolve and check display metadata: title with its markdown fallback,
/// description, favicon, and label.
fn resolve_title(title: &str, kind: &str, content: &[u8]) -> Result<String> {
    let trimmed = title.trim();
    if !trimmed.is_empty() {
        return check_title(trimmed);
    }
    if kind == "markdown"
        && let Some(heading) = first_heading(content)
    {
        return check_title(&heading);
    }
    Err(Error::InvalidArgument(
        "artifact title is required (or include a markdown heading in the content)".to_string(),
    ))
}

fn check_title(title: &str) -> Result<String> {
    if title.chars().count() > TITLE_CHARS_MAX {
        return Err(Error::InvalidArgument(format!(
            "artifact title exceeds {TITLE_CHARS_MAX} characters"
        )));
    }
    Ok(title.to_string())
}

fn check_description(description: &str) -> Result<String> {
    if description.chars().count() > DESCRIPTION_CHARS_MAX {
        return Err(Error::InvalidArgument(format!(
            "artifact description exceeds {DESCRIPTION_CHARS_MAX} characters"
        )));
    }
    Ok(description.to_string())
}

fn check_favicon(favicon: &str) -> Result<String> {
    if favicon.chars().count() > FAVICON_CHARS_MAX {
        return Err(Error::InvalidArgument(format!(
            "artifact favicon exceeds {FAVICON_CHARS_MAX} characters"
        )));
    }
    Ok(favicon.to_string())
}

fn check_label(label: Option<&str>) -> Result<Option<String>> {
    let Some(label) = label else {
        return Ok(None);
    };
    if label.is_empty() {
        return Ok(None);
    }
    if label.len() > LABEL_BYTES_MAX {
        return Err(Error::InvalidArgument(format!(
            "artifact label exceeds {LABEL_BYTES_MAX} bytes"
        )));
    }
    Ok(Some(label.to_string()))
}

/// The first ATX heading of a markdown source, for the title fallback.
fn first_heading(content: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(content).ok()?;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        if let Some(rest) = trimmed[hashes..].strip_prefix(|ch| ch == ' ' || ch == '\t') {
            let heading = rest.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

fn artifact_from_row(row: &Row) -> Result<Artifact> {
    let envelope = match text_at(row, 8)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored envelope is not JSON: {err}")))?,
        ),
        None => None,
    };
    Ok(Artifact {
        id: required_text(row, 0)?,
        project_id: required_text(row, 1)?,
        title: required_text(row, 2)?,
        description: text_at(row, 3)?.unwrap_or_default(),
        favicon: text_at(row, 4)?.unwrap_or_default(),
        label: text_at(row, 5)?,
        kind: required_text(row, 6)?,
        version: int_at(row, 7)?,
        protected: envelope.is_some(),
        envelope,
        size_bytes: int_at(row, 9)?,
        created_at: required_text(row, 10)?,
        updated_at: required_text(row, 11)?,
        path: required_text(row, 12)?,
        session_id: text_at(row, 13)?,
        actor: text_at(row, 14)?,
        comments_count: int_at(row, 15)?,
        comments_open: int_at(row, 16)?,
    })
}

fn version_from_row(row: &Row) -> Result<ArtifactVersion> {
    let envelope = match text_at(row, 7)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored envelope is not JSON: {err}")))?,
        ),
        None => None,
    };
    Ok(ArtifactVersion {
        version: int_at(row, 0)?,
        title: required_text(row, 1)?,
        description: text_at(row, 2)?.unwrap_or_default(),
        favicon: text_at(row, 3)?.unwrap_or_default(),
        kind: required_text(row, 4)?,
        label: text_at(row, 5)?,
        protected: int_at(row, 6)? != 0,
        envelope,
        size_bytes: int_at(row, 8)?,
        created_at: required_text(row, 9)?,
    })
}

fn searchable_body(content: &[u8], protected: bool) -> &str {
    if protected {
        ""
    } else {
        std::str::from_utf8(content).unwrap_or("")
    }
}

fn artifact_doc_id(artifact_id: &str) -> String {
    format!("artifact:{artifact_id}")
}

fn required_text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?
        .ok_or_else(|| Error::Engine("artifact row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in an artifact column, found {other:?}"
        ))),
    }
}

fn int_at(row: &Row, index: usize) -> Result<i64> {
    match row.get_value(index).map_err(engine)? {
        Value::Integer(value) => Ok(value),
        other => Err(Error::Engine(format!(
            "expected an integer in an artifact column, found {other:?}"
        ))),
    }
}

fn optional_text(value: Option<&str>) -> Value {
    match value {
        Some(text) => Value::Text(text.to_string()),
        None => Value::Null,
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
