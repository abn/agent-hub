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
agent tools are described in the served bootstrap document, `GET /bootstrap/SKILL.md`.

## The shape of a page

A page is markdown with a YAML frontmatter block. The hub reads it, and the
wiki shows it as the page's type, status and tags rather than as body text.
These are the keys the reader knows:

| Key | Value |
|---|---|
| `type` | what the page is: `Concept`, `Guide`, `Runbook`, `Reference`, `Decision`, `Decision Record`. The one field a concept page needs, and the only one lint requires |
| `title` | what a listing shows, and the page's backlink label |
| `description` | one line on what a reader gets here |
| `status` | `draft` or `stable` |
| `tags` | free-form; a flow list (`[usage, ops]`) or one `- item` a line |
| `stale_after` | a `YYYY-MM-DD` date after which the page reads as stale |
| `okf_version` | the bundle format, and only `/fs/index.md` may carry it |
| `verified` | `{by, at}` entries the human's review appends |
| `sources` | `{title, resource}` entries a promote adds |

Any other top-level key is kept as a custom key and ignored, so a page can
carry fields of its own. The block opens when the page's first line is exactly
`---`, never `--- # comment` or `---yaml`, and holds plain `key: value` lines;
a `|` or `>` block scalar is read as one folded line.

```
---
type: Runbook
title: Deploy the hub
description: How the hub is deployed on the node
status: draft
tags: [usage, ops]
stale_after: 2027-01-01
---
# Deploy the hub
```

A write is lenient. A page with no block is stored as it was sent, and the
result carries a `missing_frontmatter` finding that names the minimum to add,
so nothing is lost to a missing field and no page is silently untyped.

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
| One history page | 200 rows, for the write log and for one page's versions |
| One listing | 500 entries |
| One folder export or import | 10,000 pages and 256 MiB of pages |

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
| 400 | `invalid_argument` | a refused path, a JSON body that does not parse or carries a field the route does not know, a frontmatter value containing a control character, a review or a promote of a page whose frontmatter cannot be patched safely (a first line such as `--- # comment` or `---yaml`, which starts a block for some readers and not for the hub), `verified_by` naming anyone but the human, a history `limit` over 200, a revert body that does not parse, deleting a directory that still holds pages |
| 401 | `unauthenticated` | no admin token, on every route |
| 404 | `not_found` | the project, the page, the session or the session brain entry does not exist; a version the page's history does not name, or whose bytes were not kept or were forgotten; a history to forget for a path the log never named |
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

Query: `version`. Returns `{path, content, version, size_bytes, last_write:
{actor, at}}`. `version` is the token the next conditional write passes back.
`last_write` is the newest row for the path in the write log, however old.

With `version`, the response is that version of the page instead,
`{path, content, version, size_bytes, current, at, actor}`, where `current`
says whether it is what the page holds now and `at` and `actor` are the newest
write of those bytes. The version must be one the page's own history
names, and a deleted page can be read this way. A version whose bytes were not
kept, or whose history was forgotten, is `not_found`.

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
page with no block at all is `missing_frontmatter`, and its message carries
the minimum to add (a block opening with `---` that sets `type`, or
`okf_version` on the root); a page whose first line only looks like the opener
(`--- # comment`, `---yaml`) or whose block cannot be read safely is
`unparsed_frontmatter`, and a review or a promote of it is refused until the
first line is exactly `---`. The hub stores the bytes as sent.

### `DELETE /api/v1/projects/{id}/kb/pages/{path}`

Query: `if_version`. Returns `{ok, path}`. Deleting a page that does not exist
is the same 404 a read of it gives: nothing is logged, nothing is signalled and
no knowledge base file is created. A directory can be deleted once it is
empty. A delete appends one `kb_deleted` signal to the project feed. The page's
bytes stay in its history, where a revert restores them and anyone who can read
the project can read them, until its history is forgotten with
`DELETE kb/versions`.

### `POST /api/v1/projects/{id}/kb/pages/{path}/review`

Body, all optional: `{if_version?, verified_by?}`. The review appends
`{by: human, at: <now>}` to the page's `verified` block and changes no other
byte. The reviewer is set by the hub: `verified_by` is accepted only as the
literal `human` and is otherwise a 400. `if_version` is the version the human
read; when the page changed since, the review is a 409 and nothing is stamped.
Returns `{ok, path, version}`, logs one `kb.review` row with the actor `human`,
and appends one `kb_reviewed` signal.

### `POST /api/v1/projects/{id}/kb/pages/{path}/revert`

