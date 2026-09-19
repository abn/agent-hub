//! Line-oriented frontmatter patcher.
//!
//! A patch changes only the lines of the keys it names and leaves every other
//! byte of the page alone: comments, key order, blank lines, the quoting of
//! untouched values, LF or CRLF as found, a missing final newline as found.
//! The layout rules and the layout refusals are in [`super`].
//!
//! # Order
//!
//! Changes apply in the order given. A key that exists is replaced or deleted
//! in place. Keys that do not exist are appended at the end of the block in
//! the order given. Deleting an absent key does nothing. A patch that writes
//! nothing (no changes, or only deletions on a page with no block) returns
//! the page unchanged, whatever the page holds.
//!
//! A page with no block gets one: the delimiters, the written keys, and one
//! empty line before the body unless the body already starts with a line
//! ending. The body bytes are never trimmed.
//!
//! # Emitted values
//!
//! - A string is written plain only when that is safe; otherwise it is double
//!   quoted with `\\`, `\"`, `\n`, `\r`, `\t` and four-digit uppercase
//!   `\uXXXX` escapes for the remaining control characters, so no value can
//!   end its line or its quoting. Plain is refused for: the empty string;
//!   leading or trailing whitespace; a control character, U+2028, U+2029 or
//!   U+FEFF; a first character that is a YAML indicator or one of
//!   `` ~ < = + . ``; a leading digit unless the whole string is a date
//!   (`2026-09-19`) or an RFC 3339 timestamp; `: `, ` #` or a trailing `:`;
//!   the words `true false null yes no on off y n` in any case.
//! - A boolean is `true` or `false`; an integer is its decimal digits.
//! - A list of strings is a flow list with every item double quoted; the
//!   empty list is `[]`.
//! - A list of records is a block sequence of mappings at a two-space indent,
//!   fields in the order given.
//! - New lines take the line ending of the opening delimiter, or of the
//!   page's first line when there is no block, or LF.
//!
//! # What the patcher refuses
//!
//! Checked in this order: the changes as given (`invalid_key` for a key that
//! is not `[A-Za-z_][A-Za-z0-9_-]*`, `duplicate_change`, `invalid_value` for a
//! float, a nested mapping, a mixed list, an empty record, or an integer
//! beyond 53 bits), then the layout, then each change in order:
//!
//! - `duplicate_key`: the key appears more than once in the block.
//! - `anchor`: the key's inline value starts with `&` or `*`.
//! - `not_block_sequence`: review or promote would append under a key whose
//!   value is not a block sequence (a flow list, a scalar, a mapping).
//! - `indentation`: that block sequence is indented with tabs or unevenly.
//!
//! Review and promote append one record to `verified` and `sources` at the
//! indentation the sequence already uses, and never reorder or rewrite the
//! records that are there.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};

use super::{Entry, FrontmatterError, scan};

/// The largest integer magnitude a browser reads back exactly.
const INT_MAX: i64 = (1 << 53) - 1;

/// A scalar the patcher can write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scalar {
    Str(String),
    Bool(bool),
    Int(i64),
}

/// An ordered mapping written as one block sequence item.
pub type Record = Vec<(String, Scalar)>;

/// A value the patcher can write under a top-level key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchValue {
    Scalar(Scalar),
    /// A flow list of strings.
    List(Vec<String>),
    /// A block sequence of mappings.
    Records(Vec<Record>),
}

/// One change: a key, and the value to set or `None` to delete it.
pub type Change = (String, Option<PatchValue>);

impl From<&str> for PatchValue {
    fn from(value: &str) -> Self {
        Self::Scalar(Scalar::Str(value.to_string()))
    }
}

enum Op<'a> {
    Set(&'a str, &'a PatchValue),
    Delete(&'a str),
    Append(&'a str, &'a [(String, Scalar)]),
}

impl Op<'_> {
    fn key(&self) -> &str {
        match self {
            Self::Set(key, _) | Self::Delete(key) | Self::Append(key, _) => key,
        }
    }
}

