//! The project knowledge base: one write path for every surface.
//!
//! The human writes over REST and an agent writes over the MCP tools. Both
//! land here, so a page gets the same path rules, the same size limit, the
//! same log row, the same search row and the same feed signal whoever wrote
//! it, and the numbers derived from the tree go stale for neither.
//!
//! A surface authenticates its caller and decides whether the caller may
//! write. What it hands over is the actor it authenticated, never a name the
//! caller supplied.

use serde_json::json;

use super::{Brain, KNOWLEDGE_FILE, Stamp, canonical_path};
use crate::app::AppState;
use crate::error::{Error, Result};
use crate::okf::{LintFinding, extract_links, lint_page_write, parse_frontmatter};
use crate::store::events::{self, NewEvent};
use crate::store::projects;
use crate::store::search::{SearchDoc, index_doc};

/// The actor every write through the admin gate is recorded under, and the
/// `verified.by` a review stamps. The hub has one human and no user table, so
/// it is one fixed string.
pub const HUMAN: &str = "human";

/// What a write reports back.
#[derive(Debug, Clone)]
pub struct Written {
    /// The canonical path the page is stored, logged and indexed under.
    pub path: String,
    pub version: String,
    pub size_bytes: usize,
    pub lint: Vec<LintFinding>,
    pub warnings: Vec<String>,
}

/// The canonical path of a knowledge base page, from the path a caller gave.
///
/// Every reader and writer of the knowledge base keys on what this returns:
/// the store, the write log, the search corpus and the link graph. `.`, `..`,
/// an empty component and a trailing slash resolve the way the filesystem
/// resolves them, so several spellings name one page and none of them becomes
/// a second key. What cannot name a page is refused: a key-value path, a
/// control character, a backslash, and a path over the length limit.
pub fn page_path(path: &str) -> Result<String> {
    if path == "/kv" || path.starts_with("/kv/") {
        return Err(Error::InvalidArgument(format!(
            "the project knowledge base holds pages only, so '{path}' has no meaning there; use an /fs/ path"
        )));
    }
    if let Some(found) = path.chars().find(|c| c.is_control() || *c == '\\') {
        return Err(Error::InvalidArgument(format!(
            "a knowledge base path may not contain {found:?}"
        )));
    }
    let canonical = canonical_path(path)?;
    crate::limits::check_kb_path(&canonical)?;
    Ok(canonical)
}

/// Open a project's knowledge base to write to it, creating it on first use.
///
/// Deleting a project removes the file under the lock this open takes, so the
/// project is looked up again once the lock is held: a write that lost that
/// race must not bring the file back for a project with no row, where no
/// report counts it and nothing ever removes it.
pub async fn open_for_write(state: &AppState, project_id: &str) -> Result<Brain> {
    let db = state.db.clone();
    let looked_up = project_id.to_string();
    state
        .knowledge
        .open_live(
            project_id,
            KNOWLEDGE_FILE,
            async move || match projects::get(&db, &looked_up).await? {
                Some(_) => Ok(()),
                None => Err(Error::NotFound(format!("project {looked_up} not found"))),
            },
        )
        .await
}

/// The refusal for a page that is not there, the same for every operation.
pub fn no_page(path: &str) -> Error {
    Error::NotFound(format!("no knowledge base page at '{path}'"))
}

/// Write one page.
pub async fn put(
    state: &AppState,
    project_id: &str,
    actor: &str,
    path: &str,
    content: &str,
    if_version: Option<&str>,
) -> Result<Written> {
    let path = page_path(path)?;
    let brain = open_for_write(state, project_id).await?;
    store(
        state, &brain, project_id, "kb.put", actor, path, content, if_version,
    )
    .await
}

/// Delete one page, reporting the canonical path that went.
///
/// A page that is not there is the same refusal a read of it gives: nothing is
/// logged, nothing is signalled, and no knowledge base file is created to find
/// that out. `session_id` ties the feed signal to the session an agent is
/// working in.
pub async fn delete(
    state: &AppState,
    project_id: &str,
    actor: &str,
    session_id: Option<&str>,
    path: &str,
    if_version: Option<&str>,
) -> Result<String> {
    let path = page_path(path)?;
    let Some(brain) = state
        .knowledge
        .open_existing(project_id, KNOWLEDGE_FILE)
        .await?
    else {
        return Err(no_page(&path));
    };
    let stamp = Stamp {
        op: "kb.delete",
        actor,
    };
    let deleted = brain
        .delete_if_recorded(&path, if_version, stamp, async || {
            unindex(state, project_id, &path).await;
        })
        .await?;
    if !deleted {
        return Err(no_page(&path));
    }
    state.notify();
    signal(
        state,
        project_id,
        actor,
        session_id,
        format!("Knowledge base page {path} deleted"),
        json!({"action": "kb_deleted", "store": "project", "path": path}),
    )
    .await;
    Ok(path)
}

