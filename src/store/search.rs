//! The search corpus write path.
//!
//! One row per indexable document, in the hub store, written through by the
//! wrapper on every change. Querying and ranking live in the search module of
//! a later change; this is the seam every writer calls so nothing is written
//! without being indexed.

use turso::{Connection, Database, Row, Value};

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

/// A search request over the corpus.
pub struct SearchQuery<'a> {
    /// The text to match against titles and bodies.
    pub text: &'a str,
    /// Restrict to one project.
    pub project_id: Option<&'a str>,
    /// Restrict to one corpus family: `feed`, `artifact`, or `brain`.
    pub kind: Option<&'a str>,
    /// Maximum hits to return.
    pub limit: i64,
}

/// One search hit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub doc_id: String,
    pub project_id: String,
    pub kind: String,
    pub ref_id: String,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub snippet: String,
    pub updated_at: String,
}

/// Query the corpus, ranked by text relevance with a recency tiebreak.
pub async fn query(db: &Database, search: &SearchQuery<'_>) -> Result<Vec<SearchHit>> {
    if search.text.trim().is_empty() {
        return Err(Error::InvalidArgument(
            "a search query is required".to_string(),
        ));
    }
    if let Some(kind) = search.kind {
        validate_kind(kind)?;
    }
    let limit = search.limit.clamp(1, crate::limits::FEED_LIMIT_MAX);

    let mut sql = String::from(
        "SELECT doc_id, project_id, type, ref_id, session_id, title, body, updated_at
         FROM search_docs
         WHERE (fts_match(title, ?1) OR fts_match(body, ?1))",
    );
    let mut params: Vec<Value> = vec![Value::Text(search.text.to_string())];
    if let Some(project_id) = search.project_id {
        params.push(Value::Text(project_id.to_string()));
        sql.push_str(&format!(" AND project_id = ?{}", params.len()));
    }
    if let Some(kind) = search.kind {
        params.push(Value::Text(kind.to_string()));
        sql.push_str(&format!(" AND type = ?{}", params.len()));
    }
    params.push(Value::Text(search.text.to_string()));
    let score = params.len();
    params.push(Value::Integer(limit));
    let lim = params.len();
    sql.push_str(&format!(
        " ORDER BY (COALESCE(fts_score(title, ?{score}), 0) + COALESCE(fts_score(body, ?{score}), 0)) DESC, updated_at DESC LIMIT ?{lim}"
    ));

    let conn = db.connect().map_err(engine)?;
    let mut rows = conn.query(&sql, params).await.map_err(engine)?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        hits.push(hit_from_row(&row)?);
    }
    Ok(hits)
}

fn hit_from_row(row: &Row) -> Result<SearchHit> {
    let body = text_at(row, 6)?.unwrap_or_default();
    let title = text_at(row, 5)?;
    Ok(SearchHit {
        doc_id: required(row, 0)?,
        project_id: required(row, 1)?,
        kind: required(row, 2)?,
        ref_id: required(row, 3)?,
        session_id: text_at(row, 4)?,
        snippet: snippet(title.as_deref(), &body),
        title,
        updated_at: required(row, 7)?,
    })
}

/// A short plain snippet: the body's opening, or the title when there is no body.
fn snippet(title: Option<&str>, body: &str) -> String {
    let source = if body.trim().is_empty() {
        title.unwrap_or("")
    } else {
        body
    };
    let trimmed = source.trim();
    if trimmed.chars().count() <= 200 {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(200).collect();
    out.push('…');
    out
}

fn validate_kind(kind: &str) -> Result<()> {
    match kind {
        "feed" | "artifact" | "brain" => Ok(()),
        other => Err(Error::InvalidArgument(format!(
            "unknown search type '{other}'"
        ))),
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}

fn required(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("search row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in a search column, found {other:?}"
        ))),
    }
}