Body: `{version, if_version?}`. Writes the bytes of `version`, one the page's
history names, back to the page as a new write by the human, logged as
`kb.revert`. `if_version` guards it as it guards a put: the version the reader
saw as current, or `absent` to restore a deleted page only while it is still
gone. Returns `{ok, path, version, size_bytes, lint[], warnings[]}` and appends
one `kb_reverted` signal. A revert removes nothing from the history, so the
version it replaced can be reverted to in turn. A revert to the version the
page already holds writes nothing, logs nothing and signals nothing; its
result says so with `changed: false`, where a revert that wrote is
`changed: true`.

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

This is the whole base's write log. One page's versions, with their bytes, are
`kb/versions`. The log is bounded only by the knowledge base file's own 1 GiB
limit and every request scans all of it, so `total` is the real count and a
page never loses its history or its last writer by being old. `op` may also be
`kb.revert`.

### `GET /api/v1/projects/{id}/kb/versions`

Query: `path`, `before`, `limit`. Returns `{path, current_version, versions[],
total, next_before, truncated}`, newest first. A version row is `{id, op,
actor, at, version, size_bytes, summary, kept, current}`: `id` is the log row
and the `before` cursor, `version` and `size_bytes` are `null` on a delete,
`summary` says what the write did (`created`, `edited`, `reviewed`, `promoted
from a session brain`, `deleted`, `restored the version of <time>`, or
`restored an earlier version` when the version it restored has no earlier
write in the log to date it by), `kept` is
whether its bytes can be read and restored, and `current` marks the newest
write of what the page holds now. `current_version` is `null` for a page that
is not there, which keeps its history. `limit` defaults to 50 and is at most
200.

Every write and delete keeps the bytes it replaces and the bytes it stores in
the knowledge base file itself, so every version written since the hub kept
versions reads back; a version a hub from before that replaced is listed with
`kept: false`. Nothing expires: the versions are bounded by the file's 1 GiB
limit until the operator forgets them. The limit is held against the pages the
file uses, so space a purge frees counts at once, and a write is checked at its
own size plus a copy of each version it keeps that the file does not already
hold. A delete is never refused for the copy it keeps, at most one page, so a
full knowledge base can still shed a page. The decision is
[ADR 0029](../adr/0029-knowledge-base-page-history.md).

### `DELETE /api/v1/projects/{id}/kb/versions`

Query: `path`. Admin only, like prune. Forgets the kept bytes of one page's
history: every version its log names except the one the page holds now, and
all of them for a deleted page. Returns `{ok, path, versions_forgotten,
bytes_forgotten, unreferenced_forgotten}`. It also sweeps kept versions that
no log row of any page names, which only a crash between keeping a write's
bytes and logging it leaves, and counts them in `unreferenced_forgotten`. The rows stay, so the history still says who wrote what and
when, and they read `kept: false`; a version read or a revert of one is then
`not_found`, and a deleted page can no longer be restored. A version whose bytes
another page's history still names stays for that page and is not counted, but
no longer reads back through this one. Writes after the purge keep their
versions as before.

The purge is recorded as a `system` event by the human with the payload
`{action: "kb_history_forgotten", path, versions, bytes, unreferenced_versions,
unreferenced_bytes}`, which no agent reads. It is appended before anything is
removed, so a purge that cannot be recorded removes nothing, and the removals
and the mark land together or not at all; a purge that rolls back after its
audit appends a second event, `kb_history_forget_rolled_back`, with the same
counts.
The freed space is reused by later writes rather than given back to the disk,
and the engine does not zero freed pages, so the bytes can stay on disk until
they are overwritten; a backup taken before the purge still holds them. A path
the log has never named is `not_found`.

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

## Sync with a folder

`agent-hub kb export` and `agent-hub kb import` move a whole knowledge base to a
plain folder and back, so an operator can edit it in their own editor, diff it
or keep it in git. Both are client commands: they read `HUB_URL`, `HUB_TOKEN`
and `HUB_PROJECT` (or `--project`) like the rest of `agent-hub kb`, and go
through the same agent tools on one connection. The pages land only in the
directory named. The decision is
[ADR 0030](../adr/0030-knowledge-base-folder-sync.md).

```sh
agent-hub kb export --dir ./kb               # every page, at its path under /fs
$EDITOR ./kb/runbooks/deploy.md
agent-hub kb import --dir ./kb --dry-run     # what would change, nothing written
agent-hub kb import --dir ./kb               # write the pages that changed
```

### Export

