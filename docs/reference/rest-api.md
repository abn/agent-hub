---
type: Reference
title: REST API
description: The REST routes, what each response carries, and the auth model
tags: [operator, rest, pwa]
status: stable
---

# REST API

A versioned JSON API backs the installable app and any other client. The binary
serves the REST API, the app, and the MCP endpoint on one listener in one
process, so the per-session write lock covers every writer and the prune sweeper
always runs. A request body is capped at 4 MiB. The screen design is in
[Human interface](/design/human-interface.md); the agent side is in
[MCP tools](/reference/mcp-tools.md).

## Routes

```text
GET    /healthz
GET    /readyz
GET    /metrics
GET    /bootstrap/SKILL.md

GET    /api/v1/home
GET    /api/v1/feed
GET    /api/v1/projects
POST   /api/v1/projects
GET    /api/v1/projects/:id
PATCH  /api/v1/projects/:id
DELETE /api/v1/projects/:id
GET    /api/v1/projects/:id/stats
GET    /api/v1/projects/:id/feed
POST   /api/v1/projects/:id/feed/seen
POST   /api/v1/projects/:id/ideas
GET    /api/v1/projects/:id/artifacts
GET    /api/v1/projects/:id/kb/...      the routes in Knowledge base pages

GET    /api/v1/inbox?status=&project=&limit=&unread_only=
POST   /api/v1/inbox/:id/read
POST   /api/v1/inbox/:id/unread
POST   /api/v1/inbox/read-all
POST   /api/v1/questions/:id/answer
POST   /api/v1/approvals/:id/decision
GET    /api/v1/stream

GET    /api/v1/sessions?project=
GET    /api/v1/sessions/:id
GET    /api/v1/sessions/:id/brain?path=
GET    /api/v1/sessions/:id/brain/entry?path=
GET    /api/v1/sessions/:id/replay
GET    /api/v1/sessions/:id/resume
POST   /api/v1/sessions/:id/scores
POST   /api/v1/sessions/:id/end
POST   /api/v1/sessions/:id/reassign

GET    /api/v1/agents
POST   /api/v1/agents
POST   /api/v1/agents/:id/token
DELETE /api/v1/agents/:id/token
GET    /api/v1/agents/:id/grants
POST   /api/v1/agents/:id/grants
DELETE /api/v1/agents/:id/grants/:projectId
POST   /api/v1/enrol
GET    /api/v1/enrol/status?wait=30
POST   /api/v1/enrol/:id/approve
POST   /api/v1/enrol/:id/refuse

GET    /api/v1/artifacts
GET    /api/v1/artifacts/:id
GET    /api/v1/artifacts/:id/versions
GET    /api/v1/artifacts/:id/live
GET    /api/v1/artifacts/:id/raw
GET    /api/v1/artifacts/:id/viewer-pass
POST   /api/v1/artifacts/:id/share
DELETE /api/v1/artifacts/:id/share
GET    /api/v1/artifacts/:id/share
DELETE /api/v1/artifacts/:id
GET    /api/v1/artifacts/:id/comments
POST   /api/v1/artifacts/:id/comments
PATCH  /api/v1/artifacts/:id/comments/:commentId
DELETE /api/v1/artifacts/:id/comments/:commentId

GET    /api/v1/storage
DELETE /api/v1/storage/sessions/:id
DELETE /api/v1/storage/sessions
DELETE /api/v1/storage/projects/:id/sessions
POST   /api/v1/prune/undo/:token
POST   /api/v1/backups
GET    /api/v1/search?q=&scope=&project=&type=&limit=&mode=

GET    /artifacts/:id
GET    /artifacts/:id/frame
GET    /artifacts/:id/og.svg
GET    /s/:token
GET    /s/:token/frame
GET    /s/:token/og.svg
```

