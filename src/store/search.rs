//! The search corpus: the write-through seam every writer calls, and the query
//! path over it.
//!
//! Ranking uses the engine's full-text index. Its index method only pushes the
//! ordering and the page down for a query shaped exactly the way it
//! recognises: a single `fts_match` over the indexed columns with `fts_score`
//! ordering. Add any other predicate and the ordering is silently dropped
//! while `fts_score` stays readable per row, so a filtered query asks the
//! engine to match and filter, reads the scores, and ranks here.

use turso::{Connection, Database, Row, Value};

use crate::error::{Error, Result};

/// A document to index.
pub struct SearchDoc<'a> {
    /// Stable id, for example `event:<ulid>`.
    pub doc_id: &'a str,
    /// Owning project.
    pub project_id: &'a str,
    /// Corpus family: `feed`, `artifact`, `brain`, or `kb`.
    pub kind: &'a str,
    /// Id of the underlying row.
    pub ref_id: &'a str,
    /// Session this belongs to, when it is session brain content. A project
    /// knowledge base page belongs to no session.
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
    /// Restrict to one corpus family: `feed`, `artifact`, `brain`, or `kb`.
    pub kind: Option<String>,
    /// Restrict to the content of one session's brain.
    pub session_id: Option<String>,
    /// Maximum hits to return.
    pub limit: i64,
}

/// One search hit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub doc_id: String,
    pub project_id: String,
    /// The name the projects list shows, absent when no project row carries
    /// the id.
    pub project_display_name: Option<String>,
    pub kind: String,
    pub ref_id: String,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub snippet: String,
    pub updated_at: String,
    /// For a feed hit, the kind of the event and who wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// For an artifact hit, its current version and that version's size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    /// For a session brain hit, the session's name and its status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_status: Option<String>,
}

/// The corpus columns a hit is built from, in the order `hit_from_row` reads.
const COLUMNS: &str = "doc_id, project_id, type, ref_id, session_id, title, body, updated_at";

/// Query the corpus, ranked by text relevance, scoped by project and corpus
/// family.
pub async fn query(db: &Database, search: &SearchQuery) -> Result<Vec<SearchHit>> {
    query_visible(db, search, None).await
}

/// Query the corpus with an optional project confinement.
///
/// `None` means every project (the admin surface); `Some` confines the result
/// to those projects. The confinement joins the project and family scopes as
/// one more SQL predicate, so a scope never starves the page: the engine
/// matches and filters, and the hits are ranked here from the scores it
/// returns. A scope that matches more than `SEARCH_FETCH_MAX` documents is
/// ranked over that many, which bounds the scan.
pub async fn query_visible(
    db: &Database,
    search: &SearchQuery,
    visible: Option<&[String]>,
) -> Result<Vec<SearchHit>> {
    let limit = search.limit.clamp(1, crate::limits::SEARCH_LIMIT_MAX);
    query_limited(db, search, visible, limit).await
}

