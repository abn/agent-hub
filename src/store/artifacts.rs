//! Artifacts: metadata in the hub store, blobs on the data volume.
//!
//! A protected artifact carries an encryption envelope and ciphertext; the
//! server never sees its plaintext. Publishing or updating appends a feed
//! event and refreshes the search corpus.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Row, Value};

use crate::blob;
use crate::error::{Error, Result};
use crate::limits;
use crate::store::events::{self, NewEvent};
use crate::store::idempotency;
use crate::store::search::{SearchDoc, index_doc};

/// Artifact metadata. The blob path stays internal.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub kind: String,
    pub version: i64,
    pub protected: bool,
    pub envelope: Option<serde_json::Value>,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip)]
    pub path: String,
}

/// A new artifact to publish.
pub struct NewArtifact<'a> {
    pub actor: &'a str,
    pub project_id: &'a str,
    pub title: &'a str,
    pub kind: &'a str,
    pub content: &'a [u8],
    /// The encryption envelope when the artifact is protected.
    pub envelope: Option<serde_json::Value>,
}

/// Publish an artifact: write the blob, record the metadata, append a feed
/// event, and index it.
pub async fn publish(
    db: &Database,
    data_dir: &Path,
    artifact: NewArtifact<'_>,
    idempotency_key: Option<&str>,
) -> Result<Artifact> {
    limits::check_artifact(artifact.content.len())?;

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    // A retry with the same key returns what the first call produced, so a
    // dropped response does not leave a duplicate artifact.
    if let Some(key) = idempotency_key
        && let Some(entry) =
            idempotency::lookup_entry(&tx, artifact.project_id, "artifact", key).await?
    {
        return replay(&tx, &entry, None).await;
    }

    let id = ulid::Ulid::generate().to_string();
    let created_at = crate::store::now_rfc3339();
    let rel = blob::write(
        data_dir,
        artifact.project_id,
        &id,
        1,
        artifact.kind,
        artifact.content,
    )?;

    let envelope_json = artifact.envelope.as_ref().map(|value| value.to_string());
    let protected = envelope_json.is_some();

    let write = async {
        tx.execute(
            "INSERT INTO artifacts(id, project_id, title, kind, current_ver, envelope, path, size_bytes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?8)",
            vec![
                Value::Text(id.clone()),
                Value::Text(artifact.project_id.to_string()),
                Value::Text(artifact.title.to_string()),
                Value::Text(artifact.kind.to_string()),
                optional_text(envelope_json.as_deref()),
                Value::Text(rel.clone()),
                Value::Integer(artifact.content.len() as i64),
                Value::Text(created_at.clone()),
            ],
        )
        .await
        .map_err(engine)?;
        index_doc(
            &tx,
            SearchDoc {
                doc_id: &artifact_doc_id(&id),
                project_id: artifact.project_id,
                kind: "artifact",
                ref_id: &id,
                session_id: None,
                title: Some(artifact.title),
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
            artifact.title,
            artifact.kind,
            1,
            protected,
        )
        .await?;
        if let Some(key) = idempotency_key {
            idempotency::record_artifact(&tx, artifact.project_id, key, &event_id, &id, 1, &created_at)
                .await?;
        }
        tx.commit().await.map_err(engine)
    }
    .await;

    if let Err(err) = write {
        let _ = blob::remove(data_dir, &rel);
        return Err(err);
    }

    get(db, data_dir, &id).await.map(|(artifact, _)| artifact)
}

/// Publish a new version of an existing artifact.
///
/// The version bump is read inside the immediate transaction, so concurrent
/// updates serialise and each writes a distinct version file.
pub async fn update(
    db: &Database,
    data_dir: &Path,
    actor: &str,
    artifact_id: &str,
    content: &[u8],
    envelope: Option<serde_json::Value>,
    idempotency_key: Option<&str>,
) -> Result<Artifact> {
    limits::check_artifact(content.len())?;

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;
    let existing = row_on(&tx, artifact_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("artifact {artifact_id} not found")))?;

    // A retry with the same key returns the version the first call granted.
    if let Some(key) = idempotency_key
        && let Some(entry) =
            idempotency::lookup_entry(&tx, &existing.project_id, "artifact", key).await?
    {
        return replay(&tx, &entry, Some(artifact_id)).await;
    }

    let version = existing.version + 1;
    let rel = blob::write(
        data_dir,
        &existing.project_id,
        artifact_id,
        version,
        &existing.kind,
        content,
    )?;
    let envelope = envelope.or(existing.envelope.clone());
    let envelope_json = envelope.as_ref().map(|value| value.to_string());
    let protected = envelope_json.is_some();
    let updated_at = crate::store::now_rfc3339();

    let write = async {
        tx.execute(
            "UPDATE artifacts SET current_ver = ?1, envelope = ?2, path = ?3, size_bytes = ?4, updated_at = ?5 WHERE id = ?6",
            vec![
                Value::Integer(version),
                optional_text(envelope_json.as_deref()),
                Value::Text(rel.clone()),
                Value::Integer(content.len() as i64),
                Value::Text(updated_at.clone()),
                Value::Text(artifact_id.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
        index_doc(
            &tx,
            SearchDoc {
                doc_id: &artifact_doc_id(artifact_id),
                project_id: &existing.project_id,
                kind: "artifact",
                ref_id: artifact_id,
                session_id: None,
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
        )
        .await?;
        if let Some(key) = idempotency_key {
            idempotency::record_artifact(
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
        tx.commit().await.map_err(engine)
    }
    .await;

    if let Err(err) = write {
        let _ = blob::remove(data_dir, &rel);
        return Err(err);
    }

    get(db, data_dir, artifact_id)
        .await
        .map(|(artifact, _)| artifact)
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

/// Read an artifact's blob, once its metadata has been authorized.
pub async fn read_blob(data_dir: &Path, artifact: &Artifact) -> Result<Vec<u8>> {
    blob::read(data_dir, &artifact.path)
}

/// List a project's artifacts, most recently updated first.
pub async fn list(db: &Database, project_id: &str) -> Result<Vec<Artifact>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, project_id, title, kind, current_ver, envelope, size_bytes, created_at, updated_at, path
             FROM artifacts WHERE project_id = ?1 ORDER BY updated_at DESC",
            vec![Value::Text(project_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut artifacts = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        artifacts.push(artifact_from_row(&row)?);
    }
    Ok(artifacts)
}

async fn row(db: &Database, artifact_id: &str) -> Result<Option<Artifact>> {
    let conn = super::connect(db)?;
    row_on(&conn, artifact_id).await
}

async fn row_on(conn: &turso::Connection, artifact_id: &str) -> Result<Option<Artifact>> {
    let mut rows = conn
        .query(
            "SELECT id, project_id, title, kind, current_ver, envelope, size_bytes, created_at, updated_at, path
             FROM artifacts WHERE id = ?1",
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
            })),
            needs_action: false,
            thread_id: None,
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
    tx: &turso::transaction::Transaction<'_>,
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
    let mut artifact = row_on(tx, artifact_id).await?.ok_or_else(|| {
        Error::Engine(format!(
            "an idempotency record points at missing artifact {artifact_id}"
        ))
    })?;
    if let Some(version) = entry.version {
        artifact.version = version;
    }
    Ok(artifact)
}

fn artifact_from_row(row: &Row) -> Result<Artifact> {
    let envelope = match text_at(row, 5)? {
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
        kind: required_text(row, 3)?,
        version: int_at(row, 4)?,
        protected: envelope.is_some(),
        envelope,
        size_bytes: int_at(row, 6)?,
        created_at: required_text(row, 7)?,
        updated_at: required_text(row, 8)?,
        path: required_text(row, 9)?,
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
