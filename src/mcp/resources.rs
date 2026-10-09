//! The project knowledge base as MCP resources.
//!
//! A read-only view over the store the brain tools already read: a listing
//! walks the knowledge bases the caller may read, and a read is `brain_get` on
//! the project store addressed by URI. Nothing here is stored, and the access
//! check is the one the project store's tools make.

use std::borrow::Cow;

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use rmcp::model::{ReadResourceResult, Resource, ResourceContents, ResourceTemplate};

use super::HubServer;
use crate::brain::{self, Brain, Entry, EntryKind, knowledge};
use crate::error::{Error, Result};
use crate::policy::Access;
use crate::principal::Principal;
use crate::store::projects;

/// Every knowledge base page URI starts with this.
const KB_PREFIX: &str = "agenthub://kb/";

/// The RFC 6570 template a client fills to name a page it already knows.
const KB_TEMPLATE: &str = "agenthub://kb/{project_id}/{+path}";

/// What a path segment percent-encodes, so a page name with a space, a `#` or a
/// `?` still makes one URI that reads back to the same page.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// The URI of one page, from its project and canonical `/fs/` path.
fn kb_uri(project_id: &str, path: &str) -> String {
    let relative = path.strip_prefix("/fs/").unwrap_or(path);
    let segments: Vec<String> = relative
        .split('/')
        .map(|segment| utf8_percent_encode(segment, SEGMENT).to_string())
        .collect();
    format!(
        "{KB_PREFIX}{}/{}",
        utf8_percent_encode(project_id, SEGMENT),
        segments.join("/")
    )
}

/// The project and canonical page path a knowledge base URI names.
///
/// `None` is a URI outside the knowledge base, which the caller answers as an
/// unknown resource. A URI inside it that names no page is refused.
pub(super) fn parse_kb_uri(uri: &str) -> Option<Result<(String, String)>> {
    let rest = uri.strip_prefix(KB_PREFIX)?;
    let malformed = || {
        Error::InvalidArgument(format!(
            "'{uri}' names no knowledge base page; the form is {KB_TEMPLATE}"
        ))
    };
    let parsed = (|| {
        let (project, path) = rest.split_once('/').ok_or_else(malformed)?;
        let project = decode(project)?;
        let path = path
            .split('/')
            .map(decode)
            .collect::<Result<Vec<_>>>()?
            .join("/");
        if project.is_empty() {
            return Err(malformed());
        }
        let path = knowledge::page_path(&format!("/fs/{path}"))?;
        if path == "/fs" {
            return Err(malformed());
        }
        Ok((project, path))
    })();
    Some(parsed)
}

fn decode(segment: &str) -> Result<String> {
    percent_decode_str(segment)
        .decode_utf8()
        .map(Cow::into_owned)
        .map_err(|_| {
            Error::InvalidArgument(format!("'{segment}' does not percent-decode to UTF-8 text"))
        })
}

/// The media type a page is served as, from its name.
fn media_type(path: &str) -> &'static str {
    if path.ends_with(".md") {
        "text/markdown"
    } else {
        "text/plain"
    }
}

/// What names the children of one `/fs` directory, for a walk.
trait Children {
    async fn children(&mut self, dir: &str) -> Result<Vec<Entry>>;
}

impl Children for &Brain {
    async fn children(&mut self, dir: &str) -> Result<Vec<Entry>> {
        self.list_names(dir).await
    }
}

/// Up to `limit` pages under `/fs` that sort after `after`, in path order.
///
/// `tree` names a directory's children. They are visited in name order, a
/// directory sorting as its path with a trailing `/`, so pages come out in
/// path order and the walk stops at `limit`. A child that sorts wholly at or
/// before `after`, a page or a directory whose every path does, is dropped
/// before the sort, so such a directory is never listed.
async fn pages_after(
    mut tree: impl Children,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, i64)>> {
    let behind = |key: &str| {
        after.is_some_and(|after| match key.strip_suffix('/') {
            // Every path under a directory sorts before `after` when its
            // prefix does and `after` is not inside it.
            Some(_) => key < after && !after.starts_with(key),
            None => key <= after,
        })
    };
    let mut found = Vec::new();
    // What is left to visit, the next one last: a page carries its size, a
    // directory none.
    let mut stack: Vec<(String, Option<i64>)> = vec![("/fs".to_string(), None)];
    while found.len() < limit
        && let Some((path, size)) = stack.pop()
    {
        if let Some(size) = size {
            found.push((path, size));
            continue;
        }
        let mut children: Vec<(String, (String, Option<i64>))> = tree
            .children(&path)
            .await?
            .into_iter()
            .filter_map(|entry| match entry.kind {
                EntryKind::Dir => Some((format!("{}/", entry.path), (entry.path, None))),
                EntryKind::File => Some((entry.path.clone(), (entry.path, Some(entry.size_bytes)))),
                EntryKind::Key => None,
            })
            .filter(|(key, _)| !behind(key))
            .collect();
        children.sort_unstable_by(|(a, _), (b, _)| b.cmp(a));
        stack.extend(children.into_iter().map(|(_, child)| child));
    }
    Ok(found)
}

