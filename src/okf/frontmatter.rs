//! Line-oriented frontmatter layout scanner and restricted reader.
//!
//! There is no YAML parser here. A page is split into lines, the frontmatter
//! block is located, and each top-level key is given the exact byte range of
//! its lines. The reader and the patcher in [`patch`] share that one
//! scan, so they cannot disagree about where a value ends.
//!
//! # Layout rules
//!
//! - The block opens when the first line of the page is exactly `---` and
//!   closes at the next line that is exactly `---`. Lines end in LF or CRLF.
//! - A page has no block when its first line does not start with `---`, or
//!   starts with four or more dashes (a rule or a paragraph in the body). Any
//!   other first line that starts with `---` is refused.
//! - A top-level key is a column-zero line `<key>:` followed by a space, a tab
//!   or the end of the line. Its value is the rest of that line plus every
//!   following indented line. Blank lines and column-zero comment lines belong
//!   to the value only when an indented line follows them. A column-zero
//!   `- item` line belongs to the key above it when that key has no inline
//!   value (the indentless sequence form).
//!
//! # What is refused
//!
//! A page that cannot be laid out safely is refused with a typed error rather
//! than guessed at. [`FrontmatterError::code`] is the stable identifier:
//!
//! - `byte_order_mark`: the page starts with a byte order mark.
//! - `carriage_return`: a carriage return without a line feed on the opening
//!   line or inside the block.
//! - `delimiter`: a first line that is `---` followed by whitespace, with or
//!   without a comment or text after it, or a line inside the block that is
//!   exactly `...`, or `---` or `...` followed by a space or a tab. Only a
//!   line that is exactly `---` opens or closes.
//! - `unclosed`: the block opens and never closes.
//! - `unsupported_line`: a first line with anything but whitespace or a
//!   fourth dash straight after `---` (`---yaml`), a column-zero line inside the
//!   block that is neither a comment nor `key:` (a quoted or complex key, a
//!   flow mapping, a stray scalar), or an indented line before the first key.
//!
//! The patcher adds refusals of its own; see [`patch`].

use std::fmt;

use serde::{Deserialize, Serialize};

pub mod patch;

pub use patch::{
    Change, PatchValue, PromoteParams, Record, Scalar, patch_frontmatter, promote_frontmatter,
    review_frontmatter,
};

/// Why frontmatter could not be read or patched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontmatterError {
    /// A value the restricted reader could not interpret.
    Unparsed(String),
    /// The page starts with a byte order mark.
    ByteOrderMark,
    /// A carriage return without a line feed, at this 1-based line.
    CarriageReturn { line: usize },
    /// The block opens and never closes.
    Unclosed,
    /// A delimiter line that is not exactly `---`, at this 1-based line.
    Delimiter { line: usize },
    /// A line the layout rules do not cover, at this 1-based line.
    UnsupportedLine { line: usize },
    /// A key named by the patch appears more than once in the block.
    DuplicateKey(String),
    /// A key named by the patch carries an anchor or an alias.
    Anchor(String),
    /// An entry was to be appended under a key whose value is not a block
    /// sequence.
    NotBlockSequence(String),
    /// An entry was to be appended under a block sequence indented with tabs
    /// or unevenly.
    Indentation(String),
    /// A patch key that is not a plain identifier.
    InvalidKey(String),
    /// A patch names the same key twice.
    DuplicateChange(String),
    /// A patch value that cannot be written.
    InvalidValue { key: String, reason: String },
}

impl FrontmatterError {
    /// The stable identifier of this refusal, shared with the fixture corpus.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unparsed(_) => "unparsed",
            Self::ByteOrderMark => "byte_order_mark",
            Self::CarriageReturn { .. } => "carriage_return",
            Self::Unclosed => "unclosed",
            Self::Delimiter { .. } => "delimiter",
            Self::UnsupportedLine { .. } => "unsupported_line",
            Self::DuplicateKey(_) => "duplicate_key",
            Self::Anchor(_) => "anchor",
            Self::NotBlockSequence(_) => "not_block_sequence",
            Self::Indentation(_) => "indentation",
            Self::InvalidKey(_) => "invalid_key",
            Self::DuplicateChange(_) => "duplicate_change",
            Self::InvalidValue { .. } => "invalid_value",
        }
    }
}

