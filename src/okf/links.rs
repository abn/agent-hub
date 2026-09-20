//! Link extraction from markdown documents.
//!
//! Extracts inline and reference-style links from prose and resolves relative
//! paths within the OKF bundle. A link inside code is not a link. Code is:
//!
//! - A fenced block: a line of three or more backticks or tildes opens it (a
//!   backtick fence whose info string holds a backtick is a code span, not a
//!   fence), and only a line of the same character, at least as long, with
//!   nothing after it, closes it. So a longer fence holds a shorter one, and a
//!   tilde fence holds a backtick one. An unclosed fence runs to the end.
//! - An indented block outside a list: lines indented four spaces or a tab,
//!   starting after a blank line, a heading, a fence or the top of the page,
//!   up to the next line indented less. An indented line straight after a
//!   paragraph line continues that paragraph and is prose.
//! - A code span: a run of backticks up to the next run of the same length on
//!   the line. A run with no partner is literal.
//!
//! Inside a list an indented line is a continuation or a nested item, so it is
//! read as prose, and a fence there is seen at any indentation. A list ends at
//! a column-zero line that follows a blank line and is not an item. An
//! indented code block nested in a list is therefore read as prose.

use std::collections::HashMap;

/// A link occurrence extracted from a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedLink {
    /// The normalized target path or URL.
    pub target: String,
    /// 1-based line number where the link was found.
    pub line: usize,
    /// Whether this is an internal link within the bundle.
    pub is_internal: bool,
    /// Whether the link escapes the root of the bundle.
    pub escapes_bundle: bool,
    /// If internal and within bundle, the resolved canonical path (e.g. `/fs/...`).
    pub resolved_path: Option<String>,
}

/// Extract links from a markdown document.
///
/// `current_page_path` is the canonical path of the document (e.g. `/fs/services/caddy.md`).
pub fn extract_links(current_page_path: &str, content: &str) -> Vec<ExtractedLink> {
    let mut links = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let code = code_lines(&lines);
    let prose = lines
        .iter()
        .enumerate()
        .filter(|(idx, _)| !code[*idx])
        .map(|(idx, line)| (idx + 1, *line));

    // First pass: collect reference definitions `[label]: target`
    let mut ref_defs: HashMap<String, (String, usize)> = HashMap::new();
    for (line_num, line) in prose.clone() {
        if let Some(rest) = line.trim().strip_prefix('[')
            && let Some(close_bracket) = rest.find("]:")
        {
            let label = rest[..close_bracket].trim().to_lowercase();
            let rest_target = rest[close_bracket + 2..].trim();
            let target = rest_target
                .strip_prefix('<')
                .and_then(|r| r.strip_suffix('>'))
                .unwrap_or(rest_target);
            let target = target.split_whitespace().next().unwrap_or(target);
            if !target.is_empty() {
                ref_defs.insert(label, (target.to_string(), line_num));
            }
        }
    }

    // Second pass: extract inline links and reference uses
    for (line_num, line) in prose {
        // Strip inline code spans from the line
        let prose = strip_code_spans(line);

        // Find inline links [text](target)
        let mut search_pos = 0;
        // Where the next `]` is, kept between brackets. A page is agent-written
        // and this runs inside a put, so a line of `[` must cost one pass, not
        // one pass per bracket.
        let mut next_close: Option<usize> = None;
        while let Some(open_sq) = prose[search_pos..].find('[') {
            let abs_sq = search_pos + open_sq;
            if next_close.is_none_or(|at| at < abs_sq) {
                next_close = prose[abs_sq..].find(']').map(|at| abs_sq + at);
            }
            // No `]` after this bracket means none after any later one either.
            let Some(abs_close_sq) = next_close else {
                break;
            };
            {
                let after_close = &prose[abs_close_sq + 1..];

                if after_close.starts_with('(') {
                    if let Some(close_paren) = after_close.find(')') {
                        let inner = after_close[1..close_paren].trim();
                        let target = inner
                            .strip_prefix('<')
                            .and_then(|r| r.strip_suffix('>'))
                            .unwrap_or(inner);
                        let target = target.split_whitespace().next().unwrap_or(target);

                        if !target.is_empty() {
                            links.push(classify_link(current_page_path, target, line_num));
                        }
                        search_pos = abs_close_sq + 1 + close_paren + 1;
                        continue;
                    }
                } else if after_close.starts_with('[') {
                    // Reference link [text][label]
                    if let Some(close_ref) = after_close.find(']') {
                        let label = after_close[1..close_ref].trim().to_lowercase();
                        if let Some((target, _)) = ref_defs.get(&label) {
                            links.push(classify_link(current_page_path, target, line_num));
                        }
                        search_pos = abs_close_sq + 1 + close_ref + 1;
                        continue;
                    }
                }
            }
            search_pos = abs_sq + 1;
        }
    }

    links
}

