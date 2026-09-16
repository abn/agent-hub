//! The search corpus write path.
//!
//! One row per indexable document, in the hub store, written through by the
//! wrapper on every change. Querying and ranking live in the search module of
//! a later change; this is the seam every writer calls so nothing is written
//! without being indexed.

use turso::{Connection, Value};

use crate::error::{Error, Result};

/// A document to index.
pub struct SearchDoc<'a> {
    /// Stable id, for example `event:<ulid>`.
    pub doc_id: &'a str,
    /// Owning project.
    pub project_id: &'a str,
    /// Corpus family: `feed`, `artifact`, or `brain`.
    pub kind: &'a str,
    /// Id of the underlying row.
    pub ref_id: &'a str,
    /// Session this belongs to, when it is brain content.
    pub session_id: Option<&'a str>,
    /// Short title, for listings.
    pub title: Option<&'a str>,
    /// Searchable body text.
    pub body: &'a str,
    /// RFC 3339 timestamp of the change.
    pub updated_at: &'a str,
}

/// Insert or refresh one document in the corpus.
pub async fn index_doc(conn: &Connection, doc: SearchDoc<'_>) -> Result<()> {
    conn.execute(
        "INSERT INTO search_docs(doc_id, project_id, type, ref_id, session_id, title, body, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(doc_id) DO UPDATE SET
           title = excluded.title,
           body = excluded.body,
           updated_at = excluded.updated_at",
        vec![
            Value::Text(doc.doc_id.to_string()),
            Value::Text(doc.project_id.to_string()),
            Value::Text(doc.kind.to_string()),
            Value::Text(doc.ref_id.to_string()),
            optional_text(doc.session_id),
            optional_text(doc.title),
            Value::Text(doc.body.to_string()),
            Value::Text(doc.updated_at.to_string()),
        ],
    )
    .await
    .map_err(|err| Error::Engine(err.to_string()))?;
    Ok(())
}

fn optional_text(value: Option<&str>) -> Value {
    match value {
        Some(text) => Value::Text(text.to_string()),
        None => Value::Null,
    }
}