The artifact content route accepts `?version=N` to read one snapshot, or
`?live=1` for the version an agent is writing when its number is not known. The
listing and content routes carry thread counts across every version
(`comments_count` and `comments_open`, defaulting to 0); the versions route
lists the immutable history oldest first; the live route is a liveness read the
content route cannot express, returning `{state, version, live_rev, actor,
session_id}` where `state` is `live`, `idle` or `sealed`; the raw route returns
the stored bytes as text, or a JSON envelope with base64 ciphertext for a
protected artifact. Deletion removes the artifact, its history, and its search
entry, and records a `deleted` event. The public page serves `?version=N` the
same way.

## Sharing and the viewer pass

Sharing is the operator's act. `POST /api/v1/artifacts/:id/share` creates a
plain share link pinned to the active version and `DELETE` revokes it, both
admin-gated; `GET /api/v1/artifacts/:id/share` reports the current link. The
link is served unauthenticated at `/s/:token`, with `/s/:token/frame` for the
sandboxed content and `/s/:token/og.svg` for its preview card; a revoked or
unknown token is `404`.

While a link is live, `/artifacts/:id` and `/artifacts/:id/frame` are `404` for
every caller without a viewer pass, and `404` again once the link is revoked. The
pass comes from `GET /api/v1/artifacts/:id/viewer-pass`, admin-gated: it is a
keyed digest of the admin token, the artifact id and a sixty-second window,
recomputed on every request rather than stored, so it expires by itself and reads
one artifact's public page and nothing else. The app embeds that page in a frame,
and a frame navigation carries no bearer token, so the app mints the pass and
carries it in the frame address; that is the whole reason the pass exists. Agents
have no MCP tool to create or revoke a link, so their artifact workflow is
publish, version and protect, and the human shares.

## Probes

The two probes differ on purpose. `GET /healthz` is liveness: the process is up.
`GET /readyz` is readiness, and it answers `200` only when the store is usable:
the store's schema version is read, is not newer than the most this binary
supports, and matches the version the process opened; the data directory and
`hub.db` still have the device and inode identity captured at open, so a removed
or replaced store is caught; a cheap content read confirms the file is still a
hub store; and the volume has free space above a safety margin. Any leg failing
is a `503` problem, so an orchestrator takes the node out of rotation instead of
holding it there while every call fails.

## Auth

- The control surface uses a config-set admin token. It is required when the
  bind is not loopback, since the surface rejects every request without one.
- The MCP endpoint uses bearer tokens. A token is an identity in its own right,
  and grants are binary per project. The stdio transport is the local admin and
  needs no token.
- Shared artifacts use a password and browser-side encryption, so the recipient
  needs nothing else.
- The freshness stream at `GET /api/v1/stream` also accepts any active agent
  token, so an agent can wake on inbox or feed changes as well as the app.
- The app asks for the admin token on the Connect screen, and only there, so a
  token is never kept without the hub having accepted it first. It keeps the
  token in local storage, so a script running in the hub origin could read it.
  The shell is served with a strict content security policy, every
  agent-controlled field is escaped, and agent-authored HTML and rendered
  markdown run only in a sandboxed frame without same-origin access, so there is
  no hub-origin script injection path today. This is accepted for a
  single-operator node; a browser cookie is not a clean fit for a static app that
  also reaches the API.

The REST API is the operator's control surface. The destructive and privileged
verbs on it require the admin token: prune and undo, agent and token
administration, grants, deleting a project, making a confidential project public,
reading a session's resume, scoring a run, and taking an online backup
(`POST /api/v1/backups`, which writes only under the
configured `backup_dir`; see [Back up and restore](/operate/backup-and-restore.md)).
Agents reach the hub over MCP for their own work, and may also create and list
the projects they can see. The trust posture behind this split is stated once in
the [operating model](/architecture/operating-model.md).

`GET /bootstrap/SKILL.md` is a public bootstrap guide. The hub renders the
caller's own origin into it from the forwarded or request host, so an agent that
can already reach the hub can fetch the connection details and the tool list
before it has a token, and behind a reverse proxy it receives the public address
rather than the internal bind.

