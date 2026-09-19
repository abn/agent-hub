//! Knowledge base REST routes.
//!
//! The human's surface over the project knowledge base: pages, review,
//! promotion from a session brain, the write history, backlinks, lint and the
//! derived numbers. Every route sits behind the admin gate, which runs before
//! anything else is looked at, and every write goes through the one write
//! path in [`crate::brain::knowledge`] that the agent tools also use.

use std::collections::HashMap;

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::brain::knowledge::{self, HUMAN, Written};
use crate::brain::{self, Brain, EntryKind, WriteFilter, WriteRecord, is_under};
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery, json_body};
use crate::okf::lint::lint_bundle;
use crate::okf::parse_frontmatter;
use crate::store::projects;
use crate::store::sessions;
use crate::store::storage::KbCacheEntry;

/// Rows one history page returns when the caller names no limit.
const HISTORY_LIMIT_DEFAULT: usize = 50;

/// The most rows one history page returns. Zero is allowed and returns the
/// count alone.
const HISTORY_LIMIT_MAX: usize = 200;

/// The query parameters for a page listing.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagesQuery {
    pub prefix: Option<String>,
    pub limit: Option<usize>,
    pub meta: Option<String>,
}

/// The JSON body for putting a page.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagePutBody {
    pub content: String,
    pub if_version: Option<String>,
}

/// The query for a write or a delete, which guards one that has no JSON body
/// to carry the version in.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardQuery {
    pub if_version: Option<String>,
}

/// The body for reviewing a page. All of it is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBody {
    /// Who the review is recorded under. The server sets it, so the one value
    /// accepted is the one it would set.
    pub verified_by: Option<String>,
    /// The version the human read.
    pub if_version: Option<String>,
}

/// The body for promoting a session brain entry into the knowledge base.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromoteBody {
    pub from_session_id: String,
    pub from_path: String,
    pub to_path: String,
    #[serde(rename = "type")]
    pub page_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub if_version: Option<String>,
}

/// The query for the write history.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    pub path: Option<String>,
    pub actor: Option<String>,
    pub prefix: Option<String>,
    pub op: Option<String>,
    pub before: Option<i64>,
    pub limit: Option<usize>,
}

/// The query for backlinks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacklinksQuery {
    pub path: String,
}

/// The query for lint findings.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintQuery {
    pub fresh: Option<String>,
}

fn problem(err: Error) -> Problem {
    Problem::from_error(&err)
}

/// The admin gate, then the project. Nothing about the request is read
/// before the gate, so a caller without the token learns nothing from the
/// refusal, whatever it sent.
async fn check_access(
    state: &AppState,
    headers: &HeaderMap,
    project_id: &str,
) -> std::result::Result<(), Problem> {
    state
        .auth
        .require_admin(bearer_token(headers).as_deref())
        .map_err(problem)?;
    match projects::get(&state.db, project_id)
        .await
        .map_err(problem)?
    {
        Some(_) => Ok(()),
        None => Err(problem(Error::NotFound(format!(
            "project {project_id} not found"
        )))),
    }
}

/// The namespaced path a REST caller named.
///
/// A URL names a page without the leading slash, and may leave the namespace
/// out: `svc/caddy.md` is `/fs/svc/caddy.md`. A first component of `fs` or
/// `kv` is the namespace itself, so `kv/x` is the key-value path the knowledge
/// base refuses and never a directory called `kv`. The result goes through
/// the same canonicalisation every other surface uses.
fn page_path(path: &str) -> std::result::Result<String, Problem> {
    let relative = path.trim_start_matches('/');
    let namespaced = match relative.split('/').next() {
        Some("fs" | "kv") => format!("/{relative}"),
        _ => format!("/fs/{relative}"),
    };
    knowledge::page_path(&namespaced).map_err(problem)
}

/// A boolean query flag: `1` or `true`, `0` or `false`, or absent.
fn flag(name: &str, value: Option<&str>) -> std::result::Result<bool, Problem> {
    match value {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(other) => Err(problem(Error::InvalidArgument(format!(
            "{name} is 1 or 0, not '{other}'"
        )))),
    }
}

/// Format unix seconds as RFC 3339.
fn format_unix_timestamp(epoch_secs: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(epoch_secs)
        .ok()
        .and_then(|dt| {
            dt.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| epoch_secs.to_string())
}

/// Current date formatted as YYYY-MM-DD.
fn current_date_str() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        now.month() as u8,
        now.day()
    )
}

