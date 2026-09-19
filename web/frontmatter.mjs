// Frontmatter reader and patcher for knowledge base pages, run in the client.
//
// The hub patches frontmatter on the server (src/okf/frontmatter.rs and
// src/okf/frontmatter/patch.rs). This module is the same algorithm in the
// browser: one layout scan shared by the reader and the patcher, one emitter,
// the same refusals with the same codes. The fixture corpus under
// tests/fixtures/frontmatter is the contract both are held to, byte for byte.
// There is no YAML parser here, and no DOM, fetch or import.
//
// Everything returned is a plain string taken from the page or built for it.
// Nothing is escaped for display: a title can hold markup, and escaping it
// before it reaches the document is the caller's job.
//
// A patch changes only the lines of the keys it names. Comments, key order,
// blank lines, the quoting of untouched values, LF or CRLF as found and a
// missing final newline as found all stay. The work is linear in the size of
// the page and of the change list.
//
// What is refused, as a FrontmatterError whose `code` is one of CODES. Checks
// run in this order and the first failure wins:
//
// 1. Every value must have a shape that can be written, else `invalid_value`:
//    a number that is not an integer, or an integer at or beyond 2^63; a
//    mapping outside a list; a list that mixes strings and records, or holds
//    anything else; a null, a list or a mapping inside a record; `undefined`
//    or any other type; a string that holds a lone surrogate.
// 2. Each change in the order given: `invalid_key` unless the key is
//    `[A-Za-z_][A-Za-z0-9_-]*`; `duplicate_change` when the key was already
//    named; `invalid_value` for an integer beyond 53 bits, an empty record, or
//    a record field that is not a plain key. Integer-like field names such as
//    "1" are therefore refused, which is also what keeps the field order of a
//    JavaScript object equal to the order it was written in.
// 3. The page, from its first line down: `byte_order_mark`,
//    `carriage_return`, `delimiter`, `unsupported_line`, `unclosed`.
// 4. Each change in order against the block: `duplicate_key`, `anchor`, and
//    for review and promote `not_block_sequence` and `indentation`.
//
// An empty change list returns the page unchanged whatever the page holds. A
// change list that is not a list of `[key, value]` pairs is a caller bug and
// throws a TypeError, not a refusal.
//
// Where JavaScript differs from the server:
//
// - Strings are UTF-16. Every character the layout rules look at is ASCII or
//   in the basic plane, so offsets are code units and astral characters pass
//   through whole. A lone surrogate cannot be sent to the server as UTF-8. In
//   the page it is left exactly where it is and the scan ignores it; in a
//   value to be written it is refused as `invalid_value`.
// - Whitespace means the Unicode White_Space set the server trims, which is
//   not what String.prototype.trim removes (that one takes U+FEFF and leaves
//   U+0085), so trimming is done here by hand.
// - A byte order mark is refused only when it is still there. Response.text()
//   drops one from the start of a body before this module can see it; a page
//   read out of a JSON field keeps it.
// - Numbers are judged by value. -0 is 0. An integer is written only within
//   Number.MAX_SAFE_INTEGER either side of zero.

export const CODES = Object.freeze([
  "invalid_key",
  "duplicate_change",
  "invalid_value",
  "byte_order_mark",
  "carriage_return",
  "delimiter",
  "unsupported_line",
  "unclosed",
  "duplicate_key",
  "anchor",
  "not_block_sequence",
  "indentation",
]);

export class FrontmatterError extends Error {
  constructor(code, message, detail = {}) {
    super(message);
    this.name = "FrontmatterError";
    this.code = code;
    if (detail.line !== undefined) this.line = detail.line;
    if (detail.key !== undefined) this.key = detail.key;
  }
}

function refuseLine(code, line, message) {
  return new FrontmatterError(code, `line ${line} ${message}`, { line });
}

function refuseKey(code, key, message) {
  return new FrontmatterError(code, message, { key });
}

function invalidValue(key, reason) {
  return refuseKey(
    "invalid_value",
    key,
    `the value for '${key}' cannot be written: ${reason}`,
  );
}

// ---------------------------------------------------------------------------
// Characters

const TAB = 0x09;
const LF = 0x0a;
const CR = 0x0d;
const SPACE = 0x20;