## Errors

Errors are RFC 9457 problem details, whatever part of the request is refused: a
body over the limit is a `413` with `payload_too_large`, a body without the JSON
content type a `415`, a bad query string or path segment a `400`, and a method
the path does not serve a `405` that still names the methods it does. The admin
gate runs before a request's body is read, so a caller without the admin token
gets `401` whatever body it sent (a query string that cannot be read is still a
`400` first), and an agent token is not the admin token.

## Artifact pages

The public artifact route serves a host shell around a sandboxed frame: the shell
owns the back button, title, version line, version picker, and theme toggle on
the design tokens, and the frame runs authored content with scripts allowed but
no network, storage, or same-origin access. For a protected artifact the shell
shows a password gate with a ciphertext fingerprint and an optional device-local
remember; it decrypts in the browser. A public markdown artifact renders in the
page with tables, callouts, and diagrams, and any raw HTML in its source is
escaped. Every page carries link-preview tags with a built-in card. Storage acts
on sessions only, and a session prune returns an undo token valid for a short
window. Deleting a whole project is the destructive endpoint under projects.

## What each response carries

Every number a surface shows comes from one of these fields. Nothing is derived
from a proxy, and a number that cannot be measured is absent rather than zero.

| Route | Carries |
|---|---|
| `GET /api/v1/home` | `unread`, `waiting`, `waiting_items` (the newest five of the items that wait, newest first, each shaped as an inbox entry with its `project_display_name`; `waiting` stays the size of the whole queue), `node` (`host`, `mode`, the same datum Storage carries), `agents_active`, `last_event_at`, `recent`, `unseen` (per project, the feed events above its cursor), each `recent` event and each `unseen` row with its `project_display_name` beside `project_id`, `storage` (`used_bytes`, `capacity_bytes`, `free_bytes`) and `prunable` (`sessions`, `bytes`), so Home makes one request |
| `GET /api/v1/projects` and `GET /api/v1/projects/:id` | each project with its `id`, `display_name`, `owner_agent`, `created_at` and `unseen_events` |
| `GET /api/v1/projects/:id/feed` | `events`, `next_since`, `next_before`, and `last_seen`, the newest event the human has seen, so a client draws the unread dot on an event whose id is above it |
| `POST /api/v1/projects/:id/feed/seen` | `project_id`, the resulting `last_seen`, and `advanced`, false when the cursor did not move |
| `POST /api/v1/projects/:id/ideas` | `id`, the event id of the idea; the idea is appended as a needs-action signal and surfaces in the inbox for triage |
| `GET /api/v1/storage` | `total_bytes` (what the projects hold) and `used_bytes` (the whole data directory, hub store included), `capacity_bytes` and `free_bytes` for the volume, `data_path`, `node` (`host`, `mode`), `by_kind` (`events`, `sessions`, `artifacts`, `knowledge`), `events_shared_bytes`, `prunable`, and a row per project with its `project_display_name`, its `events_bytes`, `session_bytes`, `artifact_bytes` and `kb_bytes`, its prunable sessions and bytes, and its optional `last_write` timestamp. Every project has a row, one that holds nothing with zeros, so a project a prune has just emptied keeps its row |
| `GET /api/v1/projects/:id/stats` | `events`, `artifacts`, `sessions`, `kb_pages`, `agents_active`, `threads`, `agents_written`, and `disk_bytes` for the project header, its tab labels, and the deletion manifest |
| `GET /api/v1/projects/:id/kb/...` | the knowledge base pages, their history, backlinks, lint and derived numbers, route by route in [Knowledge base pages](/reference/knowledge-base-pages.md) |
| `GET /api/v1/sessions?project=` | each session, its owner, handoff, lineage and `brain_bytes` |
| `GET /api/v1/sessions/:id` | the same fields plus `events`, the count of feed events the session produced, and `last_event`, the newest of them as one line |
| `GET /api/v1/sessions/:id/replay` | `session_id` and `events`, the brain's tool-call audit entries oldest first; admin token required, and an empty list when the brain is gone |
| `GET /api/v1/sessions/:id/resume` | `session_id`, `anchor` (the latest anchor or null, carrying `id`, `phase`, `summary`, `next_steps`, `sources` with each source's `id`, `kind`, `summary` and `found`, `owner` and `created_at`), `scores` (each carrying `id`, `slice`, `score`, `rationale`, `reviewer` and `created_at`), `events` (the session events after the anchor, oldest first), `next_since`, `recovery_path` and `truncated`; admin token required |
| `POST /api/v1/sessions/:id/scores` | takes `slice`, the anchor ids it grades, `score` from 1 to 5, a `rationale`, and an optional `idempotency_key`; a score is human-only, and a slice id must be an anchor in the same session. Admin token required |
| `GET /api/v1/sessions/:id/brain?path=` | `entries[]` with `path`, `type` (`key`, `file` or `dir`) and `size_bytes`, one directory level per request, with `path` echoed and `truncated` when the level held more |
| `GET /api/v1/sessions/:id/brain/entry?path=` | one entry's `path`, `type` (`key` or `file`), `size_bytes`, `content` as text, and `written_at` (null when unrecorded). Refuses directory paths with 409 and non-UTF-8 bytes with 422 |
| `GET /api/v1/search` | `count`, the hits on this page before grouping, `truncated` when the limit cut the result, `took_ms` around the store call, `backend` (the recall backend that served the page), and `groups[]` each with its own `count`; every hit carries its `project_display_name` beside `project_id`, and by family: `event_kind` and `actor` on a feed hit, `version` and `size_bytes` (the current version's) on an artifact hit, `session_name` and `session_status` on a session brain hit. They are read after the ranked fetch, one query per family on the page, and never change the order. `mode=semantic` asks for semantic recall; when no embedding model is configured the hub falls back to the lexical index and reports `fts-fallback` |
| `GET /api/v1/inbox` | `items[]`, each with `event_id`, `project_id` and its `project_display_name` (null for an id no project row carries), `kind`, `actor`, `summary`, `payload`, `status`, `created_at` and `updated_at`; a decided approval also carries `decision` (`decision`, `approved` or `declined`; `note`, null when none was left; `actor`; `event_id`, the answer that records it; `decided_at`); a resolved question carries `answer` (`body`, what was written in reply; `actor`; `event_id`, the answer that records it; `answered_at`) |
| `POST /api/v1/approvals/:id/decision` | takes `decision` (`approve` or `decline`), an optional `note` and an optional `idempotency_key`; answers with `event_id`, the answer appended to the approval's thread, whose payload holds `body`, `decision` and, when one was left, `note` |
| `POST /api/v1/inbox/:id/read` and `.../unread` | `event_id`, the `status` the entry carries now, and `changed`, false when the entry was already there or carries no read state |
| `POST /api/v1/inbox/read-all` | `marked`, how many entries moved |

`count` is the hits the page carries, not how many documents match, and
`truncated` says when the limit cut it, so a capped page is never printed as a
total. A hit's `snippet` is at most 200 characters of plain text. For an
artifact, a brain entry or a knowledge base page it is the opening of the indexed
text. For a feed hit it is what the agent wrote: the event payload's `body` when
that is a string, otherwise the event's summary, and never the payload's
serialized JSON. The corpus row of an event still holds the whole payload, so a
word anywhere in it finds the event; only what is shown changed, which is why a
store written before the change needs no rebuild.

The volume's capacity and free space come from one `statvfs` on the data
directory, and the free figure is what a writer that is not root can use. When
the call fails, `capacity_bytes` and `free_bytes` are `null` and the rest of the
response is still served, so a surface renders "unknown" rather than 0 of 0. The
numbers that cost a syscall or a file stat are memoised for ten seconds behind a
counter every write bumps; the counts are indexed and never cached.

## How the storage rows add up

Sessions, artifacts and knowledge are files and blobs that each belong to one
project, so the rows' `session_bytes`, `artifact_bytes` and `kb_bytes` sum to
`by_kind.sessions`, `by_kind.artifacts` and `by_kind.knowledge` exactly, and all
three to `total_bytes`. The hub store is one file every project shares, so
`by_kind.events` is that file's size and a row's `events_bytes` is what can
honestly be given to one project: the bytes of its events' summaries and
payloads, the hub's own audit events about the project included, since they go
when the project does. The rest of the file (the indexes, the search corpus, the
inbox, the identity tables, free pages) is `events_shared_bytes`, and the rows'
`events_bytes` plus `events_shared_bytes` is `by_kind.events`. The feed is
weighed once, on the first report after a start; later reports read only the
events appended since, and the two things that remove events, a committed prune
and a project delete, start the weighing over, as does every ten minutes. A
report still weighing when events are removed does not keep what it weighed, and
a project that is gone has no row whatever weights were held for it.

## Reading, decisions, and the feed cursor

A decision may carry a note saying why. It is trimmed, a blank one is no note,
and one over 2000 characters is a `413` with `payload_too_large` that decides
nothing. The note is stored on the decision's feed event as `payload.note`,
beside the one-line `payload.body` that reads "Declined: ..." wherever an answer
is shown, and it comes back on the approval's inbox entry as `decision.note`, for
the human and for the agent that asked.

Read is explicit. The human marks one inbox entry read or unread, or marks every
unread entry read, optionally within one project; nothing is read by scrolling
past it. The read routes move an entry between `unread` and `read` and nothing
else: an entry that waits on the human, or one already resolved, is answered with
its unchanged status and `changed` false, so marking an approval read never takes
it out of what waits on you. They are idempotent, and an event with no inbox
entry is a `404`. The listing's `unread_only` is the Inbox header's filter and is
refused when it contradicts an explicit `status`.

A feed is not read that way. Each project carries one cursor, the newest event
the human has seen, and every event above it is unseen. Opening a project feed is
what moves it: a client reads the page and posts the newest id it showed. The
cursor only ever moves forward, and an id that is not an event of that project,
or names nothing at all, leaves it where it was and says so. A client that reads
a filtered page must post the newest id of the whole feed it has seen, not of the
filtered view, since the cursor covers the feed and not the filter. The count
above each cursor rides on the project listing and on Home, so a tab row and a
Home row draw the same dot without a request of their own, and it counts only
projects the listing returns. Upgrading an existing hub seeds every cursor at
that project's newest event, so nothing is lit by the upgrade itself.

## Projects and pruning

A project is settable after it is created. `PATCH` changes its display name and
its artifact password policy (`off`, `optional` or `required`, and `optional` for
every project that has not said otherwise); a field the body does not name is
left alone, and an unknown field is ignored as it is on the create route. The id
is the slug every MCP call and every other table names, so it is read-only after
creation and a body that carries one is refused. An agent's personal space is
settable like any other project; only deleting it is refused, because that is the
agent's lifecycle rather than a setting.

A batch prune takes every ended session of one project, or of every project, with
the same soft delete, undo window and sweep as pruning one. It never touches an
active session, a feed event, an artifact or a project knowledge base. It returns
one undo token per session rather than a token of its own, so undo is the route
it already was, once per token.

## See also

- [MCP tools](/reference/mcp-tools.md) - the agent surface over the same store
- [Knowledge base pages](/reference/knowledge-base-pages.md) - the `kb` routes in full
- [Human interface](/design/human-interface.md) - the app's screens and rules
- [Operating model](/architecture/operating-model.md) - the admin boundary
