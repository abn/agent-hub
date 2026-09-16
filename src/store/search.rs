//! The search corpus: the write-through seam every writer calls, and the query
//! path over it.
//!
//! Ranking uses the engine's full-text index, which only produces a real score
//! for a query shaped the way its index method recognises: a single
//! `fts_match` over the indexed columns with `fts_score` ordering. Project and
//! type filters are applied after the ranked fetch, so the ranking stays live.

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
pub struct SearchQuery {
    /// The text to match against titles and bodies.
    pub text: String,
    /// Restrict to one project.
    pub project_id: Option<String>,
    /// Restrict to one corpus family: `feed`, `artifact`, or `brain`.
    pub kind: Option<String>,
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

/// Query the corpus, ranked by text relevance, then filtered by project and
/// corpus family.
///
/// The ranked fetch uses the index method's recognised shape; filters and the
/// final limit are applied afterwards. More candidates are fetched than
/// returned so a filter does not starve the page.
pub async fn query(db: &Database, search: &SearchQuery) -> Result<Vec<SearchHit>> {
    query_visible(db, search, None).await
}

/// Query the corpus with an optional project confinement.
///
/// `None` means every project (the admin surface), fetched with a cap. `Some`
/// confines the result, and the ranked query is left uncapped so a confined
/// caller is not starved by higher-ranked projects it cannot see. The engine's
/// full-text score only survives the query's exact shape, so the confinement
/// is applied while reading the ranked rows rather than as a SQL predicate.
pub async fn query_visible(
    db: &Database,
    search: &SearchQuery,
    visible: Option<&[String]>,
) -> Result<Vec<SearchHit>> {
    if search.text.trim().is_empty() {
        return Err(Error::InvalidArgument(
            "a search query is required".to_string(),
        ));
    }
    if let Some(kind) = search.kind.as_deref() {
        validate_kind(kind)?;
    }
    if let Some(visible) = visible
        && visible.is_empty()
    {
        return Ok(Vec::new());
    }
    let limit = search.limit.clamp(1, crate::limits::FEED_LIMIT_MAX);

    let mut sql = String::from(
        "SELECT * FROM search_docs WHERE fts_match(title, body, ?1)
         ORDER BY fts_score(title, body, ?1) DESC",
    );
    let mut params = vec![Value::Text(search.text.clone())];
    if visible.is_none() {
        let fetch = (limit.saturating_mul(20)).clamp(limit, crate::limits::FEED_LIMIT_MAX);
        params.push(Value::Integer(fetch));
        sql.push_str(&format!(" LIMIT ?{}", params.len()));
    }

    let conn = db.connect().map_err(crate::store::engine)?;
    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(crate::store::engine)?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
        let hit = hit_from_row(&row)?;
        if let Some(project_id) = &search.project_id
            && &hit.project_id != project_id
        {
            continue;
        }
        if let Some(kind) = &search.kind
            && &hit.kind != kind
        {
            continue;
        }
        if let Some(visible) = visible
            && !visible.iter().any(|id| id == &hit.project_id)
        {
            continue;
        }
        hits.push(hit);
        if hits.len() as i64 == limit {
            break;
        }
    }
    Ok(hits)
}

/// A group of hits sharing one corpus family.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchGroup {
    pub kind: String,
    pub hits: Vec<SearchHit>,
}

/// Group hits by corpus family, preserving the ranked order inside each group
/// and ordering the groups by their best hit.
pub fn group(hits: Vec<SearchHit>) -> Vec<SearchGroup> {
    let mut groups: Vec<SearchGroup> = Vec::new();
    for hit in hits {
        match groups.iter_mut().find(|group| group.kind == hit.kind) {
            Some(group) => group.hits.push(hit),
            None => groups.push(SearchGroup {
                kind: hit.kind.clone(),
                hits: vec![hit],
            }),
        }
    }
    groups
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

fn required(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("search row is missing a column".to_string()))
}

fn text_at(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(crate::store::engine)? {
        Value::Text(text) => Ok(Some(text)),
        Value::Null => Ok(None),
        other => Err(Error::Engine(format!(
            "expected text in a search column, found {other:?}"
        ))),
    }
}