/// Store a page under a canonical path, with its log row and its search row,
/// then tell every reader of a derived number that the tree changed.
#[allow(clippy::too_many_arguments)]
async fn store(
    state: &AppState,
    brain: &Brain,
    project_id: &str,
    op: &str,
    actor: &str,
    path: String,
    content: &str,
    if_version: Option<&str>,
) -> Result<Written> {
    crate::limits::check_brain_file(brain.file_bytes())?;
    let version = brain
        .put_if_recorded(
            &path,
            content.as_bytes(),
            if_version,
            Stamp { op, actor },
            async || index(state, project_id, &path, content).await,
        )
        .await?;
    state.notify();

    let mut warnings = Vec::new();
    if brain.file_bytes() > crate::limits::BRAIN_FILE_BYTES_SOFT {
        warnings.push("brain file is over the soft limit".to_string());
    }
    let lint = lint_on_write(brain, &path, content).await;
    Ok(Written {
        path,
        version,
        size_bytes: content.len(),
        lint,
        warnings,
    })
}

/// The advisory findings for the page just written.
///
/// A link is checked against the tree itself, target by target, so a page in
/// any directory counts as there. Lint never fails a write: a target that
/// cannot be looked up is left out of the known set and reads as broken.
async fn lint_on_write(brain: &Brain, path: &str, content: &str) -> Vec<LintFinding> {
    let mut known = Vec::new();
    for link in extract_links(path, content) {
        if let Some(target) = link.resolved_path
            && !known.contains(&target)
            && brain.is_file(&target).await.unwrap_or(false)
        {
            known.push(target);
        }
    }
    lint_page_write(path, content, &known)
}

/// Mirror a page into the search corpus.
///
/// The page is already stored and logged when this runs, so a failure here is
/// logged and the write still succeeds: reporting a failure for a write that
/// happened would send the caller to retry it.
async fn index(state: &AppState, project_id: &str, path: &str, body: &str) {
    let parsed = parse_frontmatter(body).ok().flatten();
    let title = parsed
        .as_ref()
        .and_then(|frontmatter| frontmatter.title.as_deref())
        .unwrap_or(path);
    let mut end = body.len().min(crate::limits::SEARCH_BODY_BYTES_MAX);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let doc_id = doc_id(project_id, path);
    let updated_at = crate::store::now_rfc3339();
    let indexed = async {
        let conn = crate::store::connect(&state.db)?;
        index_doc(
            &conn,
            SearchDoc {
                doc_id: &doc_id,
                project_id,
                kind: "kb",
                ref_id: path,
                session_id: None,
                title: Some(title),
                body: &body[..end],
                updated_at: &updated_at,
            },
        )
        .await
    };
    if let Err(err) = indexed.await {
        tracing::warn!(project_id, path, error = %err, "could not index a knowledge base page");
    }
}

/// Remove a page's search row. A failure is logged for the same reason a
/// failed index write is.
async fn unindex(state: &AppState, project_id: &str, path: &str) {
    let removed = async {
        let conn = crate::store::connect(&state.db)?;
        conn.execute(
            "DELETE FROM search_docs WHERE doc_id = ?1",
            vec![turso::Value::Text(doc_id(project_id, path))],
        )
        .await
        .map_err(crate::store::engine)
    };
    if let Err(err) = removed.await {
        tracing::warn!(project_id, path, error = %err, "could not remove a knowledge base search row");
    }
}

/// The search document id of a page, built from its canonical path.
fn doc_id(project_id: &str, path: &str) -> String {
    format!("kb:{project_id}:{path}")
}

/// Append a lifecycle signal to the project feed.
///
/// The page operation already happened, so a feed that cannot take the event
/// is logged and does not fail it.
async fn signal(
    state: &AppState,
    project_id: &str,
    actor: &str,
    session_id: Option<&str>,
    summary: String,
    payload: serde_json::Value,
) {
    let event = NewEvent {
        project_id: project_id.to_string(),
        kind: "signal".to_string(),
        summary,
        payload: Some(payload),
        needs_action: false,
        thread_id: None,
        session_id: session_id.map(str::to_string),
    };
    match events::append(&state.db, actor, None, event).await {
        Ok(_) => state.notify(),
        Err(err) => {
            tracing::warn!(project_id, error = %err, "could not append a knowledge base signal");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_path_is_canonical_whatever_spelling_named_it() {
        for spelling in [
            "/fs/a/b.md",
            "/fs/a//b.md",
            "/fs/a/./b.md",
            "/fs/x/../a/b.md",
            "/fs/a/b.md/",
            "/fs/../../a/b.md",
        ] {
            assert_eq!(page_path(spelling).expect(spelling), "/fs/a/b.md");
        }
    }

    #[test]
    fn a_path_that_cannot_name_a_page_is_refused() {
        let long = format!("/fs/{}", "a".repeat(crate::limits::KB_PATH_BYTES_MAX));
        for hostile in [
            "/kv",
            "/kv/plan",
            "/fs/nul\u{0}.md",
            "/fs/line\nbreak.md",
            "/fs/cr\r.md",
            "/fs/tab\t.md",
            "/fs/del\u{7f}.md",
            "/fs/back\\slash.md",
            "fs/relative.md",
            "/etc/passwd",
            long.as_str(),
        ] {
            assert!(
                matches!(page_path(hostile), Err(Error::InvalidArgument(_))),
                "{hostile:?} is refused"
            );
        }
    }
}
