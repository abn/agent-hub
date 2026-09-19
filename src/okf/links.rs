//! Link extraction from markdown documents.
//!
//! Extracts inline, reference-style, and shortcut links while ignoring code blocks,
//! inline code spans, and non-prose text. Resolves relative paths within the OKF bundle.

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
    let mut in_fence = false;
    let mut fence_marker = "";

    // First pass: collect reference definitions `[label]: target`
    let mut ref_defs: HashMap<String, (String, usize)> = HashMap::new();

    let lines: Vec<&str> = content.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if !in_fence && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = true;
            fence_marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            continue;
        } else if in_fence && trimmed.starts_with(fence_marker) {
            in_fence = false;
            fence_marker = "";
            continue;
        }

        if in_fence {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix('[')
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
    in_fence = false;
    fence_marker = "";

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if !in_fence && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = true;
            fence_marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            continue;
        } else if in_fence && trimmed.starts_with(fence_marker) {
            in_fence = false;
            fence_marker = "";
            continue;
        }

        if in_fence {
            continue;
        }

        // Strip inline code spans from the line
        let prose = strip_code_spans(line);

        // Find inline links [text](target)
        let mut search_pos = 0;
        while let Some(open_sq) = prose[search_pos..].find('[') {
            let abs_sq = search_pos + open_sq;
            if let Some(close_sq) = prose[abs_sq..].find(']') {
                let abs_close_sq = abs_sq + close_sq;
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

fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for c in line.chars() {
        if c == '`' {
            in_code = !in_code;
        } else if !in_code {
            out.push(c);
        }
    }

    out
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

    #[test]
    fn test_escapes_bundle() {
        let (resolved, escapes) = resolve_bundle_path("/fs/index.md", "../../etc/passwd");
        assert!(escapes);
        assert!(!resolved.starts_with("/fs"));
    }
}