// The Unicode White_Space property, all of it in the basic plane.
function isWhitespace(code) {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function trimStart(s) {
  let from = 0;
  while (from < s.length && isWhitespace(s.charCodeAt(from))) from += 1;
  return from === 0 ? s : s.slice(from);
}

function trimEnd(s) {
  let to = s.length;
  while (to > 0 && isWhitespace(s.charCodeAt(to - 1))) to -= 1;
  return to === s.length ? s : s.slice(0, to);
}

function trim(s) {
  return trimEnd(trimStart(s));
}

function isBlank(s) {
  for (let i = 0; i < s.length; i += 1) {
    if (!isWhitespace(s.charCodeAt(i))) return false;
  }
  return true;
}

function isSpaceOrTab(code) {
  return code === SPACE || code === TAB;
}

function startsWithSpaceOrTab(s) {
  return s.length > 0 && isSpaceOrTab(s.charCodeAt(0));
}

function isDigit(code) {
  return code >= 0x30 && code <= 0x39;
}

function isAsciiLetter(code) {
  return (code >= 0x41 && code <= 0x5a) || (code >= 0x61 && code <= 0x7a);
}

function isWellFormed(s) {
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    if (code < 0xd800 || code > 0xdfff) continue;
    const next = i + 1 < s.length ? s.charCodeAt(i + 1) : 0;
    if (code > 0xdbff || next < 0xdc00 || next > 0xdfff) return false;
    i += 1;
  }
  return true;
}

// Split the way the server splits a value's lines: at LF, dropping one CR
// before it, with no empty last line after a final LF.
function splitLines(s) {
  const lines = [];
  let pos = 0;
  while (pos < s.length) {
    const idx = s.indexOf("\n", pos);
    if (idx === -1) {
      lines.push(s.slice(pos));
      break;
    }
    const end = idx > pos && s.charCodeAt(idx - 1) === CR ? idx - 1 : idx;
    lines.push(s.slice(pos, end));
    pos = idx + 1;
  }
  return lines;
}

// ---------------------------------------------------------------------------
// Layout: the one scan the reader and the patcher share

// The line at `pos`: its content without the line ending, the offset of the
// next line, and whether a line feed ended it.
function lineAt(text, pos) {
  const idx = text.indexOf("\n", pos);
  if (idx === -1) {
    return { content: text.slice(pos), next: text.length, terminated: false };
  }
  const end = idx > pos && text.charCodeAt(idx - 1) === CR ? idx - 1 : idx;
  return { content: text.slice(pos, end), next: idx + 1, terminated: true };
}

// Whether `content` is `marker` followed by nothing or by a space or a tab.
function isMarkerLine(content, marker) {
  if (!content.startsWith(marker)) return false;
  return (
    content.length === marker.length ||
    isSpaceOrTab(content.charCodeAt(marker.length))
  );
}

const NOT_A_PLAIN_KEY_START = "\"'?[]{}&*!|>%@`,";

// Split a column-zero line into its key and inline value, or null.
function splitKey(content) {
  let from = 0;
  for (;;) {
    const idx = content.indexOf(":", from);
    if (idx === -1) return null;
    const last = idx + 1 === content.length;
    if (last || isSpaceOrTab(content.charCodeAt(idx + 1))) {
      const key = trimEnd(content.slice(0, idx));
      const plain =
        key.length > 0 && !NOT_A_PLAIN_KEY_START.includes(key.charAt(0));
      if (!plain || key.includes(" #") || key.includes("\t#")) return null;
      return { key, inline: trim(content.slice(idx + 1)) };
    }
    from = idx + 1;
  }
}