impl fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unparsed(msg) => write!(f, "{msg}"),
            Self::ByteOrderMark => write!(f, "the page starts with a byte order mark"),
            Self::CarriageReturn { line } => {
                write!(f, "line {line} has a carriage return without a line feed")
            }
            Self::Unclosed => write!(f, "the frontmatter block opens and never closes"),
            Self::Delimiter { line } => write!(
                f,
                "line {line} is a frontmatter delimiter that is not exactly '---'"
            ),
            Self::UnsupportedLine { line } => write!(
                f,
                "line {line} of the frontmatter is neither a comment nor a plain 'key:' line"
            ),
            Self::DuplicateKey(key) => write!(f, "key '{key}' appears more than once"),
            Self::Anchor(key) => write!(f, "key '{key}' carries an anchor or an alias"),
            Self::NotBlockSequence(key) => {
                write!(f, "key '{key}' does not hold a block sequence")
            }
            Self::Indentation(key) => write!(
                f,
                "the block sequence under '{key}' is indented with tabs or unevenly"
            ),
            Self::InvalidKey(key) => write!(f, "'{key}' is not a plain frontmatter key"),
            Self::DuplicateChange(key) => write!(f, "the patch names '{key}' twice"),
            Self::InvalidValue { key, reason } => {
                write!(f, "the value for '{key}' cannot be written: {reason}")
            }
        }
    }
}

impl std::error::Error for FrontmatterError {}

impl From<FrontmatterError> for crate::Error {
    fn from(err: FrontmatterError) -> Self {
        Self::InvalidArgument(format!("frontmatter refused: {err}"))
    }
}

/// A recorded verification entry in frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Verification {
    pub by: String,
    pub at: String,
}

/// A citation entry in frontmatter sources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceCitation {
    pub title: String,
    pub resource: String,
}

/// Parsed frontmatter structure holding standard keys and raw text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub page_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub stale_after: Option<String>,
    pub okf_version: Option<String>,
    pub verified: Vec<Verification>,
    pub sources: Vec<SourceCitation>,
    pub custom_keys: Vec<(String, String)>,
    pub raw: String,
}

/// One top-level key and the byte range of its lines.
#[derive(Debug)]
struct Entry<'a> {
    pub key: &'a str,
    /// The text after the colon on the key line, trimmed.
    pub inline: &'a str,
    /// Offset of the key line.
    pub start: usize,
    /// Offset just past the key line and its line ending.
    pub line_end: usize,
    /// Offset just past the last line of the value.
    pub end: usize,
}

/// The located frontmatter block.
#[derive(Debug)]
struct Block<'a> {
    /// Offset of the first line after the opening delimiter.
    pub open_end: usize,
    /// Offset of the closing delimiter line.
    pub close_start: usize,
    pub entries: Vec<Entry<'a>>,
}

/// The layout of a page: its line ending and its block, when it has one.
#[derive(Debug)]
struct Layout<'a> {
    /// The ending written on new lines: the opening line's when there is a
    /// block, otherwise the first line's, otherwise LF.
    pub newline: &'static str,
    pub block: Option<Block<'a>>,
}

/// The line at `pos`: its content without the line ending, the offset of the
/// next line, and whether a line feed ended it.
fn line_at(text: &str, pos: usize) -> (&str, usize, bool) {
    let rest = &text[pos..];
    match rest.find('\n') {
        Some(idx) => {
            let raw = &rest[..idx];
            (raw.strip_suffix('\r').unwrap_or(raw), pos + idx + 1, true)
        }
        None => (rest, text.len(), false),
    }
}