/// Patch frontmatter keys in `text`, in the order given.
///
/// See the module documentation for the order, the emitted forms and the
/// refusals.
pub fn patch_frontmatter(text: &str, changes: &[Change]) -> Result<String, FrontmatterError> {
    let ops: Vec<Op<'_>> = changes
        .iter()
        .map(|(key, value)| match value {
            Some(value) => Op::Set(key, value),
            None => Op::Delete(key),
        })
        .collect();
    apply(text, &ops)
}

/// Record a review: append `{by, at}` to the `verified` block sequence,
/// creating the key or the block when absent.
pub fn review_frontmatter(
    text: &str,
    actor: &str,
    timestamp: &str,
) -> Result<String, FrontmatterError> {
    let record = vec![
        ("by".to_string(), Scalar::Str(actor.to_string())),
        ("at".to_string(), Scalar::Str(timestamp.to_string())),
    ];
    apply(text, &[Op::Append("verified", &record)])
}

/// Parameters for promoting a session entry to the knowledge base.
#[derive(Debug, Clone, Default)]
pub struct PromoteParams<'a> {
    pub page_type: Option<&'a str>,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub tags: Option<&'a [String]>,
    pub session_name: &'a str,
    pub session_id: &'a str,
    pub from_path: &'a str,
}

/// Patch frontmatter for promotion from a session brain to the knowledge base.
///
/// Sets `type`, `title`, `description` and `tags` when supplied, in that
/// order, then appends one `{title, resource}` citation to `sources`.
pub fn promote_frontmatter(
    text: &str,
    params: &PromoteParams<'_>,
) -> Result<String, FrontmatterError> {
    let scalars = [
        ("type", params.page_type),
        ("title", params.title),
        ("description", params.description),
    ];
    let mut values: Vec<(&str, PatchValue)> = scalars
        .iter()
        .filter_map(|(key, value)| Some((*key, PatchValue::from((*value)?))))
        .collect();
    if let Some(tags) = params.tags {
        values.push(("tags", PatchValue::List(tags.to_vec())));
    }

    let citation = vec![
        (
            "title".to_string(),
            Scalar::Str(format!(
                "{} brain {}",
                params.session_name, params.from_path
            )),
        ),
        (
            "resource".to_string(),
            Scalar::Str(format!(
                "agenthub://session/{}/brain{}",
                params.session_id, params.from_path
            )),
        ),
    ];

    let mut ops: Vec<Op<'_>> = values
        .iter()
        .map(|(key, value)| Op::Set(key, value))
        .collect();
    ops.push(Op::Append("sources", &citation));
    apply(text, &ops)
}

fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn invalid(key: &str, reason: &str) -> FrontmatterError {
    FrontmatterError::InvalidValue {
        key: key.to_string(),
        reason: reason.to_string(),
    }
}

fn check_scalar(key: &str, scalar: &Scalar) -> Result<(), FrontmatterError> {
    match scalar {
        Scalar::Int(n) if !(-INT_MAX..=INT_MAX).contains(n) => {
            Err(invalid(key, "an integer beyond 53 bits"))
        }
        _ => Ok(()),
    }
}

fn check_record(key: &str, record: &[(String, Scalar)]) -> Result<(), FrontmatterError> {
    if record.is_empty() {
        return Err(invalid(key, "an empty record"));
    }
    let mut seen = HashSet::new();
    for (field, scalar) in record {
        if !is_identifier(field) {
            return Err(invalid(key, "a record field that is not a plain key"));
        }
        if !seen.insert(field.as_str()) {
            return Err(invalid(key, "a record field named twice"));
        }
        check_scalar(key, scalar)?;
    }
    Ok(())
}