// Lay a page out, or refuse it. Returns the line ending for new lines and the
// block, or a null block when the page has none. Each entry carries `start`,
// `lineEnd` (just past the key line) and `end` (just past the value).
function scan(text) {
  if (text.charCodeAt(0) === 0xfeff) {
    throw new FrontmatterError(
      "byte_order_mark",
      "the page starts with a byte order mark",
    );
  }

  const opening = lineAt(text, 0);
  const first = opening.content;
  if (!(first === "---" && opening.terminated)) {
    if (first.startsWith("---")) {
      const tail = first.slice(3);
      let lead = 0;
      while (lead < tail.length && isSpaceOrTab(tail.charCodeAt(lead))) {
        lead += 1;
      }
      if (tail.charCodeAt(lead) === CR) {
        throw refuseLine(
          "carriage_return",
          1,
          "has a carriage return without a line feed",
        );
      }
      if (tail === "") {
        throw new FrontmatterError(
          "unclosed",
          "the frontmatter block opens and never closes",
        );
      }
      if (isBlank(tail)) {
        throw refuseLine(
          "delimiter",
          1,
          "is a frontmatter delimiter that is not exactly '---'",
        );
      }
    }
    const idx = text.indexOf("\n");
    const crlf = idx > 0 && text.charCodeAt(idx - 1) === CR;
    return { newline: crlf ? "\r\n" : "\n", block: null };
  }

  const openEnd = opening.next;
  const newline =
    openEnd >= 2 && text.charCodeAt(openEnd - 2) === CR ? "\r\n" : "\n";

  const entries = [];
  let pos = openEnd;
  let line = 1;
  while (pos < text.length) {
    line += 1;
    const { content, next } = lineAt(text, pos);
    if (content.includes("\r")) {
      throw refuseLine(
        "carriage_return",
        line,
        "has a carriage return without a line feed",
      );
    }
    if (content === "---") {
      return {
        newline,
        block: { openEnd, closeStart: pos, closeEnd: next, entries },
      };
    }
    if (isMarkerLine(content, "---") || isMarkerLine(content, "...")) {
      throw refuseLine(
        "delimiter",
        line,
        "is a frontmatter delimiter that is not exactly '---'",
      );
    }

    const last = entries.length > 0 ? entries[entries.length - 1] : null;
    if (isBlank(content) || content.startsWith("#")) {
      // Blank and comment lines join a value only if an indented line
      // follows, which moves `end` past them.
    } else if (startsWithSpaceOrTab(content)) {
      if (last === null) throw unsupportedLine(line);
      last.end = next;
    } else if (isMarkerLine(content, "-")) {
      if (last === null || !(last.inline === "" || last.inline.startsWith("#"))) {
        throw unsupportedLine(line);
      }
      last.end = next;
    } else {
      const split = splitKey(content);
      if (split === null) throw unsupportedLine(line);
      entries.push({
        key: split.key,
        inline: split.inline,
        start: pos,
        lineEnd: next,
        end: next,
      });
    }
    pos = next;
  }

  throw new FrontmatterError(
    "unclosed",
    "the frontmatter block opens and never closes",
  );
}

function unsupportedLine(line) {
  return refuseLine(
    "unsupported_line",
    line,
    "of the frontmatter is neither a comment nor a plain 'key:' line",
  );
}

// ---------------------------------------------------------------------------
// Reader

// `|` or `>` with optional chomping and indentation indicators.
function isBlockScalarHeader(inline) {
  let end = 0;
  while (end < inline.length && !isSpaceOrTab(inline.charCodeAt(end))) end += 1;
  if (end === 0) return false;
  const first = inline.charAt(0);
  if (first !== "|" && first !== ">") return false;
  for (let i = 1; i < end; i += 1) {
    const code = inline.charCodeAt(i);
    if (!(code === 0x2b || code === 0x2d || isDigit(code))) return false;
  }
  return true;
}

// Join a first fragment and the non-blank continuation lines with spaces.
function foldLines(first, lines) {
  let out = first;
  for (const line of lines) {
    const piece = trim(line);
    if (piece === "") continue;
    if (out !== "") out += " ";
    out += piece;
  }
  return out;
}

function onlyCommentFollows(rest) {
  const tail = trimStart(rest);
  return tail === "" || tail.startsWith("#");
}

function hexValue(code) {
  if (isDigit(code)) return code - 0x30;
  if (code >= 0x41 && code <= 0x46) return code - 0x41 + 10;
  if (code >= 0x61 && code <= 0x66) return code - 0x61 + 10;
  return -1;
}

const SIMPLE_ESCAPES = new Map([
  ["n", "\n"],
  ["r", "\r"],
  ["t", "\t"],
  ["0", "\0"],
  ["a", "\x07"],
  ["b", "\b"],
  ["e", "\x1b"],
  ["f", "\f"],
  ["v", "\v"],
]);

// Read from `from` (just past the opening quote) to the closing double
// quote. Returns the value and the offset just past the quote, or null.
function readDoubleQuoted(s, from) {
  let out = "";
  let run = from;
  let i = from;
  while (i < s.length) {
    const c = s.charAt(i);
    if (c === '"') {
      return { value: out + s.slice(run, i), after: i + 1 };
    }
    if (c !== "\\") {
      i += 1;
      continue;
    }
    out += s.slice(run, i);
    i += 1;
    if (i >= s.length) return null;
    const escape = s.charAt(i);
    i += 1;
    if (SIMPLE_ESCAPES.has(escape)) {
      out += SIMPLE_ESCAPES.get(escape);
    } else if (escape === "x" || escape === "u" || escape === "U") {
      const width = escape === "x" ? 2 : escape === "u" ? 4 : 8;
      let code = 0;
      for (let n = 0; n < width; n += 1) {
        if (i >= s.length) return null;
        const digit = hexValue(s.charCodeAt(i));
        if (digit < 0) return null;
        code = code * 16 + digit;
        i += 1;
      }
      // A surrogate is not a character and neither is anything past the
      // last plane; String.fromCodePoint would accept the first and throw
      // on the second.
      if (code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) return null;
      out += String.fromCodePoint(code);
    } else {
      // Any other escaped character stands for itself. An astral one is two
      // code units: the second follows as ordinary text.
      out += escape;
    }
    run = i;
  }
  return null;
}

