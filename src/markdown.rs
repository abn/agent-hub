//! Escaping for text the hub puts into HTML it generates.
//!
//! The hub does not render markdown itself: the browser parses it with the
//! parser already shipped for protected artifacts, whose plaintext the server
//! never sees. One renderer, and it is the one that can read what agents write.
//!
//! What remains is the escape used wherever the hub builds markup itself: the
//! viewer shell, the frame document, and the preview tags.

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The five characters that end an escape, each on its own, because a
    /// table of them all at once passes while four of the five are broken.
    #[test]
    fn every_dangerous_character_is_escaped() {
        assert_eq!(escape_html("&"), "&amp;");
        assert_eq!(escape_html("<"), "&lt;");
        assert_eq!(escape_html(">"), "&gt;");
        assert_eq!(escape_html("\""), "&quot;");
        assert_eq!(escape_html("'"), "&#39;");
    }

    /// The ampersand has to go first or every other escape is re-escaped into
    /// its own literal text.
    #[test]
    fn an_escape_is_not_escaped_twice() {
        assert_eq!(escape_html("<a>"), "&lt;a&gt;");
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    /// What the callers actually pass: a title, an id, an agent's own words.
    #[test]
    fn ordinary_text_passes_through_unchanged() {
        assert_eq!(escape_html("Notes on the audit"), "Notes on the audit");
        assert_eq!(escape_html("01M32EH3FKZJ"), "01M32EH3FKZJ");
        assert_eq!(escape_html(""), "");
    }

    /// Markup an agent wrote, which reaches a reader as the characters typed
    /// rather than as an element.
    #[test]
    fn authored_markup_becomes_text() {
        assert_eq!(
            escape_html("<script>alert('x')</script>"),
            "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"
        );
        assert_eq!(
            escape_html("\" onerror=\"boom"),
            "&quot; onerror=&quot;boom"
        );
    }

    /// The escape is per character, so text that carries none of them keeps
    /// its own length and its own bytes.
    #[test]
    fn text_outside_the_five_is_left_alone() {
        let text = "a path/to/file, 90% done - \u{e9}t\u{e9}";
        assert_eq!(escape_html(text), text);
    }
}