/// The knowledge base of a project that has one. A read never creates it.
async fn existing(
    state: &AppState,
    project_id: &str,
) -> std::result::Result<Option<Brain>, Problem> {
    state
        .knowledge
        .open_existing(project_id, brain::KNOWLEDGE_FILE)
        .await
        .map_err(problem)
}

/// Cut a listing at its limit and say whether anything was cut.
fn page_of(mut entries: Vec<Value>, limit: Option<usize>) -> Value {
    let limit = limit
        .unwrap_or(crate::limits::BRAIN_LIST_ENTRIES_MAX)
        .min(crate::limits::BRAIN_LIST_ENTRIES_MAX);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    json!({ "entries": entries, "truncated": truncated })
}

/// `GET /api/v1/projects/{id}/kb/pages?prefix=&limit=&meta=`
///
/// Without `meta` this is one directory level. With it, it is every page and
/// directory under the prefix with the page facts a row shows, so a tree, a
/// directory view and the needs-review queue each cost one request.
pub async fn pages(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<PagesQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let with_meta = flag("meta", query.meta.as_deref())?;
    let prefix = page_path(query.prefix.as_deref().unwrap_or("fs"))?;

    let Some(brain) = existing(&state, &project_id).await? else {
        return Ok(Json(page_of(Vec::new(), query.limit)));
    };

    if with_meta {
        let memo = get_or_compute_kb(&state, &project_id, &brain, false).await?;
        let entries = memo
            .meta_pages
            .into_iter()
            .filter(|entry| {
                entry["path"]
                    .as_str()
                    .is_some_and(|path| path != prefix && is_under(&prefix, path))
            })
            .collect();
        return Ok(Json(page_of(entries, query.limit)));
    }

    let mut entries = Vec::new();
    for entry in brain.list(&prefix).await.map_err(problem)? {
        let children = match entry.kind {
            EntryKind::Dir => Some(brain.list(&entry.path).await.map_err(problem)?.len()),
            _ => None,
        };
        let mut row = json!(entry);
        row["children"] = json!(children);
        entries.push(row);
    }
    Ok(Json(page_of(entries, query.limit)))
}

/// `GET /api/v1/projects/{id}/kb/pages/{*path}`
pub async fn page_get(
    State(state): State<AppState>,
    Path((project_id, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let path = page_path(&path)?;

    let absent = || problem(knowledge::no_page(&path));
    let brain = existing(&state, &project_id).await?.ok_or_else(absent)?;
    let bytes = brain
        .get(&path)
        .await
        .map_err(problem)?
        .ok_or_else(absent)?;
    let version = brain::version(&bytes);
    let content = String::from_utf8(bytes).map_err(|_| {
        problem(Error::InvalidArgument(format!(
            "the page at '{path}' is not UTF-8 text"
        )))
    })?;
    let last_write = brain.last_write(&path).await.map_err(problem)?;

    Ok(Json(json!({
        "path": path,
        "content": content,
        "version": version,
        "size_bytes": content.len(),
        "last_write": {
            "actor": last_write.as_ref().map(|write| write.actor.as_str()),
            "at": last_write.as_ref().map(|write| format_unix_timestamp(write.at)),
        },
    })))
}

/// Read a request body up to a limit, refusing a longer one as a problem.
///
/// A declared length over the limit is refused before a byte is read, and a
/// body that does not declare one stops being buffered at the limit.
async fn read_body(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> std::result::Result<axum::body::Bytes, Problem> {
    let too_large = || {
        problem(Error::PayloadTooLarge(format!(
            "the request body is over the limit of {limit} bytes"
        )))
    };
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return Err(too_large());
    }
    axum::body::to_bytes(body, limit)
        .await
        .map_err(|_| too_large())
}

/// The media type of a request, without its parameters.
fn media_type(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let essence = value.split(';').next()?.trim();
    Some(essence.to_ascii_lowercase())
}

fn write_result(written: &Written) -> Json<Value> {
    Json(json!({
        "ok": true,
        "path": written.path,
        "version": written.version,
        "size_bytes": written.size_bytes,
        "lint": written.lint,
        "warnings": written.warnings,
    }))
}

/// `PUT /api/v1/projects/{id}/kb/pages/{*path}`
///
/// The body is `{content, if_version?}` as JSON, or the page itself as
/// `text/markdown` or `text/plain` with the guard in `?if_version=`. A body
/// that says it is JSON and does not parse is refused: it is never taken for
/// the page.
pub async fn page_put(
    State(state): State<AppState>,
    Path((project_id, path)): Path<(String, String)>,
    ProblemQuery(query): ProblemQuery<GuardQuery>,
    headers: HeaderMap,
    body: Body,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let path = page_path(&path)?;

    let (content, if_version) = match media_type(&headers).as_deref() {
        Some("application/json") => {
            let bytes = read_body(&headers, body, crate::limits::REQUEST_BODY_BYTES_MAX).await?;
            let parsed: PagePutBody = serde_json::from_slice(&bytes)
                .map_err(|err| problem(Error::InvalidArgument(format!("the page body: {err}"))))?;
            if parsed.if_version.is_some() && query.if_version.is_some() {
                return Err(problem(Error::InvalidArgument(
                    "if_version is given once, in the body or in the query".to_string(),
                )));
            }
            (parsed.content, parsed.if_version.or(query.if_version))
        }
        Some("text/markdown" | "text/plain") => {
            let bytes = read_body(&headers, body, crate::limits::KB_PAGE_BYTES_MAX).await?;
            let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
                problem(Error::InvalidArgument(
                    "the page body is not valid UTF-8".to_string(),
                ))
            })?;
            (text, query.if_version)
        }
        other => {
            return Err(Problem::with_status(
                &Error::InvalidArgument(format!(
                    "a page is sent as application/json, text/markdown or text/plain, not '{}'",
                    other.unwrap_or_default()
                )),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ));
        }
    };

    let written = knowledge::put(
        &state,
        &project_id,
        HUMAN,
        &path,
        &content,
        if_version.as_deref(),
    )
    .await
    .map_err(problem)?;
    Ok(write_result(&written))
}