`kb export --dir D` writes each page to the file at its path under `/fs`, so
`/fs/runbooks/deploy.md` is `D/runbooks/deploy.md`, and writes the manifest
`D/.agent-hub-kb.json`:

```json
{
  "format": 1,
  "project": "homelab",
  "pages": {
    "/fs/index.md": "sha256:...",
    "/fs/runbooks/deploy.md": "sha256:..."
  }
}
```

A directory that holds anything is refused. `--force` writes into it anyway:
it writes over the page files and the manifest, and removes the file of each
page the old manifest names that the hub no longer has, printing
`removed <path>` for it, so the next import does not bring the page back. Only
a regular file reached through plain directories inside D is removed; a
directory left empty stays, and nothing the old manifest does not name is
touched. A manifest that cannot be read, or one exported from another
project, refuses the export.

Every page is read, and every target checked, before D is created or the first
file written: a path that would leave the folder, a symbolic link on the way
to a page, or a directory where a page goes refuses the export. Each file is
then opened without following a symbolic link, so one put in place after the
check is refused rather than written through. A page whose path has a
dot-named component is left out with a note on stderr, because an import does
not read it back.

Two pages whose paths differ only in case or Unicode normalisation, in a file
name or a directory on the way to one (`Notes.md` and `notes.md`, `Runbooks/`
and `runbooks/`), refuse the export. A filesystem that ignores case or
normalisation, as macOS does by default, keeps them as one file, and the
import after it would write the survivor to the wrong page. Rename one on the
hub first. The check folds case with Unicode lowercasing after NFC
normalisation, which is close to what such a filesystem does but not the same
in every script.

### Import

`kb import --dir D` reads every file under D except dot-named files and
directories, which keeps the manifest and a `.git` directory out of the bundle.
A symbolic link, a file that is not UTF-8 text, a page over 1 MiB, a path the
hub would refuse, two files whose paths differ only in case or normalisation,
and a folder over the bundle limits are refused before anything is written.
A file name is read as its NFC spelling, which is how an export wrote it, so a
name macOS hands back decomposed is the same page. Once the hub is listed, a
folder page that differs only in case or normalisation from a hub page the
import keeps is refused too: a rename that changes only case is then one page
again only with `--prune`, which deletes the old spelling.
Files are opened without following a symbolic link. A manifest exported from
another project is refused too, and so is one that names a page twice or names
a path that is not a canonical page path under `/fs`.

The import then plans every change against the hub, as below, and lints the
folder whole with the hub's own lint. These findings refuse the import with
nothing written when they are on a page it would create or update:
`unparsed_frontmatter`, `okf_version_misplaced` and `link_escapes_bundle`. On a
page the import leaves as it is, the hub already holds them, so they are
printed as warnings and do not refuse. The rest
(`missing_frontmatter`, `missing_type`, `broken_link`, `missing_index`,
`missing_log`, `orphan_page`, `missing_index_entry`) say a bundle is
incomplete. Each warning is one line on stderr,
`agent-hub: lint <code> <path>:<line>: <message>`, or without `:<line>` for a
finding about the whole page, and the import goes ahead, so a knowledge base whose pages already carry a broken
link still imports its own export back.

Each page is compared by version in the folder, on the hub and in the manifest,
and gets one line on stdout:

| Line | When | Written |
|---|---|---|
| none | the folder and the hub hold the same bytes | no |
| `create <path>` | the folder has a page the hub and the export do not | with `if_version: "absent"` |
| `update <path>` | the folder changed the page and the hub did not | with the manifest's version |
| `hub newer <path>: left as is` | the folder holds the page as exported and the hub changed or deleted it since | no |
| `delete <path>` | with `--prune`, the export had the page and the folder does not | with the manifest's version |
| `keep <path>: ...` | without `--prune`, the export had the page and the folder does not | no |
| `conflict <path>: ...` | the folder changed the page and the hub changed, deleted or created it too; with `--prune`, the folder removed a page the hub changed | no |
| `delete <path>: already gone` | with `--prune`, the hub deleted the page while the import ran | no |
| `not attempted <path>` | the hub refused an earlier change for a reason other than a conflict | no |

A conflict is skipped and every other change is still made. A page that
changes on the hub while the import runs is refused by its guard and reported
as the same conflict. Any other refusal from the hub stops the import there:
the lines above it are what was made, the page it refused and every later
write or delete read `not attempted`, and the hub's error follows on stderr
with its exit code. A manifest that cannot be saved once the hub was written
is reported the same way, after the lines for what was made. A last line counts each kind. The import then rewrites the
manifest with the version of every page it wrote or found in step, and drops
each page that is gone from both the folder and the hub, so a second import of
the same folder changes nothing. A `hub newer` page keeps the version it was
exported at; the next export brings the hub's copy and its version into the
folder. A folder with no manifest is taken as
a new bundle: every page in it is a `create`, and one the hub already has with
other bytes is a conflict.