fn check_ops(ops: &[Op<'_>]) -> Result<(), FrontmatterError> {
    let mut seen = HashSet::new();
    for op in ops {
        let key = op.key();
        if !is_identifier(key) {
            return Err(FrontmatterError::InvalidKey(key.to_string()));
        }
        if !seen.insert(key) {
            return Err(FrontmatterError::DuplicateChange(key.to_string()));
        }
        match op {
            Op::Set(_, PatchValue::Scalar(scalar)) => check_scalar(key, scalar)?,
            Op::Set(_, PatchValue::List(_)) | Op::Delete(_) => {}
            Op::Set(_, PatchValue::Records(records)) => {
                if records.is_empty() {
                    return Err(invalid(key, "an empty list of records"));
                }
                for record in records {
                    check_record(key, record)?;
                }
            }
            Op::Append(_, record) => check_record(key, record)?,
        }
    }
    Ok(())
}

fn apply(text: &str, ops: &[Op<'_>]) -> Result<String, FrontmatterError> {
    if ops.is_empty() {
        return Ok(text.to_string());
    }
    check_ops(ops)?;

    let layout = scan(text)?;
    let nl = layout.newline;

    let Some(block) = layout.block else {
        let mut written = String::new();
        for op in ops {
            match op {
                Op::Set(key, value) => emit_entry(&mut written, key, value, nl),
                Op::Delete(_) => {}
                Op::Append(key, record) => {
                    written.push_str(key);
                    written.push(':');
                    written.push_str(nl);
                    emit_record(&mut written, 2, record, nl);
                }
            }
        }
        if written.is_empty() {
            return Ok(text.to_string());
        }
        let mut out = String::with_capacity(text.len() + written.len() + 12);
        out.push_str("---");
        out.push_str(nl);
        out.push_str(&written);
        out.push_str("---");
        out.push_str(nl);
        if !text.is_empty() && !text.starts_with(['\n', '\r']) {
            out.push_str(nl);
        }
        out.push_str(text);
        return Ok(out);
    };

    // First position and count per key. Looked up, never iterated.
    let mut index: HashMap<&str, (usize, usize)> = HashMap::new();
    for (position, entry) in block.entries.iter().enumerate() {
        index.entry(entry.key).or_insert((position, 0)).1 += 1;
    }

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut appended = String::new();
    for op in ops {
        let key = op.key();
        let found = match index.get(key) {
            Some((_, count)) if *count > 1 => {
                return Err(FrontmatterError::DuplicateKey(key.to_string()));
            }
            Some((position, _)) => Some(&block.entries[*position]),
            None => None,
        };
        if found.is_some_and(|entry| entry.inline.starts_with(['&', '*'])) {
            return Err(FrontmatterError::Anchor(key.to_string()));
        }
        match (op, found) {
            (Op::Set(_, value), Some(entry)) => {
                let mut replacement = String::new();
                emit_entry(&mut replacement, key, value, nl);
                edits.push((entry.start, entry.end, replacement));
            }
            (Op::Set(_, value), None) => emit_entry(&mut appended, key, value, nl),
            (Op::Delete(_), Some(entry)) => edits.push((entry.start, entry.end, String::new())),
            (Op::Delete(_), None) => {}
            (Op::Append(_, record), Some(entry)) => {
                let indent = sequence_indent(text, entry)?;
                let mut insertion = String::new();
                emit_record(&mut insertion, indent, record, nl);
                edits.push((entry.end, entry.end, insertion));
            }
            (Op::Append(_, record), None) => {
                appended.push_str(key);
                appended.push(':');
                appended.push_str(nl);
                emit_record(&mut appended, 2, record, nl);
            }
        }
    }
    edits.push((block.close_start, block.close_start, appended));
    edits.sort_by_key(|(start, end, _)| (*start, *end));

    let grown: usize = edits.iter().map(|(_, _, new)| new.len()).sum();
    let mut out = String::with_capacity(text.len() + grown);
    let mut cursor = 0;
    for (start, end, replacement) in &edits {
        out.push_str(&text[cursor..*start]);
        out.push_str(replacement);
        cursor = *end;
    }
    out.push_str(&text[cursor..]);
    Ok(out)
}

/// The indentation of the block sequence under `entry`, or a refusal when the
/// value is anything else. An empty value takes the two-space default.
fn sequence_indent(text: &str, entry: &Entry<'_>) -> Result<usize, FrontmatterError> {
    let key = entry.key;
    if !(entry.inline.is_empty() || entry.inline.starts_with('#')) {
        return Err(FrontmatterError::NotBlockSequence(key.to_string()));
    }
    let mut indent = None;
    for line in text[entry.line_end..entry.end].lines() {
        let content = line.trim_start_matches([' ', '\t']);
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let lead = &line[..line.len() - content.len()];
        if lead.contains('\t') {
            return Err(FrontmatterError::Indentation(key.to_string()));
        }
        let is_item = content == "-" || content.starts_with("- ");
        match indent {
            None if is_item => indent = Some(lead.len()),
            None => return Err(FrontmatterError::NotBlockSequence(key.to_string())),
            Some(width) if lead.len() < width => {
                return Err(FrontmatterError::Indentation(key.to_string()));
            }
            Some(width) if lead.len() == width && !is_item => {
                return Err(FrontmatterError::NotBlockSequence(key.to_string()));
            }
            Some(_) => {}
        }
    }
    Ok(indent.unwrap_or(2))
}

fn emit_entry(out: &mut String, key: &str, value: &PatchValue, nl: &str) {
    out.push_str(key);
    out.push(':');
    match value {
        PatchValue::Scalar(scalar) => {
            out.push(' ');
            emit_scalar(out, scalar);
            out.push_str(nl);
        }
        PatchValue::List(items) => {
            out.push_str(" [");
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                emit_double_quoted(out, item);
            }
            out.push(']');
            out.push_str(nl);
        }
        PatchValue::Records(records) => {
            out.push_str(nl);
            for record in records {
                emit_record(out, 2, record, nl);
            }
        }
    }
}

fn emit_record(out: &mut String, indent: usize, record: &[(String, Scalar)], nl: &str) {
    for (position, (field, scalar)) in record.iter().enumerate() {
        out.extend(std::iter::repeat_n(' ', indent));
        out.push_str(if position == 0 { "- " } else { "  " });
        out.push_str(field);
        out.push_str(": ");
        emit_scalar(out, scalar);
        out.push_str(nl);
    }
}

fn emit_scalar(out: &mut String, scalar: &Scalar) {
    match scalar {
        Scalar::Str(s) if is_plain_safe(s) => out.push_str(s),
        Scalar::Str(s) => emit_double_quoted(out, s),
        Scalar::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Scalar::Int(n) => out.push_str(&n.to_string()),
    }
}

/// Control characters and the line and mark characters YAML treats specially.
fn needs_escape(c: char) -> bool {
    let code = u32::from(c);
    code < 0x20 || (0x7F..=0x9F).contains(&code) || matches!(code, 0x2028 | 0x2029 | 0xFEFF)
}

fn emit_double_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if needs_escape(c) => {
                out.push('\\');
                out.push('u');
                out.push_str(&format!("{:04X}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

const RESERVED_WORDS: [&str; 9] = ["true", "false", "null", "yes", "no", "on", "off", "y", "n"];

fn is_plain_safe(s: &str) -> bool {
    let Some(first) = s.chars().next() else {
        return false;
    };
    if s.trim() != s || s.chars().any(needs_escape) {
        return false;
    }
    if matches!(
        first,
        '-' | '?'
            | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
            | '~'
            | '<'
            | '='
            | '+'
            | '.'
    ) {
        return false;
    }
    if first.is_ascii_digit() && !is_date_or_timestamp(s) {
        return false;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return false;
    }
    !RESERVED_WORDS
        .iter()
        .any(|word| s.eq_ignore_ascii_case(word))
}

/// Match `pattern` at the start of `bytes`, where `9` stands for any digit.
fn eat<'a>(bytes: &'a [u8], pattern: &str) -> Option<&'a [u8]> {
    let head = bytes.get(..pattern.len())?;
    head.iter()
        .zip(pattern.bytes())
        .all(|(byte, want)| match want {
            b'9' => byte.is_ascii_digit(),
            _ => *byte == want,
        })
        .then(|| &bytes[pattern.len()..])
}

/// `2026-09-19`, or an RFC 3339 timestamp with `Z` or a numeric offset.
fn is_date_or_timestamp(s: &str) -> bool {
    // The shape is not enough. A loader that reads the plain form as a date
    // fails on a thirteenth month, so only a real instant is written plain and
    // anything else that merely looks like one is quoted.
    let bytes = s.as_bytes();
    let Some(rest) = eat(bytes, "9999-99-99") else {
        return false;
    };
    let number = |range: std::ops::Range<usize>| -> u32 {
        bytes[range]
            .iter()
            .fold(0, |sum, digit| sum * 10 + u32::from(digit - b'0'))
    };
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > days {
        return false;
    }
    if rest.is_empty() {
        return true;
    }
    let Some(mut rest) = eat(rest, "T99:99:99") else {
        return false;
    };
    if number(11..13) > 23 || number(14..16) > 59 || number(17..19) > 59 {
        return false;
    }
    if let Some(fraction) = rest.strip_prefix(b".") {
        let digits = fraction.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        rest = &fraction[digits..];
    }
    if rest == b"Z" {
        return true;
    }
    let offset = eat(rest, "+99:99").or_else(|| eat(rest, "-99:99"));
    if !offset.is_some_and(<[u8]>::is_empty) {
        return false;
    }
    let at = bytes.len() - 5;
    number(at..at + 2) <= 23 && number(at + 3..at + 5) <= 59
}

fn value_error<E: de::Error>(reason: &str) -> E {
    E::custom(format!("invalid_value: {reason}"))
}

struct ScalarVisitor;

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = Scalar;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string, a boolean or an integer")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Scalar, E> {
        Ok(Scalar::Str(v.to_string()))
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Scalar, E> {
        Ok(Scalar::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Scalar, E> {
        Ok(Scalar::Int(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Scalar, E> {
        i64::try_from(v)
            .map(Scalar::Int)
            .map_err(|_| value_error("an integer beyond 53 bits"))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Scalar, E> {
        Err(value_error("a float"))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Scalar, E> {
        Err(value_error("a null inside a value"))
    }

    // A record field holds one scalar. A list or a mapping there is a value
    // that cannot be written, so it is refused as one rather than failing to
    // load, which a caller could not tell from a malformed request.
    fn visit_seq<A: de::SeqAccess<'de>>(self, _: A) -> Result<Scalar, A::Error> {
        Err(value_error("a list inside a record"))
    }

    fn visit_map<A: de::MapAccess<'de>>(self, _: A) -> Result<Scalar, A::Error> {
        Err(value_error("a mapping inside a record"))
    }
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ScalarVisitor)
    }
}

/// A list item: a string, or a record with its fields in document order.
enum Item {
    Str(String),
    Record(Record),
}

struct ItemVisitor;

impl<'de> Visitor<'de> for ItemVisitor {
    type Value = Item;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string or a mapping of scalars")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Item, E> {
        Ok(Item::Str(v.to_string()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Item, A::Error> {
        let mut record = Vec::new();
        while let Some(pair) = map.next_entry::<String, Scalar>()? {
            record.push(pair);
        }
        Ok(Item::Record(record))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Item, E> {
        Err(value_error("a list item that is not a string or a record"))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Item, E> {
        Err(value_error("a list item that is not a string or a record"))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Item, E> {
        Err(value_error("a list item that is not a string or a record"))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Item, E> {
        Err(value_error("a list item that is not a string or a record"))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Item, E> {
        Err(value_error("a null inside a value"))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, _: A) -> Result<Item, A::Error> {
        Err(value_error("a nested list"))
    }
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ItemVisitor)
    }
}

struct PatchValueVisitor;

impl<'de> Visitor<'de> for PatchValueVisitor {
    type Value = PatchValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a scalar, a list of strings or a list of records")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<PatchValue, E> {
        ScalarVisitor.visit_str(v).map(PatchValue::Scalar)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<PatchValue, E> {
        ScalarVisitor.visit_bool(v).map(PatchValue::Scalar)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<PatchValue, E> {
        ScalarVisitor.visit_i64(v).map(PatchValue::Scalar)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<PatchValue, E> {
        ScalarVisitor.visit_u64(v).map(PatchValue::Scalar)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<PatchValue, E> {
        ScalarVisitor.visit_f64(v).map(PatchValue::Scalar)
    }

    fn visit_map<A: MapAccess<'de>>(self, _: A) -> Result<PatchValue, A::Error> {
        Err(value_error("a mapping outside a list"))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PatchValue, A::Error> {
        let mut strings = Vec::new();
        let mut records = Vec::new();
        while let Some(item) = seq.next_element::<Item>()? {
            match item {
                Item::Str(s) => strings.push(s),
                Item::Record(r) => records.push(r),
            }
        }
        if !strings.is_empty() && !records.is_empty() {
            return Err(value_error("a list mixing strings and records"));
        }
        if records.is_empty() {
            Ok(PatchValue::List(strings))
        } else {
            Ok(PatchValue::Records(records))
        }
    }
}

/// Reads the JSON form the fixture corpus uses. Record fields keep their
/// document order. `null` is not a value: it is the deletion marker one level
/// up, in [`Change`].
impl<'de> Deserialize<'de> for PatchValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PatchValueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse_frontmatter;
    use super::*;

    const AT: &str = "2026-09-19T17:00:00Z";

    fn set(key: &str, value: &str) -> Change {
        (key.to_string(), Some(PatchValue::from(value)))
    }

    fn delete(key: &str) -> Change {
        (key.to_string(), None)
    }

    fn bom_page() -> String {
        let mut page = String::from_utf8(vec![0xEF, 0xBB, 0xBF]).expect("utf8");
        page.push_str("---\ntype: concept\n---\n\nBody\n");
        page
    }

    #[test]
    fn empty_patch_is_identity() {
        let bom = bom_page();
        for text in [
            "",
            "\n\n# Title\n",
            "# Title",
            "---\ntype: concept\n---\n",
            "---\ntype: concept\n",
            "---\rtype: concept\r---\r",
            bom.as_str(),
        ] {
            assert_eq!(patch_frontmatter(text, &[]).as_deref(), Ok(text));
        }
    }

    #[test]
    fn a_patch_that_writes_nothing_adds_no_block() {
        let text = "\n\n# Title\n";
        let changes = [delete("status"), delete("title")];
        assert_eq!(patch_frontmatter(text, &changes).as_deref(), Ok(text));
    }

    #[test]
    fn body_bytes_are_never_trimmed() {
        let out = patch_frontmatter("\r\n\n# Title\n", &[set("type", "concept")]).expect("patch");
        assert_eq!(out, "---\r\ntype: concept\r\n---\r\n\r\n\n# Title\n");
    }

    #[test]
    fn bom_is_refused_not_prepended() {
        let text = bom_page();
        assert_eq!(
            review_frontmatter(&text, "human", AT),
            Err(FrontmatterError::ByteOrderMark)
        );
    }

    #[test]
    fn trailing_space_closer_is_refused_not_scanned_past() {
        let text = "---\ntype: concept\n--- \n\nProse.\n\n---\n\nMore.\n";
        assert_eq!(
            patch_frontmatter(text, &[set("status", "stable")]),
            Err(FrontmatterError::Delimiter { line: 3 })
        );
    }

    #[test]
    fn carriage_return_only_and_unclosed_pages_are_refused() {
        let change = [set("status", "stable")];
        assert_eq!(
            patch_frontmatter("---\rtype: concept\r---\r", &change),
            Err(FrontmatterError::CarriageReturn { line: 1 })
        );
        assert_eq!(
            patch_frontmatter("---\ntype: concept\n", &change),
            Err(FrontmatterError::Unclosed)
        );
    }

    #[test]
    fn flow_verified_is_refused_not_appended_under() {
        for value in ["[]", "yes", "[{by: a}]"] {
            let text = format!("---\ntype: concept\nverified: {value}\n---\n");
            assert_eq!(
                review_frontmatter(&text, "human", AT),
                Err(FrontmatterError::NotBlockSequence("verified".to_string())),
                "{value}"
            );
        }
        assert_eq!(
            review_frontmatter("---\nverified: *anchor\n---\n", "human", AT),
            Err(FrontmatterError::Anchor("verified".to_string()))
        );
    }

    #[test]
    fn tab_indented_verified_is_refused_not_mixed() {
        let text = "---\nverified:\n\t- by: a\n\t  at: 2026-01-01T00:00:00Z\n---\n";
        assert_eq!(
            review_frontmatter(text, "human", AT),
            Err(FrontmatterError::Indentation("verified".to_string()))
        );
    }

    #[test]
    fn duplicate_key_is_refused() {
        let text = "---\ntitle: one\ntitle: two\n---\n";
        assert_eq!(
            patch_frontmatter(text, &[set("title", "three")]),
            Err(FrontmatterError::DuplicateKey("title".to_string()))
        );
        assert_eq!(
            patch_frontmatter(text, &[delete("title")]),
            Err(FrontmatterError::DuplicateKey("title".to_string()))
        );
    }

    #[test]
    fn a_refusal_is_an_invalid_argument_that_says_why() {
        let err = crate::Error::from(FrontmatterError::ByteOrderMark);
        assert_eq!(err.code(), crate::error::ErrorCode::InvalidArgument);
        assert_eq!(
            err.to_string(),
            "frontmatter refused: the page starts with a byte order mark"
        );
    }

    #[test]
    fn review_actor_cannot_inject_keys() {
        let text = "---\ntype: concept\nstatus: draft\n---\n";
        let actor = "human\n    at: 1999-01-01T00:00:00Z\nstatus: stable\ninjected:\n  - by: x";
        let out = review_frontmatter(text, actor, AT).expect("review");
        assert_eq!(out.lines().count(), text.lines().count() + 3, "{out:?}");
        let fm = parse_frontmatter(&out).expect("parse").expect("some");
        assert_eq!(fm.status.as_deref(), Some("draft"));
        assert!(fm.custom_keys.is_empty(), "{:?}", fm.custom_keys);
        assert_eq!(fm.verified.len(), 1);
        assert_eq!(fm.verified[0].by, actor);
        assert_eq!(fm.verified[0].at, AT);
    }

    #[test]
    fn promote_title_and_description_cannot_break_quoting() {
        let params = PromoteParams {
            page_type: Some("concept"),
            title: Some("he said \"x\": y"),
            description: Some("line1\nstatus: deprecated"),
            tags: None,
            session_name: "s \"q\" #1",
            session_id: "sess_1",
            from_path: "/fs/n.md",
        };
        let out = promote_frontmatter("# n\n", &params).expect("promote");
        let fm = parse_frontmatter(&out).expect("parse").expect("some");
        assert_eq!(fm.title.as_deref(), params.title);
        assert_eq!(fm.description.as_deref(), params.description);
        assert_eq!(fm.status, None);
        assert_eq!(fm.sources.len(), 1);
        assert_eq!(fm.sources[0].title, "s \"q\" #1 brain /fs/n.md");
    }

    #[test]
    fn promote_writes_the_same_citation_with_and_without_a_block() {
        let params = PromoteParams {
            session_name: "alpha",
            session_id: "sess_1",
            from_path: "/fs/n.md",
            ..Default::default()
        };
        let citation = "sources:\n  - title: alpha brain /fs/n.md\n    resource: agenthub://session/sess_1/brain/fs/n.md\n";
        assert_eq!(
            promote_frontmatter("# n\n", &params).expect("promote"),
            format!("---\n{citation}---\n\n# n\n")
        );
        assert_eq!(
            promote_frontmatter("---\ntype: concept\n---\n# n\n", &params).expect("promote"),
            format!("---\ntype: concept\n{citation}---\n# n\n")
        );
    }

    #[test]
    fn multi_key_order_is_the_order_given() {
        let text = "---\ntype: concept\n---\n";
        let changes: Vec<Change> = ["delta", "alpha", "charlie", "bravo"]
            .iter()
            .map(|key| set(key, "v"))
            .collect();
        for _ in 0..50 {
            assert_eq!(
                patch_frontmatter(text, &changes).expect("patch"),
                "---\ntype: concept\ndelta: v\nalpha: v\ncharlie: v\nbravo: v\n---\n"
            );
        }
        assert_eq!(
            patch_frontmatter("", &changes).expect("patch"),
            "---\ndelta: v\nalpha: v\ncharlie: v\nbravo: v\n---\n"
        );
    }

    #[test]
    fn replacing_block_scalar_with_blank_line_takes_whole_value() {
        let text = "---\ndescription: |\n  para one\n\n  para two\ntitle: x\n---\n";
        assert_eq!(
            patch_frontmatter(text, &[set("description", "short")]).expect("patch"),
            "---\ndescription: short\ntitle: x\n---\n"
        );
    }

    #[test]
    fn deleting_block_scalar_with_blank_line_takes_whole_value() {
        let text = "---\ndescription: |\n  para one\n\n  para two\ntitle: x\n---\n";
        assert_eq!(
            patch_frontmatter(text, &[delete("description")]).expect("patch"),
            "---\ntitle: x\n---\n"
        );
    }

    #[test]
    fn values_that_cannot_be_written_are_refused_not_dropped() {
        let text = "---\ntype: concept\n---\n";
        for value in [
            PatchValue::Records(Vec::new()),
            PatchValue::Records(vec![Vec::new()]),
            PatchValue::Records(vec![vec![("bad key".to_string(), Scalar::Bool(true))]]),
            PatchValue::Scalar(Scalar::Int(1 << 53)),
        ] {
            let err =
                patch_frontmatter(text, &[("k".to_string(), Some(value))]).expect_err("refused");
            assert_eq!(err.code(), "invalid_value");
        }
        for json in ["1.5", "{\"a\": \"b\"}", "[\"a\", 1]", "[[\"a\"]]", "[null]"] {
            let err = serde_json::from_str::<PatchValue>(json).expect_err(json);
            assert!(err.to_string().contains("invalid_value"), "{json}: {err}");
        }
    }

    #[test]
    fn record_fields_keep_their_document_order() {
        let value: PatchValue =
            serde_json::from_str(r#"[{"title": "t", "resource": "r", "again": 1}]"#).expect("load");
        let out = patch_frontmatter("", &[("sources".to_string(), Some(value))]).expect("patch");
        assert_eq!(
            out,
            "---\nsources:\n  - title: t\n    resource: r\n    again: 1\n---\n"
        );
    }

    #[test]
    fn plain_scalars_are_the_safe_ones_only() {
        for plain in [
            "concept",
            "Simple Page",
            "2026-12-31",
            "2026-09-19T17:00:00Z",
            "2026-09-19T17:00:00.5-05:00",
            "agenthub://session/s/brain/fs/n.md",
            "a#b",
            "say \"hi\"",
        ] {
            assert!(is_plain_safe(plain), "{plain:?}");
        }
        for quoted in [
            "",
            " x",
            "x ",
            "- x",
            "-x",
            "a: b",
            "a #b",
            "x:",
            "#x",
            "&x",
            "*x",
            "!x",
            "|",
            ">",
            "'x",
            "\"x",
            "%x",
            "@x",
            "`x",
            "[x",
            "{x",
            "?x",
            ",x",
            "~",
            "<<",
            "=",
            "+1",
            ".5",
            "123",
            "1e3",
            "0x10",
            "2026-12-31x",
            "2026-09-19T17:00:00",
            "true",
            "NO",
            "Null",
            "y",
            "a\nb",
            "a\tb",
            "a\rb",
            "nul\0",
        ] {
            assert!(!is_plain_safe(quoted), "{quoted:?}");
        }
    }
}