// Read to the closing single quote, where `''` is one quote.
function readSingleQuoted(s, from) {
  let out = "";
  let run = from;
  let i = from;
  while (i < s.length) {
    if (s.charAt(i) !== "'") {
      i += 1;
      continue;
    }
    if (s.charAt(i + 1) === "'") {
      out += s.slice(run, i + 1);
      i += 2;
      run = i;
      continue;
    }
    return { value: out + s.slice(run, i), after: i + 1 };
  }
  return null;
}

// Read one scalar: double quoted with escapes, single quoted, or plain with
// an optional trailing comment. A quoted scalar that does not close cleanly
// is returned as written.
function parseScalar(input) {
  const raw = trim(input);
  if (raw.startsWith('"') || raw.startsWith("'")) {
    const read = raw.startsWith('"')
      ? readDoubleQuoted(raw, 1)
      : readSingleQuoted(raw, 1);
    if (read !== null && onlyCommentFollows(raw.slice(read.after))) {
      return read.value;
    }
    return raw;
  }
  let end = raw.length;
  for (const marker of [" #", "\t#"]) {
    const idx = raw.indexOf(marker);
    if (idx !== -1 && idx < end) end = idx;
  }
  return trimEnd(raw.slice(0, end));
}

function skipWhitespace(s, from) {
  let i = from;
  while (i < s.length && isWhitespace(s.charCodeAt(i))) i += 1;
  return i;
}

// Read a one-line flow list of scalars: `[a, "b, c", 'd']`. Null when it is
// anything else.
function parseFlowList(inline) {
  if (!inline.startsWith("[")) return null;
  const items = [];
  let pos = skipWhitespace(inline, 1);
  for (;;) {
    const c = inline.charAt(pos);
    if (c === "]") {
      return onlyCommentFollows(inline.slice(pos + 1)) ? items : null;
    }
    if (c === '"' || c === "'") {
      const read =
        c === '"'
          ? readDoubleQuoted(inline, pos + 1)
          : readSingleQuoted(inline, pos + 1);
      if (read === null) return null;
      items.push(read.value);
      pos = read.after;
    } else {
      let end = pos;
      while (end < inline.length) {
        const at = inline.charAt(end);
        if (at === "," || at === "]") break;
        end += 1;
      }
      if (end >= inline.length) return null;
      items.push(trim(inline.slice(pos, end)));
      pos = end;
    }
    pos = skipWhitespace(inline, pos);
    if (inline.charAt(pos) === ",") {
      pos = skipWhitespace(inline, pos + 1);
    } else if (inline.charAt(pos) !== "]") {
      return null;
    }
  }
}

function parseBlockList(lines) {
  const items = [];
  for (const line of lines) {
    const trimmed = trim(line);
    if (trimmed.startsWith("- ")) items.push(parseScalar(trimmed.slice(2)));
  }
  return items;
}

// Read a block sequence of mappings as ordered `[field, value]` records.
function parseRecords(lines) {
  const records = [];
  for (const line of lines) {
    const trimmed = trim(line);
    if (trimmed === "" || trimmed.startsWith("#")) continue;
    let field = trimmed;
    if (trimmed.startsWith("-")) {
      records.push([]);
      field = trimStart(trimmed.slice(1));
    }
    const split = splitKey(field);
    if (split !== null && records.length > 0) {
      records[records.length - 1].push([split.key, parseScalar(split.inline)]);
    }
  }
  return records;
}

// The records that carry both fields; when a field repeats, the last wins.
function recordsWith(lines, a, b) {
  const out = [];
  for (const record of parseRecords(lines)) {
    const found = new Map();
    for (const [field, value] of record) found.set(field, value);
    if (found.has(a) && found.has(b)) {
      out.push({ [a]: found.get(a), [b]: found.get(b) });
    }
  }
  return out;
}

