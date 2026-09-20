//! A small markdown to HTML renderer for artifact content.
//!
//! Markdown artifacts are read in two places: the public artifact route and the
//! in-app viewer, which reads the rendered HTML from the content API. Both go
//! through [`to_html`], so there is one renderer to reason about.
//!
//! The renderer is hand-rolled on purpose. Its contract is total escaping: every
//! character of the source is escaped, and raw HTML embedded in the markdown is
//! shown as text rather than passed through. A full parser with an HTML escape
//! hook would also work, but the supported subset is fixed and the escaping
//! guarantee is the whole point, so the smaller surface wins and the binary
//! gains no dependency.
//!
//! Supported: ATX headings, paragraphs, emphasis and strong emphasis, inline
//! code, fenced code blocks, unordered and ordered lists, and links. Anything
//! else is text. A link with an unsafe scheme renders as its label text.

/// Render markdown to an HTML fragment.
///
/// The output keeps only the tags and attributes this renderer emits, so it is
/// safe to place inside a sandboxed document. It is a fragment, not a document.
pub fn to_html(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(source.len() + 64);
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some((marker, lang)) = fence_open(line) {
            index += 1;
            let mut code = String::new();
            while index < lines.len() && !fence_close(lines[index], marker) {
                code.push_str(lines[index]);
                code.push('\n');
                index += 1;
            }
            if index < lines.len() {
                index += 1;
            }
            if !lang.is_empty()
                && lang
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            {
                out.push_str(&format!("<pre><code class=\"language-{lang}\">"));
            } else {
                out.push_str("<pre><code>");
            }
            out.push_str(&escape_html(&code));
            out.push_str("</code></pre>\n");
            continue;
        }
        if let Some((level, rest)) = heading(line) {
            out.push_str(&format!("<h{level}>{}</h{level}>\n", inline(rest)));
            index += 1;
            continue;
        }
        if list_item(line, false).is_some() {
            out.push_str("<ul>\n");
            index = list_block(&lines, index, false, &mut out);
            out.push_str("</ul>\n");
            continue;
        }
        if list_item(line, true).is_some() {
            out.push_str("<ol>\n");
            index = list_block(&lines, index, true, &mut out);
            out.push_str("</ol>\n");
            continue;
        }
        let mut paragraph = String::new();
        while index < lines.len() && !is_block_start(lines[index]) {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(lines[index].trim());
            index += 1;
        }
        if !paragraph.is_empty() {
            out.push_str("<p>");
            out.push_str(&inline(&paragraph));
            out.push_str("</p>\n");
        }
    }
    out
}

/// Escape a string for HTML text and attribute contexts.
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Render inline spans: code, links, strong, and emphasis.
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '`' => {
                if let Some(end) = find_from(&chars, index + 1, |ch| ch == '`') {
                    let code: String = chars[index + 1..end].iter().collect();
                    out.push_str("<code>");
                    out.push_str(&escape_html(&code));
                    out.push_str("</code>");
                    index = end + 1;
                } else {
                    out.push_str("&#96;");
                    index += 1;
                }
            }
            '*' | '_' => {
                if let Some((html, next)) = emphasis(&chars, index) {
                    out.push_str(&html);
                    index = next;
                } else {
                    out.push_str(&escape_html(&chars[index].to_string()));
                    index += 1;
                }
            }
            '[' => {
                if let Some((label, url, next)) = link(&chars, index) {
                    if let Some(href) = safe_href(&url) {
                        out.push_str("<a href=\"");
                        out.push_str(&escape_html(&href));
                        out.push_str("\" rel=\"noopener noreferrer\">");
                        out.push_str(&inline(&label));
                        out.push_str("</a>");
                    } else {
                        out.push_str(&inline(&label));
                    }
                    index = next;
                } else {
                    out.push_str("&#91;");
                    index += 1;
                }
            }
            other => {
                out.push_str(&escape_html(&other.to_string()));
                index += 1;
            }
        }
    }
    out
}

/// Parse `**strong**` or `*emphasis*` at `start`, returning the HTML and the
/// index just past the closing marker.
fn emphasis(chars: &[char], start: usize) -> Option<(String, usize)> {
    let marker = chars[start];
    if chars.get(start + 1) == Some(&marker)
        && let Some(end) = find_from(chars, start + 2, |ch| ch == marker)
        && end > start + 2
        && chars.get(end + 1) == Some(&marker)
    {
        let content: String = chars[start + 2..end].iter().collect();
        return Some((format!("<strong>{}</strong>", inline(&content)), end + 2));
    }
    let end = find_from(chars, start + 1, |ch| ch == marker)?;
    if end > start + 1 {
        let content: String = chars[start + 1..end].iter().collect();
        Some((format!("<em>{}</em>", inline(&content)), end + 1))
    } else {
        None
    }
}

/// Parse `[label](url)` at `start`, returning the label, the URL, and the index
/// just past the closing parenthesis.
fn link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = find_from(chars, start + 1, |ch| ch == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut depth = 0i32;
    let mut paren = None;
    for (index, ch) in chars.iter().enumerate().skip(close + 1) {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    paren = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let paren = paren?;
    let label: String = chars[start + 1..close].iter().collect();
    let url: String = chars[close + 2..paren].iter().collect();
    Some((label, url, paren + 1))
}

/// Accept only links that cannot smuggle a script URL. An unknown scheme is
/// refused; a relative link is allowed.
fn safe_href(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return None;
    }
    match url.find(':') {
        None => Some(url.to_string()),
        Some(index) => {
            let prefix = &url[..index];
            if prefix.contains(['/', '?', '#']) {
                return Some(url.to_string());
            }
            match prefix.to_ascii_lowercase().as_str() {
                "http" | "https" | "mailto" => Some(url.to_string()),
                _ => None,
            }
        }
    }
}

