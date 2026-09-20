---
type: Guide
title: Project knowledge base
description: Read, write, review and promote project knowledge base pages over REST and the agent tools, with every route, its response and its refusals.
tags: [usage, knowledge-base, rest, mcp, okf]
status: draft
---

# Project knowledge base

Every project has one knowledge base: a tree of markdown pages under `/fs/`
that outlives every session. Agents write it with the brain tools and
`store: "project"`; the human reads, edits, reviews and promotes over the REST
routes below. Both surfaces share one write path, so a page gets the same path
rules, the same size limit, the same log row, the same search row and the same
feed signal whoever wrote it. The decision behind the store is in
[the project knowledge base](../adr/0018-project-knowledge-base.md), and the
agent tools are described in the served skill document, `GET /SKILL.md`.

## Paths

A page path is `/fs/<directories>/<name>`. In a URL the leading slash is
dropped and the namespace may be left out, so `pages/svc/caddy.md` and
`pages/fs/svc/caddy.md` both name `/fs/svc/caddy.md`. A first component of `kv`
is the key-value namespace, which a knowledge base does not have.

Every path is made canonical before anything keys on it: `.`, `..`, an empty
component and a trailing slash resolve the way a filesystem resolves them, and
`..` never climbs above `/fs`. The store, the write log, the search corpus and
the link graph all use the canonical path, so several spellings name one page
and a response always carries the canonical one.

These are refused with `invalid_argument`, identically over REST and the agent
tools: a `/kv` path, a control character (NUL, a line break and a tab
included), a backslash, and a canonical path over 512 bytes.

## Limits

| Limit | Value |
|---|---|
| One page | 1 MiB |
| One page path | 512 bytes |
| One JSON request body | 4 MiB |
| One knowledge base file | 1 GiB; a write past 256 MiB carries a warning |
| One history page | 200 rows |
| One listing | 500 entries |

A session brain value keeps its own 4 MiB limit. The page limit applies only
to the knowledge base.

## Refusals

Every refusal is an `application/problem+json` document with the hub error
`code`. The admin gate runs before a request's body is read, so a
caller without the admin token gets 401 whatever body it sent (a query string
that cannot be read is still a 400 first), and an agent token
is not the admin token.

| Status | `code` | When |
|---|---|---|
| 400 | `invalid_argument` | a refused path, a JSON body that does not parse or carries a field the route does not know, a frontmatter value containing a control character, a review or a promote of a page whose frontmatter cannot be patched safely (a first line such as `--- # comment` or `---yaml`, which starts a block for some readers and not for the hub), `verified_by` naming anyone but the human, a history `limit` over 200, deleting a directory that still holds pages |
| 401 | `unauthenticated` | no admin token, on every route |
| 404 | `not_found` | the project, the page, the session or the session brain entry does not exist |
| 409 | `conflict` | `if_version` does not match; the detail ends `current_version=sha256:...`, or `current_version=absent` |
| 413 | `payload_too_large` | a page over 1 MiB or a body over its limit, refused before it is buffered whole; nothing is written |
| 415 | `invalid_argument` | a page sent as anything but JSON, `text/markdown` or `text/plain` |

## Routes

### `GET /api/v1/projects/{id}/kb/pages`

Query: `prefix`, `limit`, `meta`. Returns `{entries[], truncated}`. `truncated`
is true when the listing held more than `limit` entries (500 at most).

Without `meta` the listing is one directory level. Each entry is
`{path, type, size_bytes, children}` where `type` is `file` or `dir`,
`children` is the number of entries directly inside a directory and `null` on
a file, and a directory's `size_bytes` is the sum of the files directly inside
it.

With `meta=1` the listing is every page and directory under the prefix, in
path order, so a tree, a directory view and the needs-review queue each cost
one request. A directory row is the same four fields. A page row adds `title`,
`description`, `page_type`, `status`, `tags`, `trust`, `verified_by`,
`verified_at`, `stale`, `stale_after`, `last_write_by` and `last_write_at`.
The frontmatter fields are `null` when the page has no frontmatter the hub's
reader can read.

`trust` is derived, never stored: `unverified` with no `verified` entry,
`human_reviewed` when the newest `verified.by` is `human`, `machine_confirmed`
when it is anything else, and `edited_since_review` when the page no longer
holds the bytes that were verified.

The write log decides that, not a clock. When a write lands, the hub notes on
its log row whether it brought in the page's newest verification: the page it
stores ends its `verified` block with an entry the page it replaces did not
hold, stamped within five minutes of the write landing. A verification is
about the bytes its author saw, so an older entry, taken out and put back
over an edited body or copied onto another page, was made for other bytes and
brings nothing in. A review always does. The page is `edited_since_review` when the version
of its newest write is not the version that write stored. So a page an agent
writes with a `verified` block of its own is not an edit since it, whatever
second it lands in; a retry that stores the same bytes is no edit; an edit in
the same second as the review is one; putting the verified bytes back reads as
verified again, until a later write brings in a verification of its own; and
taking the newest entry away to show an older one is an
edit. A page whose log holds no such row, written by a hub from before the
rows carried the note, is judged as it was then: edited when its newest write
is later than the newest `verified.at`, to the second.

### `GET /api/v1/projects/{id}/kb/pages/{path}`

Returns `{path, content, version, size_bytes, last_write: {actor, at}}`.
`version` is the token the next conditional write passes back. `last_write` is
the newest row for the path in the write log, however old.

### `PUT /api/v1/projects/{id}/kb/pages/{path}`