/// The length of the run of `marker` that `text` starts with.
fn run_length(text: &str, marker: char) -> usize {
    text.chars().take_while(|c| *c == marker).count()
}

/// The fence a line opens, as its character and length.
fn opens_fence(rest: &str) -> Option<(char, usize)> {
    let marker = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = run_length(rest, marker);
    // A backtick in the info string makes the line a code span.
    let is_fence = len >= 3 && !(marker == '`' && rest[len..].contains('`'));
    is_fence.then_some((marker, len))
}

/// Whether a line starts a list item: a bullet or a number, then a space.
fn is_list_item(rest: &str) -> bool {
    let after_marker = match rest.chars().next() {
        Some('-' | '*' | '+') => &rest[1..],
        Some(c) if c.is_ascii_digit() => {
            let digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            match digits.strip_prefix(['.', ')']) {
                Some(after) => after,
                None => return false,
            }
        }
        _ => return false,
    };
    after_marker.is_empty() || after_marker.starts_with([' ', '\t'])
}

/// Which lines are code, by the rules in the module documentation.
fn code_lines(lines: &[&str]) -> Vec<bool> {
    let mut code = vec![false; lines.len()];
    let mut fence: Option<(char, usize)> = None;
    let mut in_list = false;
    let mut in_indented = false;
    let mut after_blank = true;
    // Whether an indented line here would start a code block and not continue
    // a paragraph.
    let mut may_start_indented = true;

    for (idx, line) in lines.iter().enumerate() {
        let rest = line.trim_start_matches([' ', '\t']);
        let indent = &line[..line.len() - rest.len()];
        let deep = indent.contains('\t') || indent.len() >= 4;

        if let Some((marker, len)) = fence {
            code[idx] = true;
            let run = run_length(rest, marker);
            if run >= len && rest[run..].trim().is_empty() {
                fence = None;
                may_start_indented = true;
            }
            after_blank = false;
            continue;
        }
        if rest.trim().is_empty() {
            code[idx] = in_indented;
            after_blank = true;
            may_start_indented = true;
            continue;
        }
        if !in_list && deep && (in_indented || may_start_indented) {
            code[idx] = true;
            in_indented = true;
            after_blank = false;
            continue;
        }
        in_indented = false;

        if is_list_item(rest) && (in_list || !deep) {
            in_list = true;
        } else if indent.is_empty() && after_blank {
            in_list = false;
        }
        after_blank = false;

        if (in_list || !deep)
            && let Some(opened) = opens_fence(rest)
        {
            fence = Some(opened);
            code[idx] = true;
            continue;
        }
        // A heading is one to six `#` and then a space or the end of the line.
        // `#tag` is a paragraph, and an indented line after it continues it.
        let hashes = run_length(rest, '#');
        may_start_indented = (1..=6).contains(&hashes)
            && rest[hashes..]
                .chars()
                .next()
                .is_none_or(|next| next == ' ' || next == '\t');
    }
    code
}