const SCALAR_KEYS = new Set([
  "type",
  "title",
  "description",
  "status",
  "stale_after",
  "okf_version",
]);

// Read a page's frontmatter with the restricted line-oriented reader, or
// throw the layout's refusal.
//
// `raw` is the text between the delimiters, or null when the page has no
// block, and `body` is everything after the closing delimiter line (the
// whole page when there is no block). `fields` lists every top-level key in
// block order as `{ key, value, text }`: `text` is the exact text of the
// key's lines, and `value` is a string, a list of strings for `tags`, or a
// list of records for `verified` and `sources`. The typed keys the editor
// form uses are also set by name; when a key repeats, the last one wins.
// `custom` holds every other key as `[key, string]` pairs.
export function read(text) {
  requireString(text, "text");
  const layout = scan(text);
  const out = {
    raw: null,
    body: text,
    fields: [],
    type: null,
    title: null,
    description: null,
    status: null,
    tags: [],
    stale_after: null,
    okf_version: null,
    verified: [],
    sources: [],
    custom: [],
  };
  const block = layout.block;
  if (block === null) return out;

  out.raw = text.slice(block.openEnd, block.closeStart);
  out.body = text.slice(block.closeEnd);

  const scalar = (key, value) => {
    if (SCALAR_KEYS.has(key)) out[key] = value;
    else out.custom.push([key, value]);
    return value;
  };

  for (const entry of block.entries) {
    const { key, inline } = entry;
    const lines = splitLines(text.slice(entry.lineEnd, entry.end));
    let value;
    if (inline === "" || inline.startsWith("#")) {
      if (key === "verified") {
        value = out.verified = recordsWith(lines, "by", "at");
      } else if (key === "sources") {
        value = out.sources = recordsWith(lines, "title", "resource");
      } else if (key === "tags") {
        value = out.tags = parseBlockList(lines);
      } else {
        value = lines.join("\n");
        out.custom.push([key, value]);
      }
    } else if (isBlockScalarHeader(inline)) {
      value = scalar(key, foldLines("", lines));
    } else if (inline.startsWith("[")) {
      const items = parseFlowList(inline);
      if (items !== null && key === "tags") {
        value = out.tags = items;
      } else {
        value = inline;
        out.custom.push([key, inline]);
      }
    } else {
      value = scalar(key, parseScalar(foldLines(inline, lines)));
    }
    out.fields.push({ key, value, text: text.slice(entry.start, entry.end) });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Emitter

// Control characters and the line and mark characters YAML treats specially.
function needsEscape(code) {
  return (
    code < 0x20 ||
    (code >= 0x7f && code <= 0x9f) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0xfeff
  );
}

function emitDoubleQuoted(s) {
  let out = '"';
  let run = 0;
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    let escaped;
    if (code === 0x5c || code === 0x22) escaped = "\\" + s.charAt(i);
    else if (code === LF) escaped = "\\n";
    else if (code === CR) escaped = "\\r";
    else if (code === TAB) escaped = "\\t";
    else if (needsEscape(code)) {
      escaped = "\\u" + code.toString(16).toUpperCase().padStart(4, "0");
    } else continue;
    out += s.slice(run, i) + escaped;
    run = i + 1;
  }
  return out + s.slice(run) + '"';
}

const RESERVED_WORDS = new Set([
  "true",
  "false",
  "null",
  "yes",
  "no",
  "on",
  "off",
  "y",
  "n",
]);

const NOT_A_PLAIN_VALUE_START = "-?:,[]{}#&*!|>'\"%@`~<=+.";

function isReservedWord(s) {
  if (s.length > 5) return false;
  let lowered = "";
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    lowered += String.fromCharCode(
      code >= 0x41 && code <= 0x5a ? code + 0x20 : code,
    );
  }
  return RESERVED_WORDS.has(lowered);
}

function isPlainSafe(s) {
  if (s === "") return false;
  if (
    isWhitespace(s.charCodeAt(0)) ||
    isWhitespace(s.charCodeAt(s.length - 1))
  ) {
    return false;
  }
  for (let i = 0; i < s.length; i += 1) {
    if (needsEscape(s.charCodeAt(i))) return false;
  }
  if (NOT_A_PLAIN_VALUE_START.includes(s.charAt(0))) return false;
  if (isDigit(s.charCodeAt(0)) && !isDateOrTimestamp(s)) return false;
  if (s.includes(": ") || s.includes(" #") || s.endsWith(":")) return false;
  return !isReservedWord(s);
}

