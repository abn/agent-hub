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
use crate::okf::frontmatter::{PromoteParams, promote_frontmatter, review_frontmatter};
use crate::okf::{LintFinding, extract_links, lint_page_write, parse_frontmatter};
use crate::store::events::{self, NewEvent};
use crate::store::projects;
use crate::store::search::{SearchDoc, index_doc};
use crate::store::sessions::Session;

/// The actor every write through the admin gate is recorded under, and the
/// `verified.by` a review stamps. The hub has one human and no user table, so
/// it is one fixed string.
pub const HUMAN: &str = "human";

/// How long before its write lands a verification may have been stamped and
/// still count as made for that write. A busy node puts seconds between the
/// two; minutes is generous, and anything older was made for other bytes.
const VERIFIES_WINDOW_SECS: i64 = 5 * 60;

/// How far ahead of this node's clock a stamp may be, for a writer whose clock
/// runs a little fast. A stamp further in the future verifies nothing.
const VERIFIES_SKEW_SECS: i64 = 60;

/// The operation a review is logged under. The human surface reads it back to
/// tell a review's own write from an edit made after it.
pub const REVIEW_OP: &str = "kb.review";

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

/// Refuse a frontmatter value that could end its own line.
///
/// The frontmatter patcher is line oriented, so a value carrying a line break
/// would write keys of its own choosing into the page. The patcher escapes
/// what it emits; this is the same refusal one layer earlier, so neither has
/// to be right alone.
pub fn check_field(name: &str, value: &str) -> Result<()> {
    match value.chars().find(|c| c.is_control()) {
        Some(found) => Err(Error::InvalidArgument(format!(
            "{name} may not contain {found:?}"
        ))),
        None => Ok(()),
    }
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
    crate::limits::check_kb_page(content.len())?;
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
        verifies: None,
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

/// Record the human's review of one page.
///
/// Only the `verified` block changes. `if_version` is the version the human
/// read: when the page moved on since, the review is refused, because what
/// was read is not what would be stamped. Without it the review applies to
/// whatever is stored when it lands.
pub async fn review(
    state: &AppState,
    project_id: &str,
    path: &str,
    if_version: Option<&str>,
) -> Result<Written> {
    let path = page_path(path)?;
    let Some(brain) = state
        .knowledge
        .open_existing(project_id, KNOWLEDGE_FILE)
        .await?
    else {
        return Err(no_page(&path));
    };
    let bytes = brain.get(&path).await?.ok_or_else(|| no_page(&path))?;
    let at = crate::store::now_rfc3339();
    stamp_review(state, &brain, project_id, path, bytes, if_version, &at).await
}

/// Stamp the human's review onto the bytes that were read.
///
/// Apart from the read, this is the whole review. It is its own function
/// because the promise it keeps is about the gap between that read and the
/// write: a test hands it bytes that have since been replaced, which is the
/// one way to stand in that gap without waiting for a race to open it. The
/// time of the review is the caller's for the same reason: the stamp is taken
/// before the write lands, and a test can say how long before.
pub async fn stamp_review(
    state: &AppState,
    brain: &super::Brain,
    project_id: &str,
    path: String,
    bytes: Vec<u8>,
    if_version: Option<&str>,
    at: &str,
) -> Result<Written> {
    let read_version = super::version(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::InvalidArgument(format!("the page at '{path}' is not UTF-8 text")))?;
    // A page whose frontmatter cannot be patched safely is refused, not guessed at.
    let reviewed = review_frontmatter(&text, HUMAN, at)?;
    crate::limits::check_kb_page(reviewed.len())?;

    // One comparison, made under the write lock: against the version the
    // human read when there is one, and otherwise against the version this
    // call patched, so a write that lands in between is never overwritten.
    let expected = if_version.unwrap_or(&read_version);
    let written = store(
        state,
        brain,
        project_id,
        REVIEW_OP,
        HUMAN,
        path,
        &reviewed,
        Some(expected),
    )
    .await?;
    signal(
        state,
        project_id,
        HUMAN,
        None,
        format!("Reviewed knowledge base page {}", written.path),
        json!({
            "action": "kb_reviewed",
            "store": "project",
            "path": written.path,
            "verified_by": HUMAN,
        }),
    )
    .await;
    Ok(written)
}

/// What a promotion copies, and what it adds to the page on the way.
#[derive(Debug, Clone, Copy)]
pub struct Promotion<'a> {
    /// The session whose brain holds the source entry.
    pub session: &'a Session,
    pub from_path: &'a str,
    pub to_path: &'a str,
    pub page_type: Option<&'a str>,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub tags: Option<&'a [String]>,
    pub if_version: Option<&'a str>,
}

