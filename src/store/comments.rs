//! Discussion on artifacts.
//!
//! A comment belongs to one artifact. Posting, resolving, and deleting are
//! plain writes; who may call them is decided by the surfaces, which also
//! set the author from the authenticated principal. A text anchor quotes
//! artifact content, so it is refused on versions the server holds only as
//! ciphertext: accepting it would copy plaintext into this table and break
//! the encryption posture.

use turso::{Database, Row, Value};

use crate::error::{Error, Result};
use crate::store::idempotency;
use crate::store::identity;

/// Maximum characters of a comment body.
pub const BODY_CHARS_MAX: usize = 2000;
/// Maximum characters of a comment author.
pub const AUTHOR_CHARS_MAX: usize = 200;
/// Maximum bytes of a serialised anchor.
pub const ANCHOR_BYTES_MAX: usize = 2048;
/// Maximum characters of a text anchor quote.
pub const QUOTE_CHARS_MAX: usize = 2000;

/// One comment on an artifact. The delete token hash stays internal.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Comment {
    pub id: String,
    pub artifact_id: String,
    pub author: String,
    pub body: String,
    pub anchor: Option<serde_json::Value>,
    pub anchor_version: Option<i64>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_token_hash: Option<String>,
    pub created_at: String,
}

/// A validated anchor: a canvas point or a verbatim quote.
#[derive(Debug, Clone)]
pub enum AnchorInput {
    Point { x: f64, y: f64 },
    Text { quote: String },
}

impl AnchorInput {
    fn into_json(self) -> serde_json::Value {
        match self {
            Self::Point { x, y } => serde_json::json!({
                "mode": "point", "x": x, "y": y,
            }),
            Self::Text { quote } => serde_json::json!({
                "mode": "text", "quote": quote,
            }),
        }
    }
}

/// Generate a per-comment delete token: the plaintext goes back to the
/// poster once, and only the hash is stored.
pub fn generate_delete_token() -> (String, String) {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let plaintext: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let hash = identity::hash_token(&plaintext);
    (plaintext, hash)
}