/// Whether `content` is `marker` followed by nothing or by whitespace.
fn is_marker_line(content: &str, marker: &str) -> bool {
    content
        .strip_prefix(marker)
        .is_some_and(|tail| tail.is_empty() || tail.starts_with([' ', '\t']))
}

/// Whether what follows `---` on the first line is a fourth dash. Four or more
/// dashes start a rule or a paragraph in the body, never a block.
fn is_dash_rule(tail: &str) -> bool {
    tail.starts_with('-')
}

/// Split a column-zero line into its key and inline value.
fn split_key(content: &str) -> Option<(&str, &str)> {
    let mut from = 0;
    while let Some(found) = content[from..].find(':') {
        let idx = from + found;
        let after = &content[idx + 1..];
        if after.is_empty() || after.starts_with([' ', '\t']) {
            let key = content[..idx].trim_end();
            let plain = key.chars().next().is_some_and(|first| {
                !matches!(
                    first,
                    '"' | '\''
                        | '?'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '&'
                        | '*'
                        | '!'
                        | '|'
                        | '>'
                        | '%'
                        | '@'
                        | '`'
                        | ','
                )
            });
            if !plain || key.contains(" #") || key.contains("\t#") {
                return None;
            }
            return Some((key, after.trim()));
        }
        from = idx + 1;
    }
    None
}

/// Lay a page out, or refuse it.
fn scan(text: &str) -> Result<Layout<'_>, FrontmatterError> {
    if text.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(FrontmatterError::ByteOrderMark);
    }

    let (first, open_end, terminated) = line_at(text, 0);
    if !(first == "---" && terminated) {
        if let Some(tail) = first.strip_prefix("---")
            && !is_dash_rule(tail)
        {
            // Anything else that starts like the opener is refused: treating
            // it as a page with no block would write a second block above one
            // that a YAML reader already sees.
            if tail.contains('\r') {
                return Err(FrontmatterError::CarriageReturn { line: 1 });
            }
            if tail.is_empty() {
                return Err(FrontmatterError::Unclosed);
            }
            if tail.starts_with([' ', '\t']) || tail.trim().is_empty() {
                return Err(FrontmatterError::Delimiter { line: 1 });
            }
            return Err(FrontmatterError::UnsupportedLine { line: 1 });
        }
        let newline = match text.find('\n') {
            Some(idx) if text[..idx].ends_with('\r') => "\r\n",
            _ => "\n",
        };
        return Ok(Layout {
            newline,
            block: None,
        });
    }

    let newline = if text[..open_end].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let mut entries: Vec<Entry<'_>> = Vec::new();
    let mut pos = open_end;
    let mut line = 1;
    while pos < text.len() {
        line += 1;
        let (content, next, _) = line_at(text, pos);
        if content.contains('\r') {
            return Err(FrontmatterError::CarriageReturn { line });
        }
        if content == "---" {
            return Ok(Layout {
                newline,
                block: Some(Block {
                    open_end,
                    close_start: pos,
                    entries,
                }),
            });
        }
        if is_marker_line(content, "---") || is_marker_line(content, "...") {
            return Err(FrontmatterError::Delimiter { line });
        }

        let indented = content.starts_with([' ', '\t']);
        if content.trim().is_empty() || content.starts_with('#') {
            // Blank and comment lines join a value only if an indented line
            // follows, which the next extension does by moving `end` past them.
        } else if indented {
            match entries.last_mut() {
                Some(entry) => entry.end = next,
                None => return Err(FrontmatterError::UnsupportedLine { line }),
            }
        } else if is_marker_line(content, "-") {
            match entries.last_mut() {
                Some(entry) if entry.inline.is_empty() || entry.inline.starts_with('#') => {
                    entry.end = next;
                }
                _ => return Err(FrontmatterError::UnsupportedLine { line }),
            }
        } else {
            let Some((key, inline)) = split_key(content) else {
                return Err(FrontmatterError::UnsupportedLine { line });
            };
            entries.push(Entry {
                key,
                inline,
                start: pos,
                line_end: next,
                end: next,
            });
        }
        pos = next;
    }

    Err(FrontmatterError::Unclosed)
}