/// The query path with the page size already resolved.
///
/// [`search`] asks for one row past the page it will serve, which is how it
/// tells a full page from a cut one. That probe is the store's own business,
/// so it is not a limit a caller can ask for.
async fn query_limited(
    db: &Database,
    search: &SearchQuery,
    visible: Option<&[String]>,
    limit: i64,
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

    let safe_query = sanitize_fts(&search.text);
    if safe_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut params = vec![Value::Text(safe_query)];
    let mut scopes = String::new();
    if let Some(project_id) = &search.project_id {
        params.push(Value::Text(project_id.clone()));
        scopes.push_str(&format!(" AND project_id = ?{}", params.len()));
    }
    if let Some(kind) = &search.kind {
        params.push(Value::Text(kind.clone()));
        scopes.push_str(&format!(" AND type = ?{}", params.len()));
    }
    if let Some(session_id) = &search.session_id {
        params.push(Value::Text(session_id.clone()));
        scopes.push_str(&format!(" AND session_id = ?{}", params.len()));
    }
    if let Some(visible) = visible {
        let holes: Vec<String> = visible
            .iter()
            .enumerate()
            .map(|(offset, _)| format!("?{}", params.len() + offset + 1))
            .collect();
        params.extend(visible.iter().map(|id| Value::Text(id.clone())));
        scopes.push_str(&format!(" AND project_id IN ({})", holes.join(", ")));
    }

    let sql = if scopes.is_empty() {
        params.push(Value::Integer(limit));
        format!(
            "SELECT {COLUMNS} FROM search_docs WHERE fts_match(title, body, ?1)
             ORDER BY fts_score(title, body, ?1) DESC LIMIT ?{}",
            params.len()
        )
    } else {
        // No ORDER BY or LIMIT here on purpose. The index method declines both
        // once the query carries a predicate it does not cover, and the
        // `fts_score` left in an ORDER BY then scores every row zero, which
        // reads as a ranked page and is not one. The score in the column list
        // is the one the index method still fills in.
        format!(
            "SELECT {COLUMNS}, fts_score(title, body, ?1) FROM search_docs
             WHERE fts_match(title, body, ?1){scopes}"
        )
    };

    let conn = super::connect(db)?;
    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(crate::store::engine)?;
    if scopes.is_empty() {
        // The engine ranked and paged this one.
        let mut hits = Vec::new();
        while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
            hits.push(hit_from_row(&row)?);
        }
        drop(rows);
        enrich(&conn, &mut hits).await?;
        return Ok(hits);
    }

    let mut scored = Vec::new();
    while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
        scored.push((score_at(&row, 8)?, hit_from_row(&row)?));
        if scored.len() as i64 == crate::limits::SEARCH_FETCH_MAX {
            break;
        }
    }
    scored.sort_by(|left, right| right.0.total_cmp(&left.0));
    scored.truncate(limit as usize);
    drop(rows);
    let mut hits: Vec<SearchHit> = scored.into_iter().map(|(_, hit)| hit).collect();
    enrich(&conn, &mut hits).await?;
    Ok(hits)
}