/// Find the first character matching `wanted` at or after `from`.
fn find_from(chars: &[char], from: usize, wanted: impl Fn(char) -> bool) -> Option<usize> {
    (from..chars.len()).find(|&index| wanted(chars[index]))
}

/// An ATX heading, returning its level and the text after the marker.
fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed[hashes..].strip_prefix(|ch| ch == ' ' || ch == '\t')?;
    Some((hashes, rest.trim()))
}

/// A fenced code block opening, returning its fence marker and language info string.
fn fence_open(line: &str) -> Option<(char, &str)> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|ch| *ch == marker).count();
    if count >= 3 {
        let rest = trimmed[count..].trim();
        let lang = rest.split_whitespace().next().unwrap_or("");
        Some((marker, lang))
    } else {
        None
    }
}

/// Whether a line closes a fenced code block opened with `marker`.
fn fence_close(line: &str, marker: char) -> bool {
    let trimmed = line.trim();
    let count = trimmed.chars().take_while(|ch| *ch == marker).count();
    count >= 3 && trimmed[count..].trim().is_empty()
}

/// A list item's text, when the line starts one of the wanted list type.
fn list_item(line: &str, ordered: bool) -> Option<String> {
    let trimmed = line.trim_start();
    if ordered {
        let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let rest = trimmed[digits..]
            .strip_prefix(|ch| ch == '.' || ch == ')')?
            .strip_prefix(|ch| ch == ' ' || ch == '\t')?;
        Some(rest.to_string())
    } else {
        let rest = trimmed
            .strip_prefix(|ch| ch == '-' || ch == '*' || ch == '+')?
            .strip_prefix(|ch| ch == ' ' || ch == '\t')?;
        Some(rest.to_string())
    }
}

/// Emit `<li>` rows until the list ends, returning the next line index.
fn list_block(lines: &[&str], mut index: usize, ordered: bool, out: &mut String) -> usize {
    while index < lines.len() {
        match list_item(lines[index], ordered) {
            Some(item) => {
                out.push_str("<li>");
                out.push_str(&inline(&item));
                out.push_str("</li>\n");
                index += 1;
            }
            None => break,
        }
    }
    index
}

/// Whether a line starts a block, ending the paragraph that precedes it.
fn is_block_start(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty()
        || heading(line).is_some()
        || fence_open(line).is_some()
        || list_item(line, false).is_some()
        || list_item(line, true).is_some()
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn renders_headings_and_paragraphs() {
        assert_eq!(to_html("# Title"), "<h1>Title</h1>\n");
        assert_eq!(
            to_html("first line\nsecond line"),
            "<p>first line second line</p>\n"
        );
    }

    #[test]
    fn renders_emphasis_and_code() {
        assert_eq!(
            to_html("*one* and **two** and `three`"),
            "<p><em>one</em> and <strong>two</strong> and <code>three</code></p>\n"
        );
    }

    #[test]
    fn renders_lists() {
        assert_eq!(
            to_html("- one\n- two"),
            "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n"
        );
        assert_eq!(
            to_html("1. one\n2. two"),
            "<ol>\n<li>one</li>\n<li>two</li>\n</ol>\n"
        );
    }

    #[test]
    fn renders_fenced_code_with_escaped_contents() {
        assert_eq!(
            to_html("```\n<tag> & \"quote\"\n```"),
            "<pre><code>&lt;tag&gt; &amp; &quot;quote&quot;\n</code></pre>\n"
        );
    }

    #[test]
    fn renders_fenced_code_with_language_class() {
        assert_eq!(
            to_html("```mermaid\ngraph TD\n```"),
            "<pre><code class=\"language-mermaid\">graph TD\n</code></pre>\n"
        );
    }

    #[test]
    fn escapes_raw_html_in_source() {
        let html = to_html("<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[test]
    fn raw_html_reaches_reader_as_text() {
        let html = to_html("<div id=\"raw\">content</div>");
        assert!(!html.contains("<div"));
        assert!(html.contains("&lt;div id=&quot;raw&quot;&gt;content&lt;/div&gt;"));
    }

    #[test]
    fn script_tag_reaches_reader_as_text() {
        let html = to_html("<script>alert('xss')</script>");
        assert!(!html.contains("<script"));
        assert!(html.contains("&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt;"));
    }

    #[test]
    fn event_handler_attribute_reaches_reader_as_text() {
        let html = to_html("<img src=\"x\" onerror=\"alert(1)\">");
        assert!(!html.contains("<img"));
        assert!(html.contains("&lt;img src=&quot;x&quot; onerror=&quot;alert(1)&quot;&gt;"));
    }

    #[test]
    fn javascript_url_reaches_reader_as_text() {
        let html = to_html("[click](javascript:alert(1))");
        assert!(!html.contains("href="));
        assert!(!html.contains("javascript:"));
        assert_eq!(html, "<p>click</p>\n");
    }

    #[test]
    fn keeps_safe_links_and_drops_script_links() {
        assert_eq!(
            to_html("[docs](https://example.com/a)"),
            "<p><a href=\"https://example.com/a\" rel=\"noopener noreferrer\">docs</a></p>\n"
        );
        assert_eq!(to_html("[x](javascript:alert(1))"), "<p>x</p>\n");
        assert_eq!(to_html("[x](data:text/html,hi)"), "<p>x</p>\n");
    }
}