`--prune` deletes only pages the manifest names, so a page created on the hub
after the export is never pruned. `--dry-run` prints the same lines, with
`create`, `update` and `delete` as `would create`, `would update` and
`would delete`, and writes nothing, the manifest included.

The exit code is 0 when every change was made, 1 when any page was a conflict
(on a dry run too, so a script can check first), when the lint refused the
folder, or when a file refused, the hub's own code when it stopped the import, 2 for a usage mistake, and the
client codes `agent-hub kb` already uses for an unreachable hub (69), a refused
token (77) and no hub configured (78).

There is no merge. To resolve a conflict, export again into a fresh folder and
carry the change across, or write the page with `agent-hub kb put`.

## Agent tools

`brain_get`, `brain_put`, `brain_list` and `brain_delete` reach the knowledge
base with `store: "project"`, under the project's own read and write grants.
`brain_promote(from_path, to_path, project_id?, type?, title?, description?,
tags?, if_version?)` reads `from_path` from the caller's active session brain
and returns `{ok, path, version, lint[]}`. A `brain_delete` of a page that does
not exist is `not_found`.

Every page an agent may read is also an MCP resource,
`agenthub://kb/<project_id>/<path>` with the path under `/fs/`, so a client that
attaches resources puts a page into context without a tool call. The listing is
paged with a cursor and the read takes the same check as `brain_get`; the
[agent surface](../architecture/agent-surface.md#resources) has the details.

`brain_history(path, project_id?, before?, limit?)` lists a page's versions as
`kb/versions` does, `brain_get` with `store: "project"` and `version` reads one
with `at` and `actor`, the newest write of those bytes, and
`brain_revert(path, version, project_id?, if_version?)` puts one back, deleted
pages included, returning `{ok, path, store, version, size_bytes, lint[],
warnings[], changed}`. History reads need the project's read access and a
revert its write access, the same as any read and write. Forgetting a history
is not an agent tool.

## The Wiki segment

A project carries a fourth section, **Wiki**, beside Feed, Artifacts and
Sessions; its tab names the page count. The tree is one listing of the whole
knowledge base (`meta=1`), so directories, pages and their derived numbers
arrive in one request. Selecting a page renders it: the frontmatter block is
metadata and is shown as the page's type, status and tags rather than as body
text, and the backlinks are listed under it. An edit writes the page back with
the version token the read carried, so a page that changed under the reader is
refused rather than overwritten; the conflict is said in place, with **Reload
theirs** and **Keep mine**. **New page** writes a page that is not there yet,
with `if_version: "absent"`. A **Review** control on the reader stamps the page
the human has read, carrying the version they read; **Needs review** lists the
pages the hub judges not human-reviewed, and a row opens the page. A session
brain entry offers **Save to wiki** (`kb/promote`). The Wiki home carries the
page, needs-review and stale counts and holds the housekeeping screens:
**Recent changes** (`kb/history`, who did what to which page and when; a row
opens that page's history) and **Lint** (`kb/lint`, the tree's findings
with a Re-check that walks it again).

A page's **History**, opened from the reader's control row, lists its versions
newest first with who, when and what. A version opens as a line diff against
the page as it is now, and **Revert to this** puts it back after a
confirmation, carrying the version the reader saw as current. A deleted page's
address says it was deleted and links to its history, where **Restore this**
writes it back the same way. A diff too long to work out line by line says so
and shows the version itself. **Forget history**, in the history's header,
forgets the kept bytes after a confirmation that names the page.

Not in this version: rename and move; a rendered-compare merge (the conflict
path offers the hub's copy or yours, not a merge); and comment threads on a
page, which have no store in the hub at all.

## What there is not

There is no move or rename. A move is a read, a write with
`if_version: "absent"` and a delete, by the client. The new path starts a
history of its own, since history is keyed by path; the old path keeps its
history, and links to the old path are not rewritten:
`backlinks` names them first. Wiki links (`[[page]]`) are not links to the hub;
only markdown links are followed.

Offline reading for the knowledge base is to be evaluated. Today what is cached
by the service worker is the application shell and its static assets; page content
is not cached, and reading pages requires an active connection to the hub.