// Match `pattern` at `from`, where `9` stands for any digit. Returns the
// offset just past it, or -1.
function eat(s, from, pattern) {
  if (from + pattern.length > s.length) return -1;
  for (let i = 0; i < pattern.length; i += 1) {
    const want = pattern.charCodeAt(i);
    const have = s.charCodeAt(from + i);
    if (want === 0x39 ? !isDigit(have) : have !== want) return -1;
  }
  return from + pattern.length;
}

function digits(s, from, to) {
  let sum = 0;
  for (let i = from; i < to; i += 1) sum = sum * 10 + (s.charCodeAt(i) - 0x30);
  return sum;
}

// `2026-09-19`, or an RFC 3339 timestamp with `Z` or a numeric offset. The
// shape is not enough: a loader that reads the plain form as a date fails on
// a thirteenth month, so only a real instant is written plain.
function isDateOrTimestamp(s) {
  let rest = eat(s, 0, "9999-99-99");
  if (rest < 0) return false;
  const year = digits(s, 0, 4);
  const month = digits(s, 5, 7);
  const day = digits(s, 8, 10);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  let days;
  if ([1, 3, 5, 7, 8, 10, 12].includes(month)) days = 31;
  else if ([4, 6, 9, 11].includes(month)) days = 30;
  else if (month === 2) days = leap ? 29 : 28;
  else return false;
  if (day === 0 || day > days) return false;
  if (rest === s.length) return true;

  rest = eat(s, rest, "T99:99:99");
  if (rest < 0) return false;
  if (digits(s, 11, 13) > 23 || digits(s, 14, 16) > 59 || digits(s, 17, 19) > 59) {
    return false;
  }
  if (s.charAt(rest) === ".") {
    let end = rest + 1;
    while (end < s.length && isDigit(s.charCodeAt(end))) end += 1;
    if (end === rest + 1) return false;
    rest = end;
  }
  if (s.slice(rest) === "Z") return true;
  let offset = eat(s, rest, "+99:99");
  if (offset < 0) offset = eat(s, rest, "-99:99");
  if (offset !== s.length) return false;
  const at = s.length - 5;
  return digits(s, at, at + 2) <= 23 && digits(s, at + 3, at + 5) <= 59;
}

function emitScalar(scalar) {
  if (typeof scalar === "string") {
    return isPlainSafe(scalar) ? scalar : emitDoubleQuoted(scalar);
  }
  if (typeof scalar === "boolean") return scalar ? "true" : "false";
  // A safe integer prints as plain decimal digits; -0 prints as 0.
  return String(scalar);
}

function emitRecord(indent, record, nl) {
  let out = "";
  record.forEach(([field, scalar], position) => {
    out += " ".repeat(indent) + (position === 0 ? "- " : "  ");
    out += field + ": " + emitScalar(scalar) + nl;
  });
  return out;
}

function emitEntry(key, value, nl) {
  if (value.kind === "scalar") return `${key}: ${emitScalar(value.scalar)}${nl}`;
  if (value.kind === "list") {
    return `${key}: [${value.items.map(emitDoubleQuoted).join(", ")}]${nl}`;
  }
  let out = key + ":" + nl;
  for (const record of value.records) out += emitRecord(2, record, nl);
  return out;
}

function emitAppended(key, record, nl) {
  return key + ":" + nl + emitRecord(2, record, nl);
}

// ---------------------------------------------------------------------------
// Values and checks

const INT_MAX = Number.MAX_SAFE_INTEGER;
const TWO_TO_63 = 2 ** 63;

function requireString(value, name) {
  if (typeof value !== "string") {
    throw new TypeError(`${name} must be a string`);
  }
}