/// A line with its code spans removed. A span is a run of backticks up to the
/// next run of the same length; a run with no partner stays as it is.
fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let len = run_length(&rest[start..], '`');
        let after = &rest[start + len..];
        match closing_run(after, len) {
            Some(end) => rest = &after[end + len..],
            None => {
                out.push_str(&rest[start..start + len]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The offset in `text` of the next run of exactly `len` backticks.
fn closing_run(text: &str, len: usize) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = text[from..].find('`') {
        let at = from + found;
        let run = run_length(&text[at..], '`');
        if run == len {
            return Some(at);
        }
        from = at + run;
    }
    None
}

fn classify_link(current_page_path: &str, raw_target: &str, line: usize) -> ExtractedLink {
    let target = raw_target
        .split('#')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");

    if target.is_empty() {
        return ExtractedLink {
            target: raw_target.to_string(),
            line,
            is_internal: false,
            escapes_bundle: false,
            resolved_path: None,
        };
    }

    // Check for URI schemes
    if let Some((scheme, _)) = target.split_once(':') {
        let is_ascii_scheme = scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.');
        if is_ascii_scheme {
            // External or non-relative link (e.g. https://, mailto:, file://)
            return ExtractedLink {
                target: target.to_string(),
                line,
                is_internal: false,
                escapes_bundle: false,
                resolved_path: None,
            };
        }
    }

    // Internal link within bundle
    let (resolved, escapes) = resolve_bundle_path(current_page_path, target);

    ExtractedLink {
        target: target.to_string(),
        line,
        is_internal: true,
        escapes_bundle: escapes,
        resolved_path: if escapes { None } else { Some(resolved) },
    }
}

/// Resolve a relative or absolute link target against the current page path.
///
/// Current page path is assumed to be under `/fs/` (e.g. `/fs/services/caddy.md`).
/// Returns `(canonical_resolved_path, escapes_bundle)`.
pub fn resolve_bundle_path(current_page_path: &str, target: &str) -> (String, bool) {
    let current_rel = current_page_path
        .strip_prefix("/fs/")
        .or_else(|| current_page_path.strip_prefix("/fs"))
        .unwrap_or("");
    let current_dir_rel = match current_rel.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };

    let parts: Vec<&str> = if current_dir_rel.is_empty() {
        Vec::new()
    } else {
        current_dir_rel
            .split('/')
            .filter(|p| !p.is_empty())
            .collect()
    };

    let (target_rel, mut parts) = if let Some(stripped) = target.strip_prefix("/fs/") {
        (stripped, Vec::new())
    } else if target == "/fs" {
        ("", Vec::new())
    } else if let Some(stripped) = target.strip_prefix('/') {
        (stripped, Vec::new())
    } else {
        (target, parts)
    };

    let mut escapes = false;
    for part in target_rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    escapes = true;
                }
            }
            name => parts.push(name),
        }
    }

    if escapes {
        (String::new(), true)
    } else if parts.is_empty() {
        ("/fs".to_string(), false)
    } else {
        (format!("/fs/{}", parts.join("/")), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_without_a_space_is_a_paragraph_not_a_heading() {
        // A heading is one to six `#` and then a space or the end of the line.
        // `#tag` is prose, so the indented line after it continues the
        // paragraph and its link counts; after a real heading it is code.
        let tagged = "#tag words\n    [a](a.md)\n";
        let targets = |content: &str| -> Vec<String> {
            extract_links("/fs/page.md", content)
                .into_iter()
                .map(|link| link.target)
                .collect()
        };
        assert_eq!(targets(tagged), vec!["a.md"]);
        assert_eq!(targets("####### seven\n    [a](a.md)\n"), vec!["a.md"]);
        assert!(targets("# Heading\n    [a](a.md)\n").is_empty());
        assert!(targets("###\n    [a](a.md)\n").is_empty());
    }

    #[test]
    fn brackets_that_never_close_cost_one_pass() {
        // A page is agent-written and up to 1 MiB, and this runs inside a put.
        // A line of `[` with no `]`, or with one at the very end, used to be
        // rescanned from every bracket.
        let started = std::time::Instant::now();
        let open = "[".repeat(1 << 20);
        assert!(extract_links("/fs/page.md", &open).is_empty());
        let late = format!("{}](a.md)", "[".repeat(1 << 20));
        let _ = extract_links("/fs/page.md", &late);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "two 1 MiB lines of brackets took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn test_extract_links() {
        let content = r#"
# Heading

An inline link to [Caddy](caddy.md) and an absolute [Root](/fs/index.md).
An external [Google](https://google.com) and a reference [Doc][doc-ref].

```rust
// [Ignored in code](ignored.md)
```

[doc-ref]: ../guide.md
"#;
        let links = extract_links("/fs/services/index.md", content);
        assert_eq!(links.len(), 4);

        assert_eq!(links[0].target, "caddy.md");
        assert_eq!(
            links[0].resolved_path.as_deref(),
            Some("/fs/services/caddy.md")
        );
        assert!(!links[0].escapes_bundle);

        assert_eq!(links[1].target, "/fs/index.md");
        assert_eq!(links[1].resolved_path.as_deref(), Some("/fs/index.md"));

        assert_eq!(links[2].target, "https://google.com");
        assert!(!links[2].is_internal);

        assert_eq!(links[3].target, "../guide.md");
        assert_eq!(links[3].resolved_path.as_deref(), Some("/fs/guide.md"));
    }

    fn targets(content: &str) -> Vec<String> {
        extract_links("/fs/page.md", content)
            .into_iter()
            .map(|link| link.target)
            .collect()
    }

    #[test]
    fn a_longer_fence_holds_a_shorter_one() {
        let content = "\
Before [a](a.md).

````markdown
```
[inner](inner.md)
```
[still code](still.md)
````

Between [b](b.md).

~~~~
~~~
[tilde](tilde.md)
~~~
[tilde still code](tilde-still.md)
~~~~

After [c](c.md).
";
        assert_eq!(targets(content), ["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn a_fence_closes_only_on_its_own_character_with_nothing_after_it() {
        let content = "\
```
~~~
[one](one.md)
``` not a closer
[two](two.md)
``` ```
[three](three.md)
```
Prose [p](p.md).
";
        assert_eq!(targets(content), ["p.md"]);
    }

    #[test]
    fn a_backtick_line_with_a_backtick_in_its_info_string_is_a_span_not_a_fence() {
        let content = "```not a fence``` and [a](a.md)\n[b](b.md)\n";
        assert_eq!(targets(content), ["a.md", "b.md"]);
    }

    #[test]
    fn an_indented_code_block_holds_no_links() {
        let content = "\
Prose [a](a.md).

    [code](code.md)

    [more code](more.md)
\t[tabbed code](tabbed.md)

Back to prose [b](b.md).
    [a lazy continuation is prose](lazy.md)
";
        assert_eq!(targets(content), ["a.md", "b.md", "lazy.md"]);
    }

    #[test]
    fn an_indented_line_inside_a_list_is_prose() {
        let content = "\
- item [a](a.md)

    continued [b](b.md)

    - nested [c](c.md)

    ```
    [fenced in the list](fenced.md)
    ```

Paragraph.

    [code again](code.md)
";
        assert_eq!(targets(content), ["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn a_code_span_of_any_length_holds_no_links() {
        let content = "\
A ``[double](double.md)`` span, a `` ` [tick inside](tick.md) `` span,
a `[single](single.md)` span and [prose](prose.md).
An unclosed `` run is literal: [kept](kept.md).
";
        assert_eq!(targets(content), ["prose.md", "kept.md"]);
    }

    #[test]
    fn a_reference_definition_inside_code_defines_nothing() {
        let content = "\
See [the doc][doc].

````
```
[doc]: inside.md
```
````

    [doc]: indented.md
";
        assert!(targets(content).is_empty(), "{:?}", targets(content));
    }

    #[test]
    fn test_escapes_bundle() {
        let (resolved, escapes) = resolve_bundle_path("/fs/index.md", "../../etc/passwd");
        assert!(escapes);
        assert!(!resolved.starts_with("/fs"));
    }
}
