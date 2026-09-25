//! Idempotency keys, so a retried write does not duplicate an event or an
//! artifact version.
//!
//! A key is scoped to the operation that used it, so the same string reused
//! for a different operation resolves to nothing rather than to that other
//! operation's result.

use turso::{Connection, Value};

use crate::error::{Error, Result};

/// The operations an idempotency key belongs to.
pub const OP_DECISION: &str = "decision";
pub const OP_ANSWER: &str = "answer";
pub const OP_ARTIFACT_PUBLISH: &str = "artifact:publish";
pub const OP_ARTIFACT_UPDATE: &str = "artifact:update";
pub const OP_COMMENT: &str = "comment";

/// Scope a generic event's idempotency key to its kind.
pub fn op_event(kind: &str) -> String {
    format!("event:{kind}")
}

/// What a key produced, when it was recorded.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The feed event the write appended, if any.
    pub event_id: Option<String>,
    /// The artifact the write touched, if any.
    pub artifact_id: Option<String>,
    /// The artifact version the write granted, if any.
    pub version: Option<i64>,
    /// The comment the write posted, if any.
    pub comment_id: Option<String>,
    /// The target entity the write was bound to, if any.
    pub target_id: Option<String>,
}

/// Return what a key produced for one operation, if it is recorded.
pub async fn lookup_entry(
    conn: &Connection,
    project_id: &str,
    operation: &str,
    key: &str,
) -> Result<Option<Entry>> {
    let mut rows = conn
        .query(
            "SELECT event_id, artifact_id, version, comment_id, target_id FROM idempotency
             WHERE project_id = ?1 AND operation = ?2 AND idempotency_key = ?3",
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(operation.to_string()),
                Value::Text(key.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    let Some(row) = rows.next().await.map_err(engine)? else {
        return Ok(None);
    };
    let text = |value: turso::Value| match value {
        Value::Text(text) => Some(text),
        _ => None,
    };
    let version = match row.get_value(2).map_err(engine)? {
        Value::Integer(value) => Some(value),
        _ => None,
    };
    Ok(Some(Entry {
        event_id: text(row.get_value(0).map_err(engine)?),
        artifact_id: text(row.get_value(1).map_err(engine)?),
        version,
        comment_id: text(row.get_value(3).map_err(engine)?),
        target_id: text(row.get_value(4).map_err(engine)?),
    }))
}

/// Return the event id already recorded for an event key of the given kind, if any.
pub async fn lookup(
    conn: &Connection,
    project_id: &str,
    kind: &str,
    key: &str,
) -> Result<Option<String>> {
    let op = op_event(kind);
    Ok(lookup_entry(conn, project_id, &op, key)
        .await?
        .and_then(|entry| entry.event_id))
}

/// Record an event key against the event it produced.
pub async fn record(
    conn: &Connection,
    project_id: &str,
    kind: &str,
    key: &str,
    event_id: &str,
    created_at: &str,
) -> Result<()> {
    let op = op_event(kind);
    record_row(
        conn,
        project_id,
        &op,
        key,
        Some(event_id),
        None,
        None,
        None,
        None,
        created_at,
    )
    .await
}

/// Record an answer key against the answer it produced and the question it answered.
pub async fn record_answer(
    conn: &Connection,
    project_id: &str,
    key: &str,
    event_id: &str,
    question_id: &str,
    created_at: &str,
) -> Result<()> {
    record_row(
        conn,
        project_id,
        OP_ANSWER,
        key,
        Some(event_id),
        None,
        None,
        None,
        Some(question_id),
        created_at,
    )
    .await
}

/// Record a decision key against the answer it produced and the approval it decided.
pub async fn record_decision(
    conn: &Connection,
    project_id: &str,
    key: &str,
    event_id: &str,
    approval_id: &str,
    created_at: &str,
) -> Result<()> {
    record_row(
        conn,
        project_id,
        OP_DECISION,
        key,
        Some(event_id),
        None,
        None,
        None,
        Some(approval_id),
        created_at,
    )
    .await
}

/// Record a comment key against the comment it posted. Comments append no
/// feed event, so only the comment id is recorded.
pub async fn record_comment(
    conn: &Connection,
    project_id: &str,
    key: &str,
    comment_id: &str,
    artifact_id: &str,
    created_at: &str,
) -> Result<()> {
    record_row(
        conn,
        project_id,
        OP_COMMENT,
        key,
        None,
        None,
        None,
        Some(comment_id),
        Some(artifact_id),
        created_at,
    )
    .await
}

/// Record an artifact publish key against the artifact version and event it produced.
pub async fn record_artifact_publish(
    conn: &Connection,
    project_id: &str,
    key: &str,
    event_id: &str,
    artifact_id: &str,
    version: i64,
    created_at: &str,
) -> Result<()> {
    record_row(
        conn,
        project_id,
        OP_ARTIFACT_PUBLISH,
        key,
        Some(event_id),
        Some(artifact_id),
        Some(version),
        None,
        Some(artifact_id),
        created_at,
    )
    .await
}

/// Record an artifact update key against the artifact version and event it produced.
pub async fn record_artifact_update(
    conn: &Connection,
    project_id: &str,
    key: &str,
    event_id: &str,
    artifact_id: &str,
    version: i64,
    created_at: &str,
) -> Result<()> {
    record_row(
        conn,
        project_id,
        OP_ARTIFACT_UPDATE,
        key,
        Some(event_id),
        Some(artifact_id),
        Some(version),
        None,
        Some(artifact_id),
        created_at,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_row(
    conn: &Connection,
    project_id: &str,
    operation: &str,
    key: &str,
    event_id: Option<&str>,
    artifact_id: Option<&str>,
    version: Option<i64>,
    comment_id: Option<&str>,
    target_id: Option<&str>,
    created_at: &str,
) -> Result<()> {
    let optional_text = |value: Option<&str>| match value {
        Some(text) => Value::Text(text.to_string()),
        None => Value::Null,
    };
    conn.execute(
        "INSERT INTO idempotency(project_id, operation, idempotency_key, event_id, artifact_id, version, comment_id, target_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        vec![
            Value::Text(project_id.to_string()),
            Value::Text(operation.to_string()),
            Value::Text(key.to_string()),
            optional_text(event_id),
            optional_text(artifact_id),
            match version {
                Some(version) => Value::Integer(version),
                None => Value::Null,
            },
            optional_text(comment_id),
            optional_text(target_id),
            Value::Text(created_at.to_string()),
        ],
    )
    .await
    .map_err(engine)?;
    Ok(())
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}
