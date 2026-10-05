//! Advisory lint rules and backlink graph construction for OKF knowledge bases.
//!
//! Codes mirror `.agents/scripts/check-okf.py`:
//! - `missing_frontmatter`
//! - `missing_type`
//! - `unparsed_frontmatter`
//! - `okf_version_misplaced`
//! - `missing_index`
//! - `missing_log`
//! - `broken_link`
//! - `link_escapes_bundle`
//! - `orphan_page`
//! - `missing_index_entry`

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::frontmatter::parse_frontmatter;
use super::links::{ExtractedLink, extract_links};

/// One advisory lint finding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LintFinding {
    /// Machine-readable finding code.
    pub code: String,
    /// Path of the document the finding applies to.
    pub path: String,
    /// Human-readable advisory explanation.
    pub message: String,
    /// 1-based line number for link findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// One backlink reference entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BacklinkEntry {
    /// Canonical path of the referring page.
    pub path: String,
    /// Frontmatter title of the referring page, or its path when title is absent.
    pub title: String,
}

/// A graph of backlinks across a knowledge base.
#[derive(Debug, Clone, Default)]
pub struct BacklinkGraph {
    /// Map from target page path to referring pages.
    referring: HashMap<String, Vec<BacklinkEntry>>,
}

impl BacklinkGraph {
    /// Get backlinks referencing `target_path`.
    pub fn backlinks_for(&self, target_path: &str) -> Vec<BacklinkEntry> {
        self.referring.get(target_path).cloned().unwrap_or_default()
    }
}

/// Whether a page says what it is.
///
/// The bundle root, a section index and the log are located by where they sit
/// rather than by what they say about themselves, so they are the three pages a
/// concept page is everything else to.
fn is_concept(path: &str) -> bool {
    path != "/fs/index.md" && path != "/fs/log.md" && !path.ends_with("/index.md")
}

/// The finding for a page that carries no frontmatter block at all, or
/// `None` for a page that is asked for none.
///
/// A write is lenient, so a page with no block is stored as it was sent. That
/// leniency is the discoverability problem this carries the answer to: the
/// finding is the only place a writer is told the shape the knowledge base
/// reads, so it names the minimum to add rather than only what is absent.
fn missing_frontmatter(path: &str) -> Option<LintFinding> {
    let (subject, hint) = if path == "/fs/index.md" {
        (
            "root index.md",
            "the minimum is a block opening with --- that sets okf_version: 0.2",
        )
    } else if is_concept(path) {
        (
            "concept page",
            "the minimum is a block opening with --- that sets type: Concept",
        )
    } else {
        return None;
    };
    Some(LintFinding {
        code: "missing_frontmatter".to_string(),
        path: path.to_string(),
        message: format!("{subject} has no YAML frontmatter; {hint}"),
        line: None,
    })
}

/// Check lint rules on a single page during write.
///
/// Evaluates frontmatter requirements and link target existence against the
/// current set of known page paths in the store.
pub fn lint_page_write(path: &str, content: &str, existing_pages: &[String]) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let is_root_index = path == "/fs/index.md";

    match parse_frontmatter(content) {
        Ok(None) => findings.extend(missing_frontmatter(path)),
        Err(msg) => {
            findings.push(LintFinding {
                code: "unparsed_frontmatter".to_string(),
                path: path.to_string(),
                message: format!("frontmatter contains unparseable syntax: {msg}"),
                line: None,
            });
        }
        Ok(Some(fm)) => {
            if is_concept(path) {
                let has_type = fm
                    .page_type
                    .as_deref()
                    .is_some_and(|t| !t.trim().is_empty());
                if !has_type {
                    findings.push(LintFinding {
                        code: "missing_type".to_string(),
                        path: path.to_string(),
                        message: "frontmatter is missing a non-empty type field".to_string(),
                        line: None,
                    });
                }
            }

            if !is_root_index && fm.okf_version.is_some() {
                findings.push(LintFinding {
                    code: "okf_version_misplaced".to_string(),
                    path: path.to_string(),
                    message: "only the bundle root may carry okf_version".to_string(),
                    line: None,
                });
            }
        }
    }

    // Check links on write
    let known_set: HashSet<&str> = existing_pages
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(path))
        .collect();
    let links = extract_links(path, content);

    for link in links {
        if link.escapes_bundle {
            findings.push(LintFinding {
                code: "link_escapes_bundle".to_string(),
                path: path.to_string(),
                message: format!("link escapes the bundle ({})", link.target),
                line: Some(link.line),
            });
        } else if let Some(target) = link.resolved_path.as_deref()
            && !known_set.contains(target)
        {
            findings.push(LintFinding {
                code: "broken_link".to_string(),
                path: path.to_string(),
                message: format!("link target does not exist ({})", link.target),
                line: Some(link.line),
            });
        }
    }

    findings
}