/// Parse frontmatter using the restricted line-oriented reader.
///
/// Supports top-level scalars, flow lists, and single-level block sequences
/// (`verified` and `sources`). Returns `Ok(None)` if no frontmatter is present
/// and an error for a page the layout rules refuse. When a key repeats, the
/// last one wins.
pub fn parse_frontmatter(text: &str) -> Result<Option<Frontmatter>, FrontmatterError> {
    let layout = scan(text)?;
    let Some(block) = layout.block else {
        return Ok(None);
    };

    let mut fm = Frontmatter {
        raw: text[block.open_end..block.close_start].to_string(),
        ..Default::default()
    };

    for entry in &block.entries {
        let key = entry.key;
        let inline = entry.inline;
        let block_lines: Vec<&str> = text[entry.line_end..entry.end].lines().collect();

        if inline.is_empty() || inline.starts_with('#') {
            match key {
                "verified" => fm.verified = parse_verified_block(&block_lines),
                "sources" => fm.sources = parse_sources_block(&block_lines),
                "tags" => fm.tags = parse_block_list(&block_lines),
                _ => fm
                    .custom_keys
                    .push((key.to_string(), block_lines.join("\n"))),
            }
        } else if is_block_scalar_header(inline) {
            assign_scalar(&mut fm, key, fold_lines("", &block_lines));
        } else if inline.starts_with('[') {
            match parse_flow_list(inline) {
                Some(items) if key == "tags" => fm.tags = items,
                _ => fm.custom_keys.push((key.to_string(), inline.to_string())),
            }
        } else {
            let folded = fold_lines(inline, &block_lines);
            assign_scalar(&mut fm, key, parse_scalar(&folded));
        }
    }

    Ok(Some(fm))
}

fn assign_scalar(fm: &mut Frontmatter, key: &str, val: String) {
    match key {
        "type" => fm.page_type = Some(val),
        "title" => fm.title = Some(val),
        "description" => fm.description = Some(val),
        "status" => fm.status = Some(val),
        "stale_after" => fm.stale_after = Some(val),
        "okf_version" => fm.okf_version = Some(val),
        other => fm.custom_keys.push((other.to_string(), val)),
    }
}

/// `|` or `>` with optional chomping and indentation indicators.
fn is_block_scalar_header(inline: &str) -> bool {
    let header = inline.split([' ', '\t']).next().unwrap_or(inline);
    let mut chars = header.chars();
    matches!(chars.next(), Some('|' | '>')) && chars.all(|c| matches!(c, '+' | '-' | '0'..='9'))
}

/// Join a first fragment and the non-blank continuation lines with spaces.
fn fold_lines(first: &str, lines: &[&str]) -> String {
    let mut out = first.to_string();
    for line in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// Whether what follows a closed quote is nothing or a comment.
fn only_comment_follows(rest: &str) -> bool {
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with('#')
}

/// Read one scalar: double quoted with escapes, single quoted, or plain with
/// an optional trailing comment. A quoted scalar that does not close cleanly
/// is returned as written.
fn parse_scalar(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        if let Some((value, after)) = read_double_quoted(rest)
            && only_comment_follows(after)
        {
            return value;
        }
        return raw.to_string();
    }
    if let Some(rest) = raw.strip_prefix('\'') {
        if let Some((value, after)) = read_single_quoted(rest)
            && only_comment_follows(after)
        {
            return value;
        }
        return raw.to_string();
    }
    let end = [" #", "\t#"]
        .iter()
        .filter_map(|marker| raw.find(marker))
        .min()
        .unwrap_or(raw.len());
    raw[..end].trim_end().to_string()
}