function isMapping(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// The shape check for one scalar. An integer within 64 bits is let through
// here and held to 53 bits later, with its change, as the server does.
function shapeScalar(key, value) {
  if (typeof value === "string") {
    if (!isWellFormed(value)) throw invalidValue(key, "a lone surrogate");
    return value;
  }
  if (typeof value === "boolean") return value;
  if (typeof value === "number") {
    if (!Number.isInteger(value)) throw invalidValue(key, "a float");
    if (Math.abs(value) >= TWO_TO_63) {
      throw invalidValue(key, "an integer beyond 53 bits");
    }
    return value;
  }
  if (value === null) throw invalidValue(key, "a null inside a value");
  throw invalidValue(key, "a value that is not a string, a boolean or an integer");
}

function shapeValue(key, value) {
  if (isMapping(value)) throw invalidValue(key, "a mapping outside a list");
  if (!Array.isArray(value)) {
    return { kind: "scalar", scalar: shapeScalar(key, value) };
  }
  const items = [];
  const records = [];
  for (const item of value) {
    if (typeof item === "string") {
      if (!isWellFormed(item)) throw invalidValue(key, "a lone surrogate");
      items.push(item);
    } else if (isMapping(item)) {
      records.push(
        Object.keys(item).map((field) => [field, shapeScalar(key, item[field])]),
      );
    } else if (item === null) {
      throw invalidValue(key, "a null inside a value");
    } else if (Array.isArray(item)) {
      throw invalidValue(key, "a nested list");
    } else {
      throw invalidValue(key, "a list item that is not a string or a record");
    }
  }
  if (items.length > 0 && records.length > 0) {
    throw invalidValue(key, "a list mixing strings and records");
  }
  return records.length === 0 ? { kind: "list", items } : { kind: "records", records };
}

function isIdentifier(key) {
  if (key.length === 0) return false;
  for (let i = 0; i < key.length; i += 1) {
    const code = key.charCodeAt(i);
    if (isAsciiLetter(code) || code === 0x5f) continue;
    if (i > 0 && (isDigit(code) || code === 0x2d)) continue;
    return false;
  }
  return true;
}

function checkScalar(key, scalar) {
  if (typeof scalar === "number" && (scalar < -INT_MAX || scalar > INT_MAX)) {
    throw invalidValue(key, "an integer beyond 53 bits");
  }
}

function checkRecord(key, record) {
  if (record.length === 0) throw invalidValue(key, "an empty record");
  const seen = new Set();
  for (const [field, scalar] of record) {
    if (!isIdentifier(field)) {
      throw invalidValue(key, "a record field that is not a plain key");
    }
    if (seen.has(field)) throw invalidValue(key, "a record field named twice");
    seen.add(field);
    checkScalar(key, scalar);
  }
}

function checkOps(ops) {
  const seen = new Set();
  for (const op of ops) {
    const key = op.key;
    if (!isIdentifier(key)) {
      throw refuseKey("invalid_key", key, `'${key}' is not a plain frontmatter key`);
    }
    if (seen.has(key)) {
      throw refuseKey("duplicate_change", key, `the patch names '${key}' twice`);
    }
    seen.add(key);
    if (op.kind === "append") {
      checkRecord(key, op.record);
    } else if (op.kind === "set") {
      if (op.value.kind === "scalar") checkScalar(key, op.value.scalar);
      if (op.value.kind === "records") {
        for (const record of op.value.records) checkRecord(key, record);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Patcher

// The indentation of the block sequence under `entry`, or a refusal when the
// value is anything else. An empty value takes the two-space default.
function sequenceIndent(text, entry) {
  const key = entry.key;
  const notSequence = () =>
    refuseKey("not_block_sequence", key, `key '${key}' does not hold a block sequence`);
  const indentation = () =>
    refuseKey(
      "indentation",
      key,
      `the block sequence under '${key}' is indented with tabs or unevenly`,
    );

  if (!(entry.inline === "" || entry.inline.startsWith("#"))) throw notSequence();
  let indent = null;
  for (const line of splitLines(text.slice(entry.lineEnd, entry.end))) {
    let lead = 0;
    while (lead < line.length && isSpaceOrTab(line.charCodeAt(lead))) lead += 1;
    const content = line.slice(lead);
    if (content === "" || content.startsWith("#")) continue;
    if (line.slice(0, lead).includes("\t")) throw indentation();
    const isItem = content === "-" || content.startsWith("- ");
    if (indent === null) {
      if (!isItem) throw notSequence();
      indent = lead;
    } else if (lead < indent) {
      throw indentation();
    } else if (lead === indent && !isItem) {
      throw notSequence();
    }
  }
  return indent === null ? 2 : indent;
}

function apply(text, ops) {
  if (ops.length === 0) return text;
  checkOps(ops);

  const { newline: nl, block } = scan(text);

  if (block === null) {
    let written = "";
    for (const op of ops) {
      if (op.kind === "set") written += emitEntry(op.key, op.value, nl);
      else if (op.kind === "append") written += emitAppended(op.key, op.record, nl);
    }
    if (written === "") return text;
    const first = text.charCodeAt(0);
    const separator = text !== "" && first !== LF && first !== CR ? nl : "";
    return "---" + nl + written + "---" + nl + separator + text;
  }

  // First position and count per key.
  const index = new Map();
  block.entries.forEach((entry, position) => {
    const found = index.get(entry.key);
    if (found === undefined) index.set(entry.key, { position, count: 1 });
    else found.count += 1;
  });

  const edits = [];
  let appended = "";
  for (const op of ops) {
    const key = op.key;
    const found = index.get(key);
    if (found !== undefined && found.count > 1) {
      throw refuseKey("duplicate_key", key, `key '${key}' appears more than once`);
    }
    const entry = found === undefined ? null : block.entries[found.position];
    if (entry !== null && (entry.inline.startsWith("&") || entry.inline.startsWith("*"))) {
      throw refuseKey("anchor", key, `key '${key}' carries an anchor or an alias`);
    }
    if (op.kind === "set") {
      const written = emitEntry(key, op.value, nl);
      if (entry === null) appended += written;
      else edits.push({ start: entry.start, end: entry.end, text: written });
    } else if (op.kind === "delete") {
      if (entry !== null) edits.push({ start: entry.start, end: entry.end, text: "" });
    } else if (entry === null) {
      appended += emitAppended(key, op.record, nl);
    } else {
      const indent = sequenceIndent(text, entry);
      edits.push({
        start: entry.end,
        end: entry.end,
        text: emitRecord(indent, op.record, nl),
      });
    }
  }
  edits.push({ start: block.closeStart, end: block.closeStart, text: appended });
  // Array.prototype.sort is stable, which the order of two insertions at one
  // offset depends on.
  edits.sort((a, b) => a.start - b.start || a.end - b.end);

  const parts = [];
  let cursor = 0;
  for (const edit of edits) {
    parts.push(text.slice(cursor, edit.start), edit.text);
    cursor = edit.end;
  }
  parts.push(text.slice(cursor));
  return parts.join("");
}

// Patch frontmatter keys in `text`. `changes` is an ordered list of
// `[key, value]` pairs applied in the order given: a key that exists changes
// in place, an absent key is appended to the end of the block, and a `null`
// value deletes the key. A value is a string, a boolean, an integer, a list
// of strings (written as a flow list) or a list of plain objects (written as
// a block sequence of mappings, fields in the order the object holds them).
// `undefined` is refused, never read as a deletion.
export function patch(text, changes) {
  requireString(text, "text");
  if (!Array.isArray(changes)) {
    throw new TypeError("changes must be a list of [key, value] pairs");
  }
  for (const change of changes) {
    if (!Array.isArray(change) || change.length !== 2 || typeof change[0] !== "string") {
      throw new TypeError("a change must be a [key, value] pair with a string key");
    }
  }
  const ops = changes.map(([key, value]) =>
    value === null
      ? { kind: "delete", key }
      : { kind: "set", key, value: shapeValue(key, value) },
  );
  return apply(text, ops);
}

// Record a review: append `{by, at}` to the `verified` block sequence,
// creating the key or the block when absent. The hub runs this on the
// server; it is here so both sides answer the same corpus.
export function review(text, actor, time) {
  requireString(text, "text");
  requireString(actor, "actor");
  requireString(time, "time");
  const record = [
    ["by", shapeScalar("verified", actor)],
    ["at", shapeScalar("verified", time)],
  ];
  return apply(text, [{ kind: "append", key: "verified", record }]);
}

// Patch frontmatter for a promotion from a session brain: set `type`,
// `title`, `description` and `tags` when given, in that order, then append
// one `{title, resource}` citation to `sources`. Server-side in the hub, as
// `review` is.
export function promote(text, params) {
  requireString(text, "text");
  if (!isMapping(params)) throw new TypeError("params must be an object");
  for (const name of ["session_name", "session_id", "from_path"]) {
    requireString(params[name], name);
  }
  const ops = [];
  for (const key of ["type", "title", "description"]) {
    const value = params[key];
    if (value === null || value === undefined) continue;
    requireString(value, key);
    ops.push({ kind: "set", key, value: shapeValue(key, value) });
  }
  if (params.tags !== null && params.tags !== undefined) {
    const tags = params.tags;
    if (!Array.isArray(tags) || tags.some((tag) => typeof tag !== "string")) {
      throw new TypeError("tags must be a list of strings");
    }
    ops.push({ kind: "set", key: "tags", value: shapeValue("tags", tags) });
  }
  const { session_name: name, session_id: id, from_path: path } = params;
  const record = [
    ["title", shapeScalar("sources", `${name} brain ${path}`)],
    ["resource", shapeScalar("sources", `agenthub://session/${id}/brain${path}`)],
  ];
  ops.push({ kind: "append", key: "sources", record });
  return apply(text, ops);
}