/// The pages of one project after `after`, at most `limit` of them.
async fn project_pages(
    server: &HubServer,
    project_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, i64)>> {
    match server
        .state
        .knowledge
        .open_existing(project_id, brain::KNOWLEDGE_FILE)
        .await?
    {
        Some(kb) => pages_after(&kb, after, limit).await,
        None => Ok(Vec::new()),
    }
}

/// What a listing takes from one project's walk.
///
/// A store the engine finds locked fails the listing, which is retryable, so
/// the client asks again with the same cursor and misses nothing. Any other
/// failure, a store removed while it was walked or one that cannot be opened,
/// leaves the project out, so one broken store never blocks the listing of
/// every other.
fn kept(project_id: &str, pages: Result<Vec<(String, i64)>>) -> Result<Vec<(String, i64)>> {
    match pages {
        Err(err) if !err.is_locked() => {
            tracing::warn!(project = %project_id, error = %err, "knowledge base left out of a resource listing");
            Ok(Vec::new())
        }
        pages => pages,
    }
}

/// The template the hub lists for knowledge base pages.
pub(super) fn kb_template() -> ResourceTemplate {
    ResourceTemplate::new(KB_TEMPLATE, "kb-page")
        .with_title("Knowledge base page")
        .with_description(
            "A page of a project knowledge base, by project id and its path under /fs/, \
             each path segment percent-encoded so a reserved character stays in its segment.",
        )
}

/// The pages of one listing.
pub(super) struct KbListing {
    pub resources: Vec<Resource>,
    /// Whether pages remain past the last one listed.
    pub more: bool,
}

impl HubServer {
    /// Up to `room` knowledge base pages the caller may read, after `after`.
    ///
    /// The order is project id, then page path, so the URI of the last page
    /// listed is enough to resume from: it is the cursor.
    pub(super) async fn kb_resources(
        &self,
        principal: &Principal,
        after: Option<&(String, String)>,
        room: usize,
    ) -> Result<KbListing> {
        let visible = projects::visible_names(&self.state.db, principal).await?;
        let mut resources = Vec::new();
        for (project_id, display_name) in visible {
            let from = match after {
                Some((id, _)) if project_id < *id => continue,
                Some((id, path)) if project_id == *id => Some(path.as_str()),
                _ => None,
            };
            // One more than fits, to learn whether pages remain.
            let want = room + 1 - resources.len();
            let pages = kept(
                &project_id,
                project_pages(self, &project_id, from, want).await,
            )?;
            for (path, size) in pages {
                let relative = path.strip_prefix("/fs/").unwrap_or(&path);
                resources.push(
                    Resource::new(kb_uri(&project_id, &path), relative)
                        .with_title(format!("{display_name}: {relative}"))
                        .with_mime_type(media_type(&path))
                        .with_size(size.max(0) as u64),
                );
            }
            if resources.len() > room {
                resources.truncate(room);
                return Ok(KbListing {
                    resources,
                    more: true,
                });
            }
        }
        Ok(KbListing {
            resources,
            more: false,
        })
    }