/// Fill in what a hit shows beyond its corpus row.
///
/// This runs after the page is ranked and cut, so it never touches the match
/// or the order, and it reads only by the ids of hits already confined to what
/// the caller may see. Each read is one query for the whole page.
async fn enrich(conn: &Connection, hits: &mut [SearchHit]) -> Result<()> {
    let ids: Vec<&str> = hits.iter().map(|hit| hit.project_id.as_str()).collect();
    let names = crate::store::projects::display_names(conn, &ids).await?;
    for hit in hits.iter_mut() {
        hit.project_display_name = names.get(&hit.project_id).cloned();
    }

    let events = related(
        conn,
        "SELECT id, project_id, kind, actor, summary, payload FROM events WHERE id IN",
        ids_of(hits, "feed", |hit| Some(hit.ref_id.as_str())),
    )
    .await?;
    let artifacts = related(
        conn,
        "SELECT id, project_id, current_ver, size_bytes FROM artifacts WHERE id IN",
        ids_of(hits, "artifact", |hit| Some(hit.ref_id.as_str())),
    )
    .await?;
    let sessions = related(
        conn,
        "SELECT id, project_id, session_name, status FROM sessions WHERE id IN",
        ids_of(hits, "brain", |hit| hit.session_id.as_deref()),
    )
    .await?;

    for hit in hits.iter_mut() {
        match hit.kind.as_str() {
            "feed" => {
                // The corpus row holds the serialized payload, which is what
                // makes every word of it searchable and is nothing to read.
                // What is shown is what the agent wrote: the payload's `body`
                // when it is a string, otherwise the summary.
                hit.snippet = snippet(hit.title.as_deref(), "");
                if let Some(row) = own(events.get(&hit.ref_id), &hit.project_id) {
                    hit.event_kind = row.text(0);
                    hit.actor = row.text(1);
                    let summary = row.text(2);
                    let body = row.text(3).and_then(|payload| written_body(&payload));
                    hit.snippet = snippet(summary.as_deref(), body.as_deref().unwrap_or(""));
                }
            }
            "artifact" => {
                if let Some(row) = own(artifacts.get(&hit.ref_id), &hit.project_id) {
                    hit.version = row.integer(0);
                    hit.size_bytes = row.integer(1);
                }
            }
            "brain" => {
                let session = hit.session_id.as_ref().and_then(|id| sessions.get(id));
                if let Some(row) = own(session, &hit.project_id) {
                    hit.session_name = row.text(0);
                    hit.session_status = row.text(1);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A corpus row speaks for its own project only: a row that names something
/// held by another project shows nothing of it.
fn own<'a>(found: Option<&'a Related>, project_id: &str) -> Option<&'a Related> {
    found.filter(|row| row.project_id == project_id)
}

/// The `body` of an event payload, when the payload is an object that holds
/// one as a string with something in it.
fn written_body(payload: &str) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(payload).ok()?;
    let body = payload.get("body")?.as_str()?;
    (!body.trim().is_empty()).then(|| body.to_string())
}

/// A row a hit points at: the project that holds it, and the columns the hit
/// shows, in the order the query named them.
struct Related {
    project_id: String,
    columns: Vec<Value>,
}

impl Related {
    fn text(&self, at: usize) -> Option<String> {
        match self.columns.get(at) {
            Some(Value::Text(text)) => Some(text.clone()),
            _ => None,
        }
    }

    fn integer(&self, at: usize) -> Option<i64> {
        match self.columns.get(at) {
            Some(Value::Integer(number)) => Some(*number),
            _ => None,
        }
    }
}

/// The ids one family of hits points at, without repeats.
fn ids_of<'a>(
    hits: &'a [SearchHit],
    kind: &str,
    id: impl Fn(&'a SearchHit) -> Option<&'a str>,
) -> Vec<&'a str> {
    let mut ids: Vec<&str> = hits
        .iter()
        .filter(|hit| hit.kind == kind)
        .filter_map(id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Read the rows a family of hits points at, in one query, keyed by id.
///
/// `select` names the id and the holding project first, then what the hit
/// shows, and ends at `IN`. No ids means no query.
async fn related(
    conn: &Connection,
    select: &str,
    ids: Vec<&str>,
) -> Result<std::collections::HashMap<String, Related>> {
    let mut found = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(found);
    }
    let holes: Vec<String> = (1..=ids.len()).map(|at| format!("?{at}")).collect();
    let mut rows = conn
        .query(
            &format!("{select} ({})", holes.join(", ")),
            ids.iter()
                .map(|id| Value::Text(id.to_string()))
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(crate::store::engine)?;
    while let Some(row) = rows.next().await.map_err(crate::store::engine)? {
        let mut columns = Vec::new();
        for at in 2..row.column_count() {
            columns.push(row.get_value(at).map_err(crate::store::engine)?);
        }
        found.insert(
            required(&row, 0)?,
            Related {
                project_id: required(&row, 1)?,
                columns,
            },
        );
    }
    Ok(found)
}

/// A group of hits sharing one corpus family.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchGroup {
    pub kind: String,
    /// Hits in this group, so a group header counts without walking the list.
    pub count: usize,
    pub hits: Vec<SearchHit>,
}

/// A ranked page with the two numbers the results line shows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResults {
    /// Hits on this page, before grouping. It is what came back, not how many
    /// documents match: `truncated` says when those differ.
    pub count: usize,
    /// Whether the limit cut the result, so a surface never prints a capped
    /// page as a total.
    pub truncated: bool,
    /// How long the query itself took, floored at zero.
    pub took_ms: u64,
    pub groups: Vec<SearchGroup>,
}

/// Run a query and report what it found and how long it took.
///
/// The clock spans the store call and nothing else: the results line says how
/// fast the index is, not how fast the process serialised JSON.
///
/// One row beyond the limit is asked for and thrown away, which is what tells
/// a full page from a cut one without a second count over the same predicate.
pub async fn search(
    db: &Database,
    query: &SearchQuery,
    visible: Option<&[String]>,
) -> Result<SearchResults> {
    let limit = query.limit.clamp(1, crate::limits::SEARCH_LIMIT_MAX);
    let started = std::time::Instant::now();
    let mut hits = query_limited(db, query, visible, limit.saturating_add(1)).await?;
    let took_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let truncated = hits.len() as i64 > limit;
    hits.truncate(limit as usize);
    Ok(SearchResults {
        count: hits.len(),
        truncated,
        took_ms,
        groups: group(hits),
    })
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
                count: 0,
                hits: vec![hit],
            }),
        }
    }
    for group in &mut groups {
        group.count = group.hits.len();
    }
    groups
}