/// Read up to the closing double quote. Returns the value and what follows.
fn read_double_quoted(rest: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((idx, c)) = chars.next() {
        match c {
            '"' => return Some((out, &rest[idx + 1..])),
            '\\' => {
                let (_, escape) = chars.next()?;
                match escape {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '0' => out.push('\0'),
                    'a' => out.push('\x07'),
                    'b' => out.push('\x08'),
                    'e' => out.push('\x1b'),
                    'f' => out.push('\x0c'),
                    'v' => out.push('\x0b'),
                    'x' | 'u' | 'U' => {
                        let width = match escape {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let mut code = 0u32;
                        for _ in 0..width {
                            let (_, digit) = chars.next()?;
                            code = code.checked_mul(16)? + digit.to_digit(16)?;
                        }
                        out.push(char::from_u32(code)?);
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
    None
}

/// Read up to the closing single quote, where `''` is one quote.
fn read_single_quoted(rest: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if c != '\'' {
            out.push(c);
        } else if chars.peek().is_some_and(|(_, next)| *next == '\'') {
            chars.next();
            out.push('\'');
        } else {
            return Some((out, &rest[idx + 1..]));
        }
    }
    None
}

/// Read a one-line flow list of scalars: `[a, "b, c", 'd']`.
fn parse_flow_list(inline: &str) -> Option<Vec<String>> {
    let mut rest = inline.strip_prefix('[')?.trim_start();
    let mut items = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix(']') {
            return only_comment_follows(after).then_some(items);
        }
        let (value, after) = if let Some(quoted) = rest.strip_prefix('"') {
            read_double_quoted(quoted)?
        } else if let Some(quoted) = rest.strip_prefix('\'') {
            read_single_quoted(quoted)?
        } else {
            let end = rest.find([',', ']'])?;
            (rest[..end].trim().to_string(), &rest[end..])
        };
        items.push(value);
        rest = after.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after.trim_start();
        } else if !rest.starts_with(']') {
            return None;
        }
    }
}

fn parse_block_list(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(parse_scalar)
        .collect()
}

/// Read a block sequence of mappings as ordered `(key, value)` records.
fn parse_records(lines: &[&str]) -> Vec<Vec<(String, String)>> {
    let mut records: Vec<Vec<(String, String)>> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let field = if let Some(item) = trimmed.strip_prefix('-') {
            records.push(Vec::new());
            item.trim_start()
        } else {
            trimmed
        };
        if let (Some((key, inline)), Some(record)) = (split_key(field), records.last_mut()) {
            record.push((key.to_string(), parse_scalar(inline)));
        }
    }
    records
}

fn field(record: &[(String, String)], name: &str) -> Option<String> {
    record
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn parse_verified_block(lines: &[&str]) -> Vec<Verification> {
    parse_records(lines)
        .iter()
        .filter_map(|record| {
            Some(Verification {
                by: field(record, "by")?,
                at: field(record, "at")?,
            })
        })
        .collect()
}

fn parse_sources_block(lines: &[&str]) -> Vec<SourceCitation> {
    parse_records(lines)
        .iter()
        .filter_map(|record| {
            Some(SourceCitation {
                title: field(record, "title")?,
                resource: field(record, "resource")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_concept_page() {
        let text = "---\ntype: concept\ntitle: My Page\ndescription: A cool page\ntags: [\"tag1\", \"tag2\"]\nstatus: stable\nstale_after: 2026-12-31\n---\n\n# Body\n";
        let fm = parse_frontmatter(text).expect("parse ok").expect("some fm");
        assert_eq!(fm.page_type.as_deref(), Some("concept"));
        assert_eq!(fm.title.as_deref(), Some("My Page"));
        assert_eq!(fm.description.as_deref(), Some("A cool page"));
        assert_eq!(fm.tags, vec!["tag1", "tag2"]);
        assert_eq!(fm.status.as_deref(), Some("stable"));
        assert_eq!(fm.stale_after.as_deref(), Some("2026-12-31"));
    }

    #[test]
    fn test_parse_invalid_indentation() {
        let text = "---\n  bad_indent: true\n---\n";
        let err = parse_frontmatter(text).unwrap_err();
        assert_eq!(err.code(), "unsupported_line");
    }

    #[test]
    fn reader_and_layout_agree_on_a_block_scalar_with_a_blank_line() {
        let text = "---\ndescription: |\n  para one\n\n  para two\ntitle: x\n---\n";
        let layout = scan(text).expect("scan");
        let block = layout.block.expect("block");
        assert_eq!(
            &text[block.entries[0].start..block.entries[0].end],
            "description: |\n  para one\n\n  para two\n"
        );
        let fm = parse_frontmatter(text).expect("parse").expect("some");
        assert_eq!(fm.description.as_deref(), Some("para one para two"));
        assert_eq!(fm.title.as_deref(), Some("x"));
    }

    #[test]
    fn trailing_blank_and_comment_lines_stay_outside_the_value() {
        let text = "---\nsources:\n  - title: a\n\n# note\ntitle: x\n---\n";
        let block = scan(text).expect("scan").block.expect("block");
        assert_eq!(
            &text[block.entries[0].start..block.entries[0].end],
            "sources:\n  - title: a\n"
        );
    }

    #[test]
    fn indentless_sequence_belongs_to_the_key_above() {
        let text = "---\ntags:\n- a\n- \"b, c\"\ntitle: x\n---\n";
        let fm = parse_frontmatter(text).expect("parse").expect("some");
        assert_eq!(fm.tags, vec!["a", "b, c"]);
        assert_eq!(fm.title.as_deref(), Some("x"));
    }

    #[test]
    fn scalars_are_unescaped_and_comments_dropped() {
        assert_eq!(parse_scalar(r#""a \"b\": c\nd" # note"#), "a \"b\": c\nd");
        assert_eq!(parse_scalar("'it''s' # note"), "it's");
        assert_eq!(parse_scalar("plain value # note"), "plain value");
        assert_eq!(parse_scalar("\"unterminated"), "\"unterminated");
    }

    #[test]
    fn flow_lists_respect_quotes() {
        assert_eq!(
            parse_flow_list(r#"["a, b", 'c', d] # tail"#),
            Some(vec!["a, b".to_string(), "c".to_string(), "d".to_string()])
        );
        assert_eq!(parse_flow_list("[]"), Some(Vec::new()));
        assert_eq!(parse_flow_list("[a, b"), None);
    }

    #[test]
    fn unsafe_layouts_are_errors_not_missing_frontmatter() {
        let bom = String::from_utf8(vec![0xEF, 0xBB, 0xBF]).expect("utf8") + "---\na: b\n---\n";
        for (text, code) in [
            (bom.as_str(), "byte_order_mark"),
            ("---\rtype: x\r---\r", "carriage_return"),
            ("---\ntype: x\rmore\n---\n", "carriage_return"),
            ("---\ntype: x\n", "unclosed"),
            ("---", "unclosed"),
            ("--- \ntype: x\n---\n", "delimiter"),
            ("--- # comment\ntype: x\n---\n", "delimiter"),
            ("---\t%YAML\ntype: x\n---\n", "delimiter"),
            ("--- text", "delimiter"),
            ("---yaml\ntype: x\n---\n", "unsupported_line"),
            ("---:\n", "unsupported_line"),
            ("---x\ry\n", "carriage_return"),
            ("---\ntype: x\n--- \nbody\n---\n", "delimiter"),
            ("---\ntype: x\n...\n", "delimiter"),
            ("---\n\"quoted\": x\n---\n", "unsupported_line"),
            ("---\njust text\n---\n", "unsupported_line"),
            ("---\ntitle: x\n- stray\n---\n", "unsupported_line"),
        ] {
            let err = parse_frontmatter(text).expect_err(text);
            assert_eq!(err.code(), code, "{text:?}");
        }
    }

    #[test]
    fn a_page_without_a_block_is_not_an_error() {
        for text in [
            "",
            "# Title\n",
            "----\nrule\n",
            "---- x\n",
            "\n---\na: b\n---\n",
        ] {
            assert_eq!(parse_frontmatter(text), Ok(None), "{text:?}");
        }
    }
}