    /// Read one page, held to the read check `brain_get` makes on the
    /// project store.
    pub(super) async fn kb_read(
        &self,
        principal: &Principal,
        uri: &str,
        project_id: &str,
        path: &str,
    ) -> Result<ReadResourceResult> {
        let project_id = self
            .knowledge_project(principal, Some(project_id), Access::Read)
            .await?;
        let absent = || knowledge::no_page(path);
        let kb = self
            .state
            .knowledge
            .open_existing(&project_id, brain::KNOWLEDGE_FILE)
            .await?
            .ok_or_else(absent)?;
        let bytes = kb.get(path).await?.ok_or_else(absent)?;
        let text = String::from_utf8(bytes).map_err(|_| {
            Error::InvalidArgument(format!("the page at '{path}' is not UTF-8 text"))
        })?;
        let contents = ResourceContents::text(text, uri)
            .with_mime_type(format!("{}; charset=utf-8", media_type(path)));
        Ok(ReadResourceResult::new(vec![contents]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_uri_reads_back_to_the_page_it_was_made_from() {
        for path in ["/fs/a.md", "/fs/svc/caddy.md", "/fs/notes/a b#c?.md"] {
            let uri = kb_uri("homelab", path);
            let (project, parsed) = parse_kb_uri(&uri)
                .expect("a knowledge base URI")
                .expect("a page");
            assert_eq!((project.as_str(), parsed.as_str()), ("homelab", path));
        }
        assert_eq!(
            kb_uri("homelab", "/fs/notes/a b.md"),
            "agenthub://kb/homelab/notes/a%20b.md"
        );
    }

    /// An in-memory tree for `pages_after`, counting the directories listed.
    struct Tree {
        dirs: Vec<&'static str>,
        pages: Vec<&'static str>,
        listed: Vec<String>,
    }

    impl Tree {
        fn new(dirs: &[&'static str], pages: &[&'static str]) -> Self {
            Self {
                dirs: dirs.to_vec(),
                pages: pages.to_vec(),
                listed: Vec::new(),
            }
        }

        async fn walk(&mut self, after: Option<&str>, limit: usize) -> Vec<String> {
            pages_after(&mut *self, after, limit)
                .await
                .expect("an in-memory walk")
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        }
    }

    fn directly_under(dir: &str, path: &str) -> bool {
        path.strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| !rest.contains('/'))
    }

    impl Children for &mut Tree {
        async fn children(&mut self, dir: &str) -> Result<Vec<Entry>> {
            self.listed.push(dir.to_string());
            let dirs = self
                .dirs
                .iter()
                .filter(|path| directly_under(dir, path))
                .map(|path| Entry {
                    path: path.to_string(),
                    kind: EntryKind::Dir,
                    size_bytes: 0,
                });
            let pages = self
                .pages
                .iter()
                .filter(|path| directly_under(dir, path))
                .map(|path| Entry {
                    path: path.to_string(),
                    kind: EntryKind::File,
                    size_bytes: 1,
                });
            // Reversed, so the walk's own order is what is tested.
            let mut children: Vec<Entry> = dirs.chain(pages).collect();
            children.reverse();
            Ok(children)
        }
    }

    fn tree() -> Tree {
        Tree::new(
            &["/fs/a", "/fs/a/deep", "/fs/notes", "/fs/zz"],
            &[
                "/fs/a/deep/x.md",
                "/fs/a/y.md",
                "/fs/notes.md",
                "/fs/notes/1.md",
                "/fs/notes/2.md",
                "/fs/zz/last.md",
            ],
        )
    }

    #[tokio::test]
    async fn the_walk_lists_pages_in_path_order_and_stops_at_the_limit() {
        let mut every = tree();
        let all = every.walk(None, usize::MAX).await;
        let mut sorted = all.clone();
        sorted.sort();
        assert_eq!(all, sorted, "pages come out in path order");
        assert_eq!(all.len(), 6);

        let mut first = tree();
        assert_eq!(first.walk(None, 2).await, ["/fs/a/deep/x.md", "/fs/a/y.md"]);
        assert!(
            !first.listed.iter().any(|dir| dir == "/fs/zz"),
            "a full walk stops before the directories it does not need: {:?}",
            first.listed
        );
    }

    #[tokio::test]
    async fn the_walk_does_not_list_a_directory_wholly_before_the_cursor() {
        let mut resumed = tree();
        assert_eq!(
            resumed.walk(Some("/fs/notes/1.md"), 10).await,
            ["/fs/notes/2.md", "/fs/zz/last.md"]
        );
        assert_eq!(
            resumed.listed,
            ["/fs", "/fs/notes", "/fs/zz"],
            "the directories before the cursor are not listed"
        );
    }

    #[test]
    fn a_locked_store_fails_the_listing_and_any_other_failure_leaves_the_project_out() {
        let locked = kept("homelab", Err(Error::Engine("database is locked".into())));
        assert!(locked.is_err_and(|err| err.retryable()));
        for failure in [
            Error::Conflict("removed".into()),
            Error::Engine("file is not a database".into()),
        ] {
            let left_out = kept("homelab", Err(failure));
            assert!(left_out.is_ok_and(|pages| pages.is_empty()));
        }
    }

    #[test]
    fn a_uri_outside_the_knowledge_base_is_not_parsed() {
        assert!(parse_kb_uri("agenthub://skill").is_none());
        for uri in [
            "agenthub://kb/homelab",
            "agenthub://kb/homelab/",
            "agenthub://kb//a.md",
            "agenthub://kb/homelab/%FF.md",
        ] {
            assert!(
                parse_kb_uri(uri).expect("inside the namespace").is_err(),
                "{uri} names no page"
            );
        }
        let (_, climbed) = parse_kb_uri("agenthub://kb/homelab/../../kv/x")
            .expect("inside the namespace")
            .expect("a page");
        assert_eq!(climbed, "/fs/kv/x", "a path never leaves /fs");
    }
}