fn hit_from_row(row: &Row) -> Result<SearchHit> {
    let body = text_at(row, 6)?.unwrap_or_default();
    let title = text_at(row, 5)?;
    Ok(SearchHit {
        doc_id: required(row, 0)?,
        project_id: required(row, 1)?,
        project_display_name: None,
        kind: required(row, 2)?,
        ref_id: required(row, 3)?,
        session_id: text_at(row, 4)?,
        snippet: snippet(title.as_deref(), &body),
        title,
        updated_at: required(row, 7)?,
        event_kind: None,
        actor: None,
        version: None,
        size_bytes: None,
        session_name: None,
        session_status: None,
    })
}

/// A short plain snippet: the body's opening, or the title when there is no
/// body. A feed hit is given its own in [`enrich`].
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
        "feed" | "artifact" | "brain" | "kb" => Ok(()),
        other => Err(Error::InvalidArgument(format!(
            "unknown search type '{other}'"
        ))),
    }
}

fn required(row: &Row, index: usize) -> Result<String> {
    text_at(row, index)?.ok_or_else(|| Error::Engine("search row is missing a column".to_string()))
}

fn score_at(row: &Row, index: usize) -> Result<f64> {
    match row.get_value(index).map_err(crate::store::engine)? {
        Value::Real(score) => Ok(score),
        Value::Integer(score) => Ok(score as f64),
        other => Err(Error::Engine(format!(
            "expected a relevance score, found {other:?}"
        ))),
    }
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

/// Turn arbitrary user input into a safe FTS query.
///
/// Balanced double-quoted phrases are preserved as phrase queries. Unbalanced
/// quotes and FTS syntax characters (parentheses, colons, asterisks, booleans)
/// are stripped, and bare terms are quoted as string literals so no input can
/// trigger an FTS parse error. If the input contains no searchable terms, an
/// empty string is returned.
pub fn sanitize_fts(input: &str) -> String {
    // A word is kept only if it holds a letter or a digit: a run of nothing but
    // `-` or `_` is not something the index can look up, and the engine refuses
    // a query made only of those. The bare operators are dropped rather than
    // searched for, since a reader who types `engine AND state` is not looking
    // for the word "and".
    fn is_word(ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_' || ch == '-'
    }
    fn searchable(term: &str) -> bool {
        term.chars().any(char::is_alphanumeric)
    }
    fn operator(term: &str) -> bool {
        matches!(term, "AND" | "OR" | "NOT" | "NEAR")
    }

    let mut terms: Vec<String> = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch == '"' {
            chars.next();
            let mut words = Vec::new();
            let mut word = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '"' {
                    closed = true;
                    break;
                }
                if is_word(inner) {
                    word.push(inner);
                } else if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            if !word.is_empty() {
                words.push(word);
            }
            words.retain(|w| searchable(w));
            if words.is_empty() {
                continue;
            }
            if closed {
                // A balanced phrase is honoured as one.
                terms.push(format!("\"{}\"", words.join(" ")));
            } else {
                terms.extend(words.into_iter().map(|w| format!("\"{w}\"")));
            }
        } else if is_word(ch) {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if !is_word(c) {
                    break;
                }
                word.push(c);
                chars.next();
            }
            if searchable(&word) && !operator(&word) {
                terms.push(format!("\"{word}\""));
            }
        } else {
            chars.next();
        }
    }

    // Quoting costs two bytes and a separator per term, so a long query grows
    // on its way through here. The engine refuses a query past its own limit;
    // the terms that fit are searched and the rest are left out, which a query
    // of that length will not miss.
    let mut safe = String::new();
    for (count, term) in terms.iter().enumerate() {
        if count == crate::limits::SEARCH_TERMS_MAX
            || safe.len() + term.len() + 1 > crate::limits::SEARCH_QUERY_BYTES_MAX
        {
            break;
        }
        if !safe.is_empty() {
            safe.push(' ');
        }
        safe.push_str(term);
    }
    safe
}
