//! Discussion on knowledge base pages.
//!
//! A comment belongs to one project and one canonical page path, since the
//! knowledge base has no page table to hang it off. Posting, resolving and
//! deleting are plain writes; who may call them is decided by the surfaces,
//! which also set the author from the authenticated principal. The path is
//! normalised by the caller before it arrives here, so every surface keys on
//! the same canonical spelling the knowledge base itself uses.

use turso::{Database, Row, Value};

use crate::error::{Error, Result};

/// Maximum characters of a comment body.
pub const BODY_CHARS_MAX: usize = 2000;
/// Maximum characters of a comment author.
pub const AUTHOR_CHARS_MAX: usize = 200;
/// Maximum bytes of a serialised anchor.
pub const ANCHOR_BYTES_MAX: usize = 2048;

/// One comment on a knowledge base page.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageComment {
    pub id: String,
    pub project_id: String,
    pub path: String,
    pub author: String,
    pub body: String,
    pub anchor: Option<serde_json::Value>,
    pub done: bool,
    pub created_at: String,
}

/// Post a comment on a page and return the stored row.
pub async fn add_comment(
    db: &Database,
    project_id: &str,
    path: &str,
    author: &str,
    body: &str,
    anchor: Option<serde_json::Value>,
) -> Result<PageComment> {
    let author = check_author(author)?;
    let body = check_body(body)?;
    let anchor = check_anchor(anchor)?;

    let id = crate::store::next_id();
    let created_at = crate::store::now_rfc3339();
    let conn = super::connect(db)?;
    conn.execute(
        "INSERT INTO kb_comments(id, project_id, path, author, body, anchor, done, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        vec![
            Value::Text(id.clone()),
            Value::Text(project_id.to_string()),
            Value::Text(path.to_string()),
            Value::Text(author),
            Value::Text(body),
            optional_text(anchor.as_ref().map(|anchor| anchor.to_string()).as_deref()),
            Value::Text(created_at),
        ],
    )
    .await
    .map_err(engine)?;

    get_comment(db, &id).await
}

/// List a page's comments, oldest first.
///
/// A project or page with no comments is an empty list, not a refusal: a
/// thread is a list, and a page that never had one is not missing.
pub async fn list_comments(
    db: &Database,
    project_id: &str,
    path: &str,
) -> Result<Vec<PageComment>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT id, project_id, path, author, body, anchor, done, created_at
             FROM kb_comments WHERE project_id = ?1 AND path = ?2
             ORDER BY created_at ASC, id ASC",
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(path.to_string()),
            ],
        )
        .await
        .map_err(engine)?;
    let mut comments = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        comments.push(comment_from_row(&row)?);
    }
    Ok(comments)
}

/// Read one comment by id.
pub async fn get_comment(db: &Database, comment_id: &str) -> Result<PageComment> {
    let conn = super::connect(db)?;
    comment_on(&conn, comment_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("comment {comment_id} not found")))
}

/// Mark a comment done or reopen it, returning the updated row.
pub async fn set_comment_done(db: &Database, comment_id: &str, done: bool) -> Result<PageComment> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "UPDATE kb_comments SET done = ?1 WHERE id = ?2
             RETURNING id, project_id, path, author, body, anchor, done, created_at",
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
            "DELETE FROM kb_comments WHERE id = ?1",
            vec![Value::Text(comment_id.to_string())],
        )
        .await
        .map_err(engine)?;
    if changed == 0 {
        return Err(Error::NotFound(format!("comment {comment_id} not found")));
    }
    Ok(())
}

async fn comment_on(conn: &turso::Connection, comment_id: &str) -> Result<Option<PageComment>> {
    let mut rows = conn
        .query(
            "SELECT id, project_id, path, author, body, anchor, done, created_at
             FROM kb_comments WHERE id = ?1",
            vec![Value::Text(comment_id.to_string())],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => Ok(Some(comment_from_row(&row)?)),
        None => Ok(None),
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

fn check_anchor(anchor: Option<serde_json::Value>) -> Result<Option<serde_json::Value>> {
    let Some(anchor) = anchor else {
        return Ok(None);
    };
    if anchor.is_null() {
        return Ok(None);
    }
    if anchor.to_string().len() > ANCHOR_BYTES_MAX {
        return Err(Error::InvalidArgument(format!(
            "anchor exceeds {ANCHOR_BYTES_MAX} bytes"
        )));
    }
    Ok(Some(anchor))
}

fn comment_from_row(row: &Row) -> Result<PageComment> {
    let anchor = match text_at(row, 5)? {
        Some(json) => Some(
            serde_json::from_str(&json)
                .map_err(|err| Error::Engine(format!("stored anchor is not JSON: {err}")))?,
        ),
        None => None,
    };
    let done = match row.get_value(6).map_err(engine)? {
        Value::Integer(done) => done != 0,
        other => {
            return Err(Error::Engine(format!(
                "expected an integer in a page comment column, found {other:?}"
            )));
        }
    };
    Ok(PageComment {
        id: required_text(row, 0)?,
        project_id: required_text(row, 1)?,
        path: required_text(row, 2)?,
        author: required_text(row, 3)?,
        body: required_text(row, 4)?,
        anchor,
        done,
        created_at: required_text(row, 7)?,
    })
}

fn required_text(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?
        .ok_or_else(|| Error::Engine("page comment row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in a page comment column, found {other:?}"
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