/// `DELETE /api/v1/projects/{id}/kb/pages/{*path}?if_version=`
pub async fn page_delete(
    State(state): State<AppState>,
    Path((project_id, path)): Path<(String, String)>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<GuardQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let path = page_path(&path)?;
    let path = knowledge::delete(
        &state,
        &project_id,
        HUMAN,
        None,
        &path,
        query.if_version.as_deref(),
    )
    .await
    .map_err(problem)?;
    Ok(Json(json!({ "ok": true, "path": path })))
}

/// `POST /api/v1/projects/{id}/kb/pages/{*path}/review`
///
/// The wildcard takes the whole tail, so the one POST under a page is matched
/// here by its last segment.
pub async fn page_post(
    State(state): State<AppState>,
    Path((project_id, full_path)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let Some(page) = full_path.strip_suffix("/review") else {
        return Err(problem(Error::NotFound(
            "no route matches this path".to_string(),
        )));
    };
    let path = page_path(page)?;

    let bytes = read_body(&headers, body, crate::limits::REQUEST_BODY_BYTES_MAX).await?;
    let review: ReviewBody = if bytes.is_empty() {
        ReviewBody::default()
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|err| problem(Error::InvalidArgument(format!("the review body: {err}"))))?
    };
    // The reviewer is whoever holds the admin token, which is the human. The
    // field is accepted only as an echo of that, so a caller cannot put a
    // review in anybody else's name.
    if review.verified_by.as_deref().is_some_and(|by| by != HUMAN) {
        return Err(problem(Error::InvalidArgument(format!(
            "verified_by is set by the hub to '{HUMAN}'; leave it out"
        ))));
    }

    let written = knowledge::review(&state, &project_id, &path, review.if_version.as_deref())
        .await
        .map_err(problem)?;
    Ok(Json(json!({
        "ok": true,
        "path": written.path,
        "version": written.version,
    })))
}

/// `POST /api/v1/projects/{id}/kb/promote`
pub async fn promote(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<PromoteBody>, JsonRejection>,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let body = json_body(body, "promote body")?;
    let to_path = page_path(&body.to_path)?;

    let session = sessions::get(&state.db, &body.from_session_id)
        .await
        .map_err(problem)?
        .filter(|session| session.deleted_at.is_none())
        .ok_or_else(|| {
            problem(Error::NotFound(format!(
                "session {} not found",
                body.from_session_id
            )))
        })?;

    let written = knowledge::promote(
        &state,
        &project_id,
        HUMAN,
        &knowledge::Promotion {
            session: &session,
            from_path: &body.from_path,
            to_path: &to_path,
            page_type: body.page_type.as_deref(),
            title: body.title.as_deref(),
            description: body.description.as_deref(),
            tags: body.tags.as_deref(),
            if_version: body.if_version.as_deref(),
        },
    )
    .await
    .map_err(problem)?;
    Ok(Json(json!({
        "ok": true,
        "path": written.path,
        "version": written.version,
        "lint": written.lint,
    })))
}

