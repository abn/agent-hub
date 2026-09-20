# Frontmatter corpus

The contract for every implementation of the line-oriented frontmatter
patcher. The Rust reference is `src/okf/frontmatter.rs` (layout and reader)
and `src/okf/frontmatter/patch.rs` (patcher); their module documentation
states the layout rules, the emitted forms and the refusals in full. An
implementation passes when every case here produces the expected bytes, or the
expected refusal, exactly.

The `.md` files are compared byte for byte. Trailing spaces, CRLF and mixed
endings, a byte order mark and a missing final newline are the cases: never
let an editor or a formatter touch them.

## Manifest

`manifest.json` holds `version` (currently `2`) and `cases`. Each case has:

| Field | Meaning |
|---|---|
| `id` | Unique name, also the file name stem. |
| `description` | What the case pins. |
| `input` | File holding the page before the operation. |
| `patch` | File holding the operation, described below. |
| `expected` | File holding the page after the operation. |
| `error` | The refusal code, instead of `expected`. |

Exactly one of `expected` and `error` is present. A runner fails on a case it
cannot load, on an operation it does not know, and on a file in this directory
that no case names. It never skips.

## Operations

The patch file is a JSON object whose `operation` selects one of three.

`patch` is the operation a second implementation must provide:

```json
{ "operation": "patch", "changes": [["status", "stable"], ["old_key", null]] }
```

`changes` is an ordered list of `[key, value]` pairs, not an object, because
the order is part of the contract: existing keys change in place, absent keys
are appended to the end of the block in the order given. A `null` value
deletes the key. A value is a string, a boolean, an integer, a list of strings
(written as a flow list), or a list of objects (written as a block sequence of
mappings). Object fields are written in the order they appear in the JSON
text, so read them with a parser that keeps that order. A field name is held
to the same rule as a key, so an integer-like name such as `"1"`, the one kind
a JavaScript object moves ahead of the fields written before it, is refused
with `invalid_value` and never reordered.

A record field holds one scalar: a list or a mapping there is refused with
`invalid_value`. The corpus never repeats a field name inside one record: a
JavaScript reader keeps the last and cannot see the repeat, so do not add
such a case.

`review` appends `{by, at}` to the `verified` sequence:

```json
{ "operation": "review", "actor": "human", "time": "2026-09-19T17:00:00Z" }
```

`promote` sets `type`, `title`, `description` and `tags` when present, in that
order, then appends one citation to `sources` with the title
`<session_name> brain <from_path>` and the resource
`agenthub://session/<session_id>/brain<from_path>`:

```json
{
  "operation": "promote",
  "type": "concept",
  "title": "Notes",
  "session_name": "session-alpha",
  "session_id": "sess_01j5a",
  "from_path": "/fs/notes.md"
}
```

The hub runs `review` and `promote` on the server only. A runner for an
implementation that provides `patch` alone says so, reports how many cases of
the other two it left out, and still fails on an operation name outside these
three.

## Refusal codes

| Code | Refused because |
|---|---|
| `invalid_key` | A patch key is not `[A-Za-z_][A-Za-z0-9_-]*`. |
| `duplicate_change` | The patch names one key twice. |
| `invalid_value` | A number with a fractional part, a mapping outside a list, a mixed or nested list, a null inside a value, an empty record, or an integer beyond 53 bits. |
| `byte_order_mark` | The page starts with a byte order mark. |
| `carriage_return` | A carriage return without a line feed on the opening line or inside the block. |
| `delimiter` | The first line is `---` followed by whitespace, with or without a comment or text after it, or a line inside the block is exactly `...`, or is `---` or `...` followed by a space or a tab. |
| `unsupported_line` | The first line has anything but whitespace or a fourth dash straight after `---` (`---yaml`), or a column-zero line in the block is neither a comment nor a plain `key:` line, or an indented line comes before the first key. |
| `unclosed` | The block opens and never closes. |
| `duplicate_key` | A key the operation names appears more than once in the block. |
| `anchor` | A key the operation names has an inline value starting with `&` or `*`. |
| `not_block_sequence` | `review` or `promote` would append under a value that is not a block sequence. |
| `indentation` | That block sequence is indented with tabs or unevenly. |

The order of the table is the order of the checks: the changes as given, then
the page from its first line down, then each change in the order given. The
first failure is the refusal. One step comes before all of it: a value whose
shape cannot be written (a fraction, a mapping outside a list, a mixed or
nested list, a null inside a value) is refused wherever it sits in the list,
before any key is looked at. An integer beyond 53 bits, an empty record and a
bad field name are refused with their own change, after its key. An empty `changes` list is never refused: it
returns the page unchanged whatever the page holds.

## The first line

Only a first line that is exactly `---` opens a block. A first line that does
not start with `---`, or that starts with four or more dashes (a rule or a
paragraph in the body), means the page has no block, and a patch gives it one. Every other first line that starts with `---` is refused, in
this order: `carriage_return` when it holds one, `unclosed` when it is `---`
and the page ends there, `delimiter` when whitespace follows the dashes,
`unsupported_line` otherwise. A YAML reader sees a block under `--- # comment`,
so writing a second block above it is never right.

## Adding a case

Write the expected bytes by hand from the rules, not by running an
implementation and saving what it printed. Add the three files and the
manifest entry; JSON files end in a newline.

## Numbers and dates

The rule for a number is about its value, not how the JSON spelled it: an
integer from -9007199254740991 to 9007199254740991 is written as it is, and
anything else is refused. The corpus never writes an integer as `1.0`, `1e2`
or `-0`, because a JavaScript reader cannot tell those from `1`, `100` and `0`
while other readers can; do not add such a case. For the same reason it holds
no integer from 2^63 - 512 up to 2^63 - 1 beside a change that is refused for
another reason: a JavaScript reader rounds those to 2^63.

A string is written plain when it is a real calendar date (`2024-02-29`) or a
real instant (`2026-01-01T23:59:59Z`, with an optional fraction and a `Z` or
`+hh:mm` offset). A string that only has that shape (`2026-13-45`,
`2026-02-30`, an hour of 25) is quoted, so a loader is never asked to make a
date out of it.

## Whitespace

Blank, and every trim the rules speak of, mean the Unicode White_Space set.
That is not what every language's own trim removes: U+0085 is whitespace and
U+FEFF is not. Indentation is only ever a space or a tab.