/// Copy a session brain entry into the knowledge base as a page that cites
/// where it came from. The source is left as it was.
pub async fn promote(
    state: &AppState,
    project_id: &str,
    actor: &str,
    promotion: &Promotion<'_>,
) -> Result<Written> {
    let to_path = page_path(promotion.to_path)?;
    let from_path = canonical_path(promotion.from_path)?;
    // The source path and the session name both land in the citation.
    check_field("from_path", &from_path)?;
    for (name, value) in [
        ("type", promotion.page_type),
        ("title", promotion.title),
        ("description", promotion.description),
    ] {
        if let Some(value) = value {
            check_field(name, value)?;
        }
    }
    for tag in promotion.tags.unwrap_or_default() {
        check_field("a tag", tag)?;
    }
    let session = promotion.session;
    let session_name: String = session
        .session_name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    let absent = || {
        Error::NotFound(format!(
            "no brain value at '{from_path}' in session {}",
            session.id
        ))
    };
    let source = state
        .brain
        .open_existing(&session.project_id, &session.id)
        .await?
        .ok_or_else(absent)?
        .get(&from_path)
        .await?
        .ok_or_else(absent)?;
    let source = String::from_utf8(source).map_err(|_| {
        Error::InvalidArgument(format!("the value at '{from_path}' is not UTF-8 text"))
    })?;

    let page = promote_frontmatter(
        &source,
        &PromoteParams {
            page_type: promotion.page_type,
            title: promotion.title,
            description: promotion.description,
            tags: promotion.tags,
            session_name: &session_name,
            session_id: &session.id,
            from_path: &from_path,
        },
    )?;
    crate::limits::check_kb_page(page.len())?;

    let brain = open_for_write(state, project_id).await?;
    let written = store(
        state,
        &brain,
        project_id,
        "kb.promote",
        actor,
        to_path,
        &page,
        promotion.if_version,
    )
    .await?;
    signal(
        state,
        project_id,
        actor,
        Some(&session.id),
        format!("Promoted {from_path} to {}", written.path),
        json!({
            "action": "kb_promoted",
            "store": "project",
            "from_session_id": session.id,
            "from_path": from_path,
            "to_path": written.path,
        }),
    )
    .await;
    Ok(written)
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
            Stamp {
                op,
                actor,
                verifies: Some(brings_in_newest_verification),
            },
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

/// Whether a write is the one that brings in a page's newest verification:
/// the page it stores ends its `verified` block with an entry that the page it
/// replaces did not hold anywhere in its own.
///
/// This is what `edited_since_review` rests on. The page is edited since its
/// review when the bytes it holds are not the bytes the newest such write
/// stored, so no clock is asked: a page that arrives with a verification of
/// its own is not an edit since it whatever second it lands in, a retry that
/// stores the same bytes is no edit, and an edit in the same second as the
/// review is one. Taking the newest entry away to show an older one brings
/// nothing in, and neither does an entry the page already held.
fn brings_in_newest_verification(before: Option<&[u8]>, after: &[u8]) -> bool {
    let verified = |bytes: &[u8]| {
        std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| parse_frontmatter(text).ok().flatten())
            .map(|frontmatter| frontmatter.verified)
            .unwrap_or_default()
    };
    let Some(newest) = verified(after).last().cloned() else {
        return false;
    };
    // A verification is about the bytes its author saw, so it is brought in
    // only by the write it was made for: one landing about when the entry says
    // it was made. An entry that is genuine but old, taken out and put back
    // over an edited body or copied onto a page its author never opened, was
    // made for other bytes and verifies nothing here.
    let made =
        time::OffsetDateTime::parse(&newest.at, &time::format_description::well_known::Rfc3339)
            .map(|at| at.unix_timestamp());
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let fresh = made
        .is_ok_and(|made| (now - VERIFIES_WINDOW_SECS..=now + VERIFIES_SKEW_SECS).contains(&made));
    fresh && !before.map(verified).unwrap_or_default().contains(&newest)
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
    use crate::config::Config;
    use crate::store::events::{FeedQuery, read_feed};

    /// A directory under the build tree, removed when the test ends. Unit
    /// tests get no CARGO_TARGET_TMPDIR, so the path is built from the
    /// manifest directory.
    struct DataDir(std::path::PathBuf);

    impl Drop for DataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn state(tag: &str) -> (AppState, DataDir) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/tmp")
            .join(format!("agent-hub-{tag}-{}-{nanos}", std::process::id()));
        let state = AppState::open(Config {
            data_dir: dir.clone(),
            bind: "127.0.0.1:0".parse().expect("socket address"),
            public_url: None,
            admin_token: Some("token".to_string()),
            inbox_caps: crate::limits::InboxCaps::disabled(),
            active_window: std::time::Duration::from_secs(900),
            node_name: None,
        })
        .await
        .expect("open state");
        (state, DataDir(dir))
    }

    /// The page write nudges the stream before its feed event exists, so a
    /// listener that refetched on that nudge has not seen the event. The
    /// signal sends a nudge of its own once the event is stored, and none for
    /// an event the feed refused.
    #[tokio::test]
    async fn a_signal_nudges_the_stream_once_its_event_is_in_the_feed() {
        let (state, _dir) = state("kb-signal").await;
        let mut ticks = state.ticker.subscribe();
        let stored = async || {
            read_feed(&state.db, "proj", &FeedQuery::default())
                .await
                .expect("read feed")
                .events
                .len()
        };

        let payload = json!({"action": "kb_written", "store": "project", "path": "/fs/a.md"});
        let summary = "Knowledge base page /fs/a.md written".to_string();
        signal(&state, "proj", "human", None, summary, payload.clone()).await;
        assert_eq!(stored().await, 1);
        assert!(
            ticks.try_recv().is_ok(),
            "the stored event is followed by a nudge"
        );
        assert!(ticks.try_recv().is_err(), "and by one only");

        let too_long = "x".repeat(crate::limits::EVENT_SUMMARY_CHARS_MAX + 1);
        signal(&state, "proj", "human", None, too_long, payload).await;
        assert_eq!(stored().await, 1, "the feed refused the second event");
        assert!(
            ticks.try_recv().is_err(),
            "a signal that stored nothing nudges no one"
        );
    }

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

    #[test]
    fn a_field_with_a_control_character_is_refused() {
        assert!(check_field("title", "He said \"x\": y # fine").is_ok());
        for hostile in ["a\nb", "a\rb", "a\u{0}b", "a\u{85}b"] {
            assert!(check_field("title", hostile).is_err(), "{hostile:?}");
        }
    }
}