/// `GET /api/v1/projects/{id}/kb/history?path=&actor=&prefix=&op=&before=&limit=`
///
/// The whole write log is scanned for every request, so `total` is the real
/// count however long the log is, and a page that was cut says so.
pub async fn history(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<HistoryQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let limit = query.limit.unwrap_or(HISTORY_LIMIT_DEFAULT);
    if limit > HISTORY_LIMIT_MAX {
        return Err(problem(Error::InvalidArgument(format!(
            "limit is at most {HISTORY_LIMIT_MAX}"
        ))));
    }
    let path = query.path.as_deref().map(page_path).transpose()?;
    let prefix = query.prefix.as_deref().map(page_path).transpose()?;

    let log = match existing(&state, &project_id).await? {
        Some(brain) => brain
            .write_log(
                &WriteFilter {
                    path: path.as_deref(),
                    prefix: prefix.as_deref(),
                    actor: query.actor.as_deref(),
                    op: query.op.as_deref(),
                },
                query.before,
                limit,
            )
            .await
            .map_err(problem)?,
        None => brain::WriteLogPage::default(),
    };

    let rows: Vec<Value> = log.rows.iter().map(history_row).collect();
    Ok(Json(json!({
        "rows": rows,
        "total": log.total,
        "next_before": log.next_before,
        "truncated": log.truncated,
    })))
}

fn history_row(record: &WriteRecord) -> Value {
    json!({
        "op": record.op,
        "path": record.path,
        "actor": record.actor,
        "at": format_unix_timestamp(record.at),
        "version": record.version,
    })
}

/// `GET /api/v1/projects/{id}/kb/backlinks?path=`
pub async fn backlinks(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<BacklinksQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let path = page_path(&query.path)?;

    let Some(brain) = existing(&state, &project_id).await? else {
        return Ok(Json(json!([])));
    };
    let memo = get_or_compute_kb(&state, &project_id, &brain, false).await?;
    // A page that links to itself is not something else referring to it, and
    // the order is the path's so it does not move between walks.
    let mut referring = memo.backlink_graph.backlinks_for(&path);
    referring.retain(|entry| entry.path != path);
    referring.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(json!(referring)))
}

/// `GET /api/v1/projects/{id}/kb/lint?fresh=`
pub async fn lint(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<LintQuery>,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;
    let fresh = flag("fresh", query.fresh.as_deref())?;

    let Some(brain) = existing(&state, &project_id).await? else {
        return Ok(Json(json!({
            "checked_at": crate::store::now_rfc3339(),
            "findings": [],
        })));
    };
    let memo = get_or_compute_kb(&state, &project_id, &brain, fresh).await?;
    Ok(Json(json!({
        "checked_at": memo.checked_at,
        "findings": memo.lint_findings,
    })))
}

/// `GET /api/v1/projects/{id}/kb/stats`
pub async fn stats(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, Problem> {
    check_access(&state, &headers, &project_id).await?;

    let Some(brain) = existing(&state, &project_id).await? else {
        return Ok(Json(json!({
            "pages": 0,
            "bytes": 0,
            "unverified": 0,
            "stale": 0,
            "last_change_at": null,
            "last_change_by": null,
            "needs_review": {
                "unverified": 0,
                "machine_confirmed": 0,
                "edited_since_review": 0,
                "total": 0,
            }
        })));
    };
    let memo = get_or_compute_kb(&state, &project_id, &brain, false).await?;
    Ok(Json(memo.stats))
}

/// What one walk of the tree finds: every page's text, and every directory
/// with the number of entries directly inside it and the bytes of its files.
struct Tree {
    pages: HashMap<String, String>,
    directories: Vec<(String, usize, i64)>,
}

/// Walk everything under `/fs`.
async fn walk(brain: &Brain) -> std::result::Result<Tree, Problem> {
    let mut tree = Tree {
        pages: HashMap::new(),
        directories: Vec::new(),
    };
    let mut stack = vec!["/fs".to_string()];
    while let Some(dir) = stack.pop() {
        let entries = brain.list(&dir).await.map_err(problem)?;
        if dir != "/fs" {
            let bytes = entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::File)
                .map(|entry| entry.size_bytes)
                .sum();
            tree.directories.push((dir.clone(), entries.len(), bytes));
        }
        for entry in entries {
            match entry.kind {
                EntryKind::Dir => stack.push(entry.path),
                EntryKind::File => {
                    if let Some(bytes) = brain.get(&entry.path).await.map_err(problem)?
                        && let Ok(text) = String::from_utf8(bytes)
                    {
                        tree.pages.insert(entry.path, text);
                    }
                }
                EntryKind::Key => {}
            }
        }
    }
    Ok(tree)
}