Body: `{content, if_version?}` as `application/json`, or the page itself as
`text/markdown` or `text/plain` with the guard in `?if_version=`. A body sent
as JSON that does not parse is a 400 and is never stored as the page.
`if_version` is a version token, or `absent` to create a page only when
nothing is there. Without it the last writer wins.

Returns `{ok, path, version, size_bytes, lint[], warnings[]}`. `lint` is
advisory and never fails a write: `missing_frontmatter`, `missing_type`,
`unparsed_frontmatter`, `okf_version_misplaced`, `link_escapes_bundle`, and
`broken_link` for a link whose target is not a page anywhere in the tree. A
page with no block at all is `missing_frontmatter`; a page whose first line
only looks like the opener (`--- # comment`, `---yaml`) or whose block cannot
be read safely is `unparsed_frontmatter`, and a review or a promote of it is
refused until the first line is exactly `---`. The hub stores the bytes as
sent.

### `DELETE /api/v1/projects/{id}/kb/pages/{path}`

Query: `if_version`. Returns `{ok, path}`. Deleting a page that does not exist
is the same 404 a read of it gives: nothing is logged, nothing is signalled and
no knowledge base file is created. A directory can be deleted once it is
empty. A delete appends one `kb_deleted` signal to the project feed.

### `POST /api/v1/projects/{id}/kb/pages/{path}/review`

Body, all optional: `{if_version?, verified_by?}`. The review appends
`{by: human, at: <now>}` to the page's `verified` block and changes no other
byte. The reviewer is set by the hub: `verified_by` is accepted only as the
literal `human` and is otherwise a 400. `if_version` is the version the human
read; when the page changed since, the review is a 409 and nothing is stamped.
Returns `{ok, path, version}`, logs one `kb.review` row with the actor `human`,
and appends one `kb_reviewed` signal.

### `POST /api/v1/projects/{id}/kb/promote`

Body: `{from_session_id, from_path, to_path, type?, title?, description?,
tags?, if_version?}`. Copies a session brain entry into the knowledge base,
patches the frontmatter with the fields given and adds a `sources` entry
`{title: "<session name> brain <from_path>", resource:
"agenthub://session/<session_id>/brain<from_path>"}`. The source entry is left
as it was. This is the same code path as the `brain_promote` tool, so the page
is byte for byte the one an agent's promote writes. Returns
`{ok, path, version, lint[]}`, logs one `kb.promote` row and appends one
`kb_promoted` signal.

### `GET /api/v1/projects/{id}/kb/history`

Query: `path`, `prefix`, `actor`, `op`, `before`, `limit`. Returns
`{rows[], total, next_before, truncated}`, newest first. A row is
`{op, path, actor, at, version}` where `op` is `kb.put`, `kb.delete`,
`kb.review` or `kb.promote`. `prefix` is a directory. `limit` defaults to 50
and is at most 200; `limit=0` returns the count alone. `next_before` is the
cursor for the next page and `truncated` is true whenever the filter matches
rows the response does not carry.

History answers who and when, never what changed: the hub keeps no earlier
content. The log is bounded only by the knowledge base file's own 1 GiB limit
and every request scans all of it, so `total` is the real count and a page
never loses its history or its last writer by being old.

### `GET /api/v1/projects/{id}/kb/backlinks`

Query: `path`. Returns `[{path, title}]`, the pages that link to the page, in
path order. A page that links to itself is not listed, and such a link does
not save the page from `orphan_page` either.

A link inside code is not a link, here or in lint. Code is a fenced block (a
fence closes only on a line of the same character, at least as long, with
nothing after it, so a longer fence holds a shorter one), a block indented four
spaces or a tab after a blank line outside a list, and a code span of any
number of backticks. Inside a list an indented line is read as prose, so an
indented code block nested in a list item still counts; fence it instead.

### `GET /api/v1/projects/{id}/kb/lint`

Query: `fresh`. Returns `{checked_at, findings[]}` for the whole tree: the
per-write codes plus `missing_index`, `missing_log`, `orphan_page` and
`missing_index_entry`. A finding is `{code, path, message, line?, meta?}`;
`meta` is the page's `meta=1` row when the finding names a page that exists.
`checked_at` is when the tree was walked, not when it was asked for. `fresh=1`
walks again.

### `GET /api/v1/projects/{id}/kb/stats`

Returns `{pages, bytes, unverified, stale, last_change_at, last_change_by,
needs_review: {unverified, machine_confirmed, edited_since_review, total}}`.

## Derived numbers

The `meta=1` listing, backlinks, lint and stats come from one walk of the
tree, kept per project. Every write, from either surface, invalidates it, so a
read straight after a write is current. An entry is also dropped after ten
seconds.

## Agent tools

`brain_get`, `brain_put`, `brain_list` and `brain_delete` reach the knowledge
base with `store: "project"`, under the project's own read and write grants.
`brain_promote(from_path, to_path, project_id?, type?, title?, description?,
tags?, if_version?)` reads `from_path` from the caller's active session brain
and returns `{ok, path, version, lint[]}`. A `brain_delete` of a page that does
not exist is `not_found`.

## What there is not

There is no move or rename. A move is a read, a write with
`if_version: "absent"` and a delete, by the client. It drops the page's
history, which is keyed by path, and links to the old path are not rewritten:
`backlinks` names them first. Wiki links (`[[page]]`) are not links to the hub;
only markdown links are followed.

Offline reading for the knowledge base is to be evaluated. Today what is cached
by the service worker is the application shell and its static assets; page content
is not cached, and reading pages requires an active connection to the hub.