/// Post a comment: validate, stamp the version, record the row, and return
/// it with whether an idempotency key replayed an earlier post.
///
/// A replay returns the recorded comment; its delete token is not
/// re-issued, so callers keep the token from the first response.
#[allow(clippy::too_many_arguments)]
pub async fn add_comment(
    db: &Database,
    artifact_id: &str,
    author: &str,
    body: &str,
    anchor: Option<AnchorInput>,
    anchor_version: Option<i64>,
    delete_token_hash: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(Comment, bool)> {
    let author = check_author(author)?;
    let body = check_body(body)?;
    let anchor_json = check_anchor(anchor)?;

    let mut conn = super::connect(db)?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .map_err(engine)?;

    let (project_id, current_ver) = artifact_version(&tx, artifact_id).await?;
    if let Some(key) = idempotency_key
        && let Some(entry) = idempotency::lookup_entry(&tx, &project_id, "comment", key).await?
    {
        let comment_id = entry.comment_id.as_deref().ok_or_else(|| {
            Error::InvalidArgument("idempotency key was used for a different write".to_string())
        })?;
        let comment = comment_on(&tx, comment_id).await?.ok_or_else(|| {
            Error::Engine(format!(
                "an idempotency record points at missing comment {comment_id}"
            ))
        })?;
        if comment.artifact_id != artifact_id {
            return Err(Error::InvalidArgument(
                "idempotency key was used for a different write".to_string(),
            ));
        }
        return Ok((comment, true));
    }

    // A forged version lands on the current one rather than addressing a
    // version that does not exist yet.
    let stamped = match anchor_version {
        None => current_ver,
        Some(version) => version.clamp(1, current_ver),
    };
    if anchor_json
        .as_ref()
        .and_then(|anchor| anchor.get("mode"))
        .is_some_and(|mode| mode == "text")
        && version_protected(&tx, artifact_id, stamped).await?
    {
        return Err(Error::InvalidArgument(
            "text anchors are not allowed on protected versions".to_string(),
        ));
    }

    let id = crate::store::next_id();
    let created_at = crate::store::now_rfc3339();
    tx.execute(
        "INSERT INTO comments(id, artifact_id, author, body, anchor, anchor_version, done, delete_token_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
        vec![
            Value::Text(id.clone()),
            Value::Text(artifact_id.to_string()),
            Value::Text(author.clone()),
            Value::Text(body.clone()),
            optional_text(anchor_json.as_ref().map(|anchor| anchor.to_string()).as_deref()),
            Value::Integer(stamped),
            optional_text(delete_token_hash),
            Value::Text(created_at.clone()),
        ],
    )
    .await
    .map_err(engine)?;
    if let Some(key) = idempotency_key {
        idempotency::record_comment(&tx, &project_id, key, &id, &created_at).await?;
    }
    tx.commit().await.map_err(engine)?;

    get_comment(db, &id).await.map(|comment| (comment, false))
}

/// List an artifact's comments, oldest first.
pub async fn list_comments(db: &Database, artifact_id: &str) -> Result<Vec<Comment>> {
    let conn = super::connect(db)?;
    if !artifact_exists(&conn, artifact_id).await? {
        return Err(Error::NotFound(format!("artifact {artifact_id} not found")));
    }
    let mut rows = conn
        .query(
            "SELECT id, artifact_id, author, body, anchor, anchor_version, done, delete_token_hash, created_at
             FROM comments WHERE artifact_id = ?1 ORDER BY created_at ASC, id ASC",
            vec![Value::Text(artifact_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let mut comments = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        comments.push(comment_from_row(&row)?);
    }
    Ok(comments)
}

/// Read one comment by id, with the artifact it belongs to.
pub async fn get_comment(db: &Database, comment_id: &str) -> Result<Comment> {
    let conn = super::connect(db)?;
    comment_on(&conn, comment_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("comment {comment_id} not found")))
}

/// Mark a comment done or reopen it, returning the updated row.
pub async fn set_comment_done(db: &Database, comment_id: &str, done: bool) -> Result<Comment> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "UPDATE comments SET done = ?1 WHERE id = ?2 RETURNING id, artifact_id, author, body, anchor, anchor_version, done, delete_token_hash, created_at",
            vec![
                Value::Integer(i64::from(done)),
                Value::Text(comment_id.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => comment_from_row(&row),
        None => Err(Error::NotFound(format!("comment {comment_id} not found"))),
    }
}

/// Delete one comment. Replies are flat, so nothing else hangs off it.
pub async fn delete_comment(db: &Database, comment_id: &str) -> Result<()> {
    let conn = super::connect(db)?;
    let changed = conn
        .execute(
            "DELETE FROM comments WHERE id = ?1",
            vec![Value::Text(comment_id.to_string())],
        )
        .await
        .map_err(engine)?;
    if changed == 0 {
        return Err(Error::NotFound(format!("comment {comment_id} not found")));
    }
    Ok(())
}

async fn comment_on(conn: &turso::Connection, comment_id: &str) -> Result<Option<Comment>> {
    let mut rows = conn
        .query(
            "SELECT id, artifact_id, author, body, anchor, anchor_version, done, delete_token_hash, created_at
             FROM comments WHERE id = ?1",
            vec![Value::Text(comment_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(comment_from_row(&row)?)),
        None => Ok(None),
    }
}

async fn artifact_exists(conn: &turso::Connection, artifact_id: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT id FROM artifacts WHERE id = ?1",
            vec![Value::Text(artifact_id.to_string())],
        )
        .await
        .map_err(engine)?;
    Ok(rows.next().await.map_err(engine)?.is_some())
}

/// The owning project and current version of an artifact.
async fn artifact_version(
    tx: &turso::transaction::Transaction<'_>,
    artifact_id: &str,
) -> Result<(String, i64)> {
    let mut rows = tx
        .query(
            "SELECT project_id, current_ver FROM artifacts WHERE id = ?1",
            vec![Value::Text(artifact_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => {
            let project_id = match row.get_value(0).map_err(engine)? {
                Value::Text(text) => text,
                _ => {
                    return Err(Error::Engine(
                        "artifact row is missing its project".to_string(),
                    ));
                }
            };
            let current_ver = match row.get_value(1).map_err(engine)? {
                Value::Integer(version) => version,
                _ => {
                    return Err(Error::Engine(
                        "artifact row is missing its version".to_string(),
                    ));
                }
            };
            Ok((project_id, current_ver))
        }
        None => Err(Error::NotFound(format!("artifact {artifact_id} not found"))),
    }
}

/// Whether one version row is protected. A missing row fails closed: it is
/// treated as protected rather than assumed public.
async fn version_protected(
    tx: &turso::transaction::Transaction<'_>,
    artifact_id: &str,
    version: i64,
) -> Result<bool> {
    let mut rows = tx
        .query(
            "SELECT encrypted FROM artifact_versions WHERE artifact_id = ?1 AND version = ?2",
            vec![
                Value::Text(artifact_id.to_string()),
                Value::Integer(version),
            ],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => match row.get_value(0).map_err(engine)? {
            Value::Integer(encrypted) => Ok(encrypted != 0),
            _ => Ok(true),
        },
        None => Ok(true),
    }
}

fn check_author(author: &str) -> Result<String> {
    let trimmed = author.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidArgument(
            "comment author is required".to_string(),
        ));
    }
    if trimmed.chars().count() > AUTHOR_CHARS_MAX {
        return Err(Error::InvalidArgument(format!(
            "comment author exceeds {AUTHOR_CHARS_MAX} characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn check_body(body: &str) -> Result<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidArgument(
            "comment body is required".to_string(),
        ));
    }
    if trimmed.chars().count() > BODY_CHARS_MAX {
        return Err(Error::InvalidArgument(format!(
            "comment body exceeds {BODY_CHARS_MAX} characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn check_anchor(anchor: Option<AnchorInput>) -> Result<Option<serde_json::Value>> {
    let Some(anchor) = anchor else {
        return Ok(None);
    };
    match &anchor {
        AnchorInput::Point { x, y } => {
            if !x.is_finite() || !y.is_finite() {
                return Err(Error::InvalidArgument(
                    "point anchor coordinates must be finite numbers".to_string(),
                ));
            }
        }
        AnchorInput::Text { quote } => {
            let quote = quote.trim();
            if quote.is_empty() {
                return Err(Error::InvalidArgument(
                    "text anchor quote is required".to_string(),
                ));
            }
            if quote.chars().count() > QUOTE_CHARS_MAX {
                return Err(Error::InvalidArgument(format!(
                    "text anchor quote exceeds {QUOTE_CHARS_MAX} characters"
                )));
            }
        }
    }
    let json = anchor.into_json();
    if json.to_string().len() > ANCHOR_BYTES_MAX {
        return Err(Error::InvalidArgument(format!(
            "anchor exceeds {ANCHOR_BYTES_MAX} bytes"
        )));
    }
    Ok(Some(json))
}

fn comment_from_row(row: &Row) -> Result<Comment> {
    let anchor = match text_at(row, 4)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored anchor is not JSON: {err}")))?,
        ),
        None => None,
    };
    let anchor_version = match row.get_value(5).map_err(engine)? {
        Value::Integer(version) => Some(version),
        Value::Null => None,
        other => {
            return Err(Error::Engine(format!(
                "expected an integer or null in a comment column, found {other:?}"
            )));
        }
    };
    let done = match row.get_value(6).map_err(engine)? {
        Value::Integer(done) => done != 0,
        other => {
            return Err(Error::Engine(format!(
                "expected an integer in a comment column, found {other:?}"
            )));
        }
    };
    Ok(Comment {
        id: required_text(row, 0)?,
        artifact_id: required_text(row, 1)?,
        author: required_text(row, 2)?,
        body: required_text(row, 3)?,
        anchor,
        anchor_version,
        done,
        delete_token_hash: text_at(row, 7)?,
        created_at: required_text(row, 8)?,
    })
}

fn required_text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("comment row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in a comment column, found {other:?}"
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