/// The derived facts of a knowledge base, from the memo when nothing was
/// written since they were computed.
///
/// Every write through either surface bumps the generation, and the
/// generation is read before the walk, so a write that lands during a walk
/// leaves an entry no later read will serve.
async fn get_or_compute_kb(
    state: &AppState,
    project_id: &str,
    brain: &Brain,
    fresh: bool,
) -> std::result::Result<KbCacheEntry, Problem> {
    let current_gen = state.generation();
    if !fresh && let Some(cached) = state.stats.get_kb(project_id, current_gen) {
        return Ok(cached);
    }

    let tree = walk(brain).await?;
    let (findings, backlink_graph) = lint_bundle(&tree.pages);
    let (last_writes, last_change) = brain.last_writes().await.map_err(problem)?;

    let today = current_date_str();
    let mut page_meta: HashMap<&str, Value> = HashMap::new();
    let mut unverified_count = 0;
    let mut machine_count = 0;
    let mut edited_since_review_count = 0;
    let mut stale_count = 0;
    let mut total_bytes: i64 = 0;

    for (path, content) in &tree.pages {
        let size_bytes = content.len() as i64;
        total_bytes += size_bytes;

        let fm = parse_frontmatter(content)
            .ok()
            .flatten()
            .unwrap_or_default();
        let is_stale = fm
            .stale_after
            .as_ref()
            .is_some_and(|d| d.as_str() < today.as_str());
        if is_stale {
            stale_count += 1;
        }

        let verified_by = fm.verified.last().map(|v| v.by.clone());
        let verified_at = fm.verified.last().map(|v| v.at.clone());
        let last_write = last_writes.get(path);

        let reviewed_tier = |machine_count: &mut i32| {
            if verified_by.as_deref() == Some(HUMAN) {
                "human_reviewed"
            } else {
                *machine_count += 1;
                "machine_confirmed"
            }
        };
        let trust = if fm.verified.is_empty() {
            unverified_count += 1;
            "unverified"
        } else {
            let verified_ts = verified_at
                .as_ref()
                .and_then(|at| {
                    time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .map(|dt| dt.unix_timestamp());
            match (last_write, verified_ts) {
                (Some(write), Some(verified)) if write.at > verified => {
                    edited_since_review_count += 1;
                    "edited_since_review"
                }
                _ => reviewed_tier(&mut machine_count),
            }
        };

        page_meta.insert(
            path.as_str(),
            json!({
                "path": path,
                "type": "file",
                "size_bytes": size_bytes,
                "children": null,
                "title": fm.title,
                "description": fm.description,
                "page_type": fm.page_type,
                "status": fm.status,
                "tags": fm.tags,
                "trust": trust,
                "verified_by": verified_by,
                "verified_at": verified_at,
                "stale": is_stale,
                "stale_after": fm.stale_after,
                "last_write_by": last_write.map(|write| write.actor.as_str()),
                "last_write_at": last_write.map(|write| format_unix_timestamp(write.at)),
            }),
        );
    }

    // A finding that names a page carries that page's row, so a lint list
    // shows who wrote the page and when without a second request.
    let lint_findings: Vec<Value> = findings
        .iter()
        .map(|finding| {
            let mut row = json!(finding);
            if let Some(meta) = page_meta.get(finding.path.as_str()) {
                row["meta"] = meta.clone();
            }
            row
        })
        .collect();

    let mut meta_pages: Vec<Value> = page_meta.into_values().collect();
    meta_pages.extend(tree.directories.iter().map(|(path, children, bytes)| {
        json!({
            "path": path,
            "type": "dir",
            "size_bytes": bytes,
            "children": children,
        })
    }));
    meta_pages.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));

    let needs_review_total = unverified_count + machine_count + edited_since_review_count;
    let stats = json!({
        "pages": tree.pages.len(),
        "bytes": total_bytes,
        "unverified": unverified_count,
        "stale": stale_count,
        "last_change_at": last_change.as_ref().map(|write| format_unix_timestamp(write.at)),
        "last_change_by": last_change.as_ref().map(|write| write.actor.as_str()),
        "needs_review": {
            "unverified": unverified_count,
            "machine_confirmed": machine_count,
            "edited_since_review": edited_since_review_count,
            "total": needs_review_total,
        }
    });

    let entry = KbCacheEntry {
        generation: current_gen,
        at: std::time::Instant::now(),
        checked_at: crate::store::now_rfc3339(),
        backlink_graph,
        lint_findings,
        stats,
        meta_pages,
    };

    state.stats.set_kb(project_id, entry.clone());
    Ok(entry)
}