/// Perform a full walk of the bundle pages, calculating all 10 lint rules and building backlinks.
pub fn lint_bundle(pages: &HashMap<String, String>) -> (Vec<LintFinding>, BacklinkGraph) {
    let mut findings = Vec::new();
    let mut backlink_graph = BacklinkGraph::default();

    let all_paths: HashSet<&str> = pages.keys().map(String::as_str).collect();

    // 1. Root log.md check
    if !all_paths.contains("/fs/log.md") {
        findings.push(LintFinding {
            code: "missing_log".to_string(),
            path: "/fs/log.md".to_string(),
            message: "log.md is missing from the bundle root".to_string(),
            line: None,
        });
    }

    // Collect sections (directories with files)
    let mut sections: HashSet<String> = HashSet::new();
    for path in &all_paths {
        if let Some((dir, _)) = path.rsplit_once('/')
            && dir != "/fs"
            && dir.starts_with("/fs/")
        {
            sections.insert(dir.to_string());
        }
    }

    // 2. Section index.md check
    for section in &sections {
        let expected_index = format!("{section}/index.md");
        if !all_paths.contains(expected_index.as_str()) {
            findings.push(LintFinding {
                code: "missing_index".to_string(),
                path: expected_index,
                message: format!("section {section} has no index.md"),
                line: None,
            });
        }
    }

    // Parse frontmatters and extract links for each page
    let mut page_titles: HashMap<String, String> = HashMap::new();
    let mut page_links: HashMap<String, Vec<ExtractedLink>> = HashMap::new();

    for (path, content) in pages {
        let is_root_index = path == "/fs/index.md";

        let fm_res = parse_frontmatter(content);
        match &fm_res {
            Ok(None) => {
                findings.extend(missing_frontmatter(path));
                page_titles.insert(path.clone(), path.clone());
            }
            Err(msg) => {
                findings.push(LintFinding {
                    code: "unparsed_frontmatter".to_string(),
                    path: path.clone(),
                    message: format!("frontmatter contains unparseable syntax: {msg}"),
                    line: None,
                });
                page_titles.insert(path.clone(), path.clone());
            }
            Ok(Some(fm)) => {
                let title = fm
                    .title
                    .as_deref()
                    .filter(|t| !t.is_empty())
                    .unwrap_or(path);
                page_titles.insert(path.clone(), title.to_string());

                if is_concept(path) {
                    let has_type = fm.page_type.as_ref().is_some_and(|t| !t.trim().is_empty());
                    if !has_type {
                        findings.push(LintFinding {
                            code: "missing_type".to_string(),
                            path: path.clone(),
                            message: "frontmatter is missing a non-empty type field".to_string(),
                            line: None,
                        });
                    }
                }

                if !is_root_index && fm.okf_version.is_some() {
                    findings.push(LintFinding {
                        code: "okf_version_misplaced".to_string(),
                        path: path.clone(),
                        message: "only the bundle root may carry okf_version".to_string(),
                        line: None,
                    });
                }
            }
        }

        let links = extract_links(path, content);
        page_links.insert(path.clone(), links);
    }

    // Build incoming links map and check links
    let mut incoming_count: HashMap<String, usize> = HashMap::new();

    for (src_path, links) in &page_links {
        let src_title = page_titles
            .get(src_path)
            .cloned()
            .unwrap_or_else(|| src_path.clone());

        for link in links {
            if link.escapes_bundle {
                findings.push(LintFinding {
                    code: "link_escapes_bundle".to_string(),
                    path: src_path.clone(),
                    message: format!("link escapes the bundle ({})", link.target),
                    line: Some(link.line),
                });
            } else if link.is_internal
                && let Some(target) = &link.resolved_path
            {
                if !all_paths.contains(target.as_str()) {
                    findings.push(LintFinding {
                        code: "broken_link".to_string(),
                        path: src_path.clone(),
                        message: format!("link target does not exist ({})", link.target),
                        line: Some(link.line),
                    });
                } else if target != src_path {
                    // Record backlink. A page that links to itself is not
                    // something else referring to it.
                    *incoming_count.entry(target.clone()).or_insert(0) += 1;
                    let entry = BacklinkEntry {
                        path: src_path.clone(),
                        title: src_title.clone(),
                    };
                    let list = backlink_graph.referring.entry(target.clone()).or_default();
                    if !list.iter().any(|e| e.path == *src_path) {
                        list.push(entry);
                    }
                }
            }
        }
    }

    // 3. Orphan page check (concept pages with no incoming links)
    for path in &all_paths {
        if is_concept(path) {
            let count = incoming_count.get(*path).copied().unwrap_or(0);
            if count == 0 {
                findings.push(LintFinding {
                    code: "orphan_page".to_string(),
                    path: (*path).to_string(),
                    message: format!("page {path} has no incoming links in the bundle"),
                    line: None,
                });
            }
        }
    }

    // 4. Missing index entry check
    // For each directory, verify index.md references all sibling pages in that directory
    for section in &sections {
        let section_index = format!("{section}/index.md");
        if let Some(links) = page_links.get(&section_index) {
            let indexed_targets: HashSet<&str> = links
                .iter()
                .filter_map(|l| l.resolved_path.as_deref())
                .collect();

            for path in &all_paths {
                if path.starts_with(section.as_str()) && *path != section_index.as_str() {
                    // Sibling file in this section
                    if !indexed_targets.contains(*path) {
                        findings.push(LintFinding {
                            code: "missing_index_entry".to_string(),
                            path: (*path).to_string(),
                            message: format!("page {path} is not listed in section index"),
                            line: None,
                        });
                    }
                }
            }
        }
    }

    // Sort findings deterministically by path and code
    findings.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.code.cmp(&b.code)));

    (findings, backlink_graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lint_bundle() {
        let mut pages = HashMap::new();
        pages.insert(
            "/fs/index.md".to_string(),
            "---\nokf_version: \"0.2\"\n---\n# Root\n[Services](services/index.md)\n".to_string(),
        );
        pages.insert("/fs/log.md".to_string(), "# Log\n".to_string());
        pages.insert(
            "/fs/services/index.md".to_string(),
            "# Services\n[Caddy](caddy.md)\n".to_string(),
        );
        pages.insert(
            "/fs/services/caddy.md".to_string(),
            "---\ntype: concept\ntitle: Caddy\n---\n# Caddy\n".to_string(),
        );

        let (findings, backlinks) = lint_bundle(&pages);
        assert_eq!(findings, vec![]);

        let caddy_backlinks = backlinks.backlinks_for("/fs/services/caddy.md");
        assert_eq!(caddy_backlinks.len(), 1);
        assert_eq!(caddy_backlinks[0].path, "/fs/services/index.md");
    }

    #[test]
    fn test_lint_broken_link_and_orphan() {
        let mut pages = HashMap::new();
        pages.insert(
            "/fs/index.md".to_string(),
            "---\nokf_version: \"0.2\"\n---\n# Root\n[Missing](nonexistent.md)\n".to_string(),
        );
        pages.insert("/fs/log.md".to_string(), "# Log\n".to_string());
        pages.insert(
            "/fs/services/index.md".to_string(),
            "# Services\n".to_string(),
        );
        pages.insert(
            "/fs/services/caddy.md".to_string(),
            "---\ntype: concept\ntitle: Caddy\n---\n# Caddy\n".to_string(),
        );

        let (findings, _) = lint_bundle(&pages);
        let codes: Vec<&str> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"broken_link"));
        assert!(codes.contains(&"orphan_page"));
        assert!(codes.contains(&"missing_index_entry"));
    }
    #[test]
    fn a_page_that_links_to_itself_is_not_its_own_referrer() {
        let mut pages = HashMap::new();
        pages.insert(
            "/fs/index.md".to_string(),
            "---\nokf_version: \"0.2\"\n---\n# Root\n[Linked](linked.md)\n".to_string(),
        );
        pages.insert("/fs/log.md".to_string(), "# Log\n".to_string());
        pages.insert(
            "/fs/alone.md".to_string(),
            "---\ntype: concept\ntitle: Alone\n---\nSee [above](alone.md#top) and [here](./alone.md).\n"
                .to_string(),
        );
        pages.insert(
            "/fs/linked.md".to_string(),
            "---\ntype: concept\ntitle: Linked\n---\nSee [myself](/fs/linked.md).\n".to_string(),
        );

        let (findings, backlinks) = lint_bundle(&pages);
        assert!(backlinks.backlinks_for("/fs/alone.md").is_empty());
        let referring: Vec<String> = backlinks
            .backlinks_for("/fs/linked.md")
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(referring, ["/fs/index.md"]);

        let orphans: Vec<&str> = findings
            .iter()
            .filter(|finding| finding.code == "orphan_page")
            .map(|finding| finding.path.as_str())
            .collect();
        assert_eq!(
            orphans,
            ["/fs/alone.md"],
            "a link to itself does not bring a page into the bundle"
        );
    }

    /// The write stays lenient, so a page with no block is stored as it was
    /// sent. The finding is the only place a writer learns the shape, so it
    /// carries the minimum to add rather than only what is missing.
    #[test]
    fn a_page_with_no_frontmatter_is_told_the_minimum_to_add() {
        let concept = lint_page_write("/fs/notes.md", "# Notes\n", &[]);
        assert_eq!(concept.len(), 1);
        assert_eq!(concept[0].code, "missing_frontmatter");
        assert!(
            concept[0].message.contains("type: Concept"),
            "a concept page is told the field it needs: {}",
            concept[0].message
        );

        let root = lint_page_write("/fs/index.md", "# Root\n", &[]);
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].code, "missing_frontmatter");
        assert!(
            root[0].message.contains("okf_version"),
            "the bundle root is told the field only it carries: {}",
            root[0].message
        );

        // A page that says nothing about itself is asked for nothing, and the
        // whole-bundle walk says the same thing the write does.
        let mut pages = HashMap::new();
        pages.insert("/fs/notes.md".to_string(), "# Notes\n".to_string());
        let (findings, _) = lint_bundle(&pages);
        let missing: Vec<&LintFinding> = findings
            .iter()
            .filter(|finding| finding.code == "missing_frontmatter")
            .collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].message, concept[0].message);
    }

    #[test]
    fn a_section_index_and_the_log_are_not_asked_for_frontmatter() {
        for path in ["/fs/log.md", "/fs/runbooks/index.md"] {
            assert_eq!(
                lint_page_write(path, "# Plain\n", &[]),
                vec![],
                "{path} is located by its place in the bundle, not by its own fields"
            );
        }
    }
}
