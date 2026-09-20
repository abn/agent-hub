---
type: Reference
title: Human surface
description: The REST API, the installable PWA, and the v1 auth model.
tags: [architecture, pwa, rest, human]
status: draft
---

# Human surface

The human reaches the hub through a REST API and an installable,
mobile-first PWA served as static assets from the same binary. The feed,
session, inbox, home, artifact, prune, search, and identity routes ship, along
with the installable PWA shell and its screens: Home, Inbox, Project feed,
Artifacts and viewer, Sessions, Storage, Search, and Settings with the Agents
and access section, a session detail view, and project deletion under
Settings. The session detail view shows a drill-down brain tree with expand,
collapse, keyboard navigation and lazy-loaded children. The binary serves the REST API, the PWA, and the MCP
endpoint on one listener in one process, so the per-session write lock covers
every writer and the prune sweeper always runs.

## REST API

A versioned JSON API backs the PWA and any other client. A request body is
capped at 4 MiB:

```
GET    /api/v1/home
GET    /api/v1/projects
POST   /api/v1/projects
GET    /api/v1/projects/:id
PATCH  /api/v1/projects/:id
DELETE /api/v1/projects/:id
GET    /api/v1/projects/:id/stats
GET    /api/v1/projects/:id/feed
POST   /api/v1/projects/:id/feed/seen
GET    /api/v1/projects/:id/artifacts
GET    /api/v1/inbox?status=&project=&limit=&unread_only=
POST   /api/v1/inbox/:id/read
POST   /api/v1/inbox/:id/unread
POST   /api/v1/inbox/read-all
GET    /api/v1/stream
POST   /api/v1/questions/:id/answer
POST   /api/v1/approvals/:id/decision
POST   /api/v1/sessions/:id/end
POST   /api/v1/sessions/:id/reassign
GET    /api/v1/sessions?project=
GET    /api/v1/sessions/:id
GET    /api/v1/sessions/:id/brain?path=
GET    /api/v1/agents
POST   /api/v1/agents
PATCH  /api/v1/agents/:id
POST   /api/v1/agents/:id/token
DELETE /api/v1/agents/:id/token
GET    /api/v1/agents/:id/grants
POST   /api/v1/agents/:id/grants
DELETE /api/v1/agents/:id/grants/:projectId
GET    /api/v1/artifacts/:id
GET    /api/v1/artifacts/:id/versions
GET    /api/v1/artifacts/:id/raw
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
GET    /api/v1/search?q=&scope=&project=&type=&limit=
GET    /healthz
GET    /readyz
GET    /artifacts/:id
GET    /artifacts/:id/frame
GET    /artifacts/:id/og.svg
GET    /SKILL.md
```

The artifact content route accepts `?version=N` to read one snapshot; the
versions route lists the immutable history oldest first; the raw route
returns the stored bytes as text, or a JSON envelope with base64 ciphertext
for a protected artifact. Deletion removes the artifact, its history, and
its search entry, and records a `deleted` event. The public page serves
`?version=N` the same way.

The two probes differ on purpose. `GET /healthz` is liveness: the process is
up. `GET /readyz` is readiness: it queries the engine for its schema version
and answers `200` with that version only when the store replies and is at the
version the process opened. Otherwise it is a `503` problem, so an orchestrator
takes the node out of rotation instead of holding it there while every call
fails.

The whole REST surface is the human control surface and is admin-only: it
accepts the configured admin token and nothing else, so an agent token is
rejected there. Agents reach the hub over MCP.

`GET /SKILL.md` is a public bootstrap guide. The hub renders the caller's own
origin into it from the forwarded or request host, so an agent that can already
reach the hub can fetch the connection details and the tool list before it has
a token, and behind a reverse proxy it receives the public address rather than
the internal bind.

Errors are RFC 9457 problem details, whatever part of the request is refused:
a body over the limit is a 413 with `payload_too_large`, a body without the
JSON content type a 415, a bad query string or path segment a 400, and a
method the path does not serve a 405 that still names the methods it does.
The public artifact route serves a host
shell around a sandboxed frame: the shell owns the back button, title,
version line, version picker, and theme toggle on the design tokens, and
the frame runs authored content with scripts allowed but
no network, storage, or same-origin access. For a protected artifact the
shell shows a password gate with a ciphertext fingerprint and an optional
device-local remember; it decrypts in the browser. A public markdown
artifact renders in the page with tables, callouts, and diagrams, and any raw
HTML in its source is escaped. Every page carries link-preview tags with a
built-in card. Storage acts on sessions only,
and a session prune returns an undo token valid for a short window. Deleting a
whole project is the destructive endpoint under projects.

### What each response carries

Every number a surface shows comes from one of these fields. Nothing is
derived from a proxy, and a number that cannot be measured is absent rather
than zero.

| Route | Carries |
|---|---|
| `GET /api/v1/home` | `unread`, `waiting`, `agents_active`, `last_event_at`, `recent`, `unseen` (per project, the feed events above its cursor), `storage` (`used_bytes`, `capacity_bytes`, `free_bytes`) and `prunable` (`sessions`, `bytes`), so Home makes one request |
| `GET /api/v1/projects` and `GET /api/v1/projects/:id` | each project with its `id`, `display_name`, `owner_agent`, `created_at`, `artifact_password_policy` and `unseen_events` |
| `GET /api/v1/projects/:id/feed` | `events`, `next_since`, `next_before`, and `last_seen`, the newest event the human has seen, so a client draws the unread dot on an event whose id is above it |
| `POST /api/v1/projects/:id/feed/seen` | `project_id`, the resulting `last_seen`, and `advanced`, false when the cursor did not move |
| `GET /api/v1/storage` | `total_bytes` (what the projects hold) and `used_bytes` (the whole data directory, hub store included), `capacity_bytes` and `free_bytes` for the volume, `data_path`, `node` (`host`, `mode`), `by_kind` (`events`, `sessions`, `artifacts`, `knowledge`), `prunable`, and a row per project with its artifact, session, knowledge and prunable bytes |
| `GET /api/v1/projects/:id/stats` | `events`, `artifacts`, `sessions`, `kb_pages` and `agents_active` for the project header and its tab labels |
| `GET /api/v1/projects/:id/kb/...` | the knowledge base pages, their history, backlinks, lint and derived numbers, route by route in [project knowledge base](../usage/knowledge-base.md) |
| `GET /api/v1/sessions?project=` | each session, its owner, handoff, lineage and `brain_bytes` |
| `GET /api/v1/sessions/:id` | the same fields plus `events`, the count of feed events the session produced, and `last_event`, the newest of them as one line |
| `GET /api/v1/sessions/:id/brain?path=` | `entries[]` with `path`, `type` (`key`, `file` or `dir`) and `size_bytes`, one directory level per request, with `path` echoed and `truncated` when the level held more |
| `GET /api/v1/search` | `count`, the hits on this page before grouping, `truncated` when the limit cut the result, `took_ms` around the store call, and `groups[]` each with its own `count` |
| `POST /api/v1/inbox/:id/read` and `.../unread` | `event_id`, the `status` the entry carries now, and `changed`, false when the entry was already there or carries no read state |
| `POST /api/v1/inbox/read-all` | `marked`, how many entries moved |

`count` is the hits the page carries, not how many documents match, and
`truncated` says when the limit cut it, so a capped page is never printed as a
total.

The volume's capacity and free space come from one `statvfs` on the data
directory, and the free figure is what a writer that is not root can use. When
the call fails, `capacity_bytes` and `free_bytes` are `null` and the rest of the
response is still served, so a surface renders "unknown" rather than 0 of 0.
The numbers that cost a syscall or a file stat are memoised for ten seconds
behind a counter every write bumps; the counts are indexed and never cached.

Read is explicit. The human marks one inbox entry read or unread, or marks
every unread entry read, optionally within one project; nothing is read by
scrolling past it. The read routes move an entry between `unread` and `read`
and nothing else: an entry that waits on the human, or one already resolved,
is answered with its unchanged status and `changed` false, so marking an
approval read never takes it out of what waits on you. They are idempotent,
and an event with no inbox entry is a 404. The listing's `unread_only` is the
Inbox header's filter and is refused when it contradicts an explicit `status`.
The Inbox calls them: opening an item, a swipe right, the row's own control and Mark all read.

A feed is not read that way. Each project carries one cursor, the newest event
the human has seen, and every event above it is unseen. Opening a project feed
is what moves it: a client reads the page and posts the newest id it showed.
The cursor only ever moves forward, and an id that is not an event of that
project, or names nothing at all, leaves it where it was and says so. A client
that reads a filtered page must post the newest id of the whole feed it has
seen, not of the filtered view, since the cursor covers the feed and not the
filter. The count above each cursor rides on the project listing and on Home,
so a tab row and a Home row draw the same dot without a request of their own,
and it counts only projects the listing returns. Upgrading an existing hub
seeds every cursor at that project's newest event, so nothing is lit by the
upgrade itself.

The project feed is the client of that cursor. A row whose id is above the
cursor carries an accent dot, a heavier title and the word "Unread" for a
reader who gets neither. The marks are held for the length of one stay on the
feed, against the cursor as it stood on arrival, so a repaint after a filter
does not wipe them. The PWA posts the newest id once an unfiltered page has
been painted in a visible tab, and posts nothing for a filtered page, which
skips events it cannot speak for.

A project is settable after it is created. `PATCH` changes its display name
and its artifact password policy (`off`, `optional` or `required`, and
`optional` for every project that has not said otherwise); a field the body
does not name is left alone, and an unknown field is ignored as it is on the
create route. The id is the slug every MCP call and every other table names,
so it is read-only after creation and a body that carries one is refused. An
agent's personal space is settable like any other project; only deleting it is
refused, because that is the agent's lifecycle rather than a setting. The
Project settings screen is the client of both routes: it reads the project,
and a save is one `PATCH` that names only the fields the reader changed.

A batch prune takes every ended session of one project, or of every project,
with the same soft delete, undo window and sweep as pruning one. It never
touches an active session, a feed event, an artifact or a project knowledge
base. It returns one undo token per session rather than a token of its own, so
undo is the route it already was, once per token.

## PWA

The interface follows the design foundation, whose tokens, type, spacing,
states, and copy are final. It installs to a phone home screen and works
equally well on desktop. Mobile is primary, with a four-tab bar of labelled
icons (Home, Inbox, Projects, Search) ending in a safe-area bottom edge;
desktop swaps it for a top bar that adds an inline search field with a slash
hint, the node line naming the hub the storage response reports, and a gear
to Settings. Each project is an address of its own: the feed, the artifact
gallery and the sessions list are segmented tabs under `#/projects/<id>/`,
every one with its own route, and the artifact viewer is a route too
(`#/artifacts/<id>`), so reload and the browser's Back keep the artifact on
screen. A desktop list plus detail layout (a 420px list pane beside a detail
pane) is a layout primitive screens opt into; Sessions is the first to use
it, and the project tabs sit in the same shell.

| Screen | Purpose |
|---|---|
| Home | Today at a glance, from the one Home response: a title that is the reader's own day and part of day over a summary line (waiting, unread, agents active); a "Waiting on you" card with the queue's count, the waiting items among the newest events and a way into the Inbox for the rest; "Newest across projects", each row naming its project and linking to that project's feed, with the unseen dot drawn from the per-project cursor counts; and a storage card linking to Storage, with used against capacity, a bar, the same share in words, and what a prune would free. A volume that cannot be measured shows the used bytes alone, with no bar. When nothing waits, nothing is unread and nothing sits above a cursor, the two cards give way to the quiet empty state. The response carries project slugs rather than display names, and no list of the waiting queue itself, so rows name the slug and the card lists only waiting items that are among the newest events. |
| Project feed | What happened in one project, filterable by kind on one scrolling line of chips. Today and Yesterday are open; older days sit behind an "Earlier" disclosure that carries the hub's own count and pages back on the `next_before` cursor. Unseen events are marked from the project's read cursor, and viewing the feed moves it. An empty feed offers "Copy MCP setup", which copies the connection details for this hub's origin and shows them to be copied by hand where the browser has no clipboard. |
| Inbox | The global queue in three groups: "Waiting on you" with its count, its open items grouped by actor; "Unread" with its count; and "Earlier", the read items, folded on the desktop. A row carries the title, a one-line body on a waiting item (the event payload's `body`, when it is a string), and a footer of project and agent. An unread row is marked by a dot, its weight and the word beside the dot. An approval offers Decline and Approve and a question offers Reply; each decision is asked for in a dialog. Opening a row shows the whole item as a card, with the answers at full size and a question's composer held open; the open item and the filter live in the address. The card carries its own way out, "Back to inbox" on a phone and "Close" beside the list on the desktop, named by the words it shows; Esc closes it too, except from a field that holds text, and only while the Inbox is the screen. Closing hands focus back to the row the card was opened from, or to the Earlier disclosure when that row is folded under it, as the row of an item read by opening it is on the desktop. Esc and the card's own control close it the same way: the card's address is replaced, so Back does not reopen it. The header carries "Unread only" and "Mark all read", and a last-synced line with a Refresh control. A swipe right marks a row read or unread, a swipe left uncovers a waiting row's actions, and a pull down refreshes. Quick answers on a question, a snooze, and a note sent with a decision are intended design, not yet shipped. |
| Artifacts | A per-project gallery and viewer: the viewer embeds the artifact page with its unlock form, themes, and version picker, plus a comments drawer with compose, resolve, and delete. |
| Sessions | Sessions per project, with End and a Prune that is confirmed in a dialog and undoable for 30 seconds. Session rows carry the state dot, owner, mono size and a chevron; the detail shows three stat cards, the lineage and handoff note, a Latest event line, a drill-down brain tree with lazy-loaded children, and a pinned End / Prune action bar. An audit log over the brain file's own tool calls is intended design, not yet shipped; the Latest event line is the session's newest feed event, not that log. |
| Search | One field over feed, artifacts, and session brains, answering as it is typed. The query and the scope live in the route (`#/search?q=&type=`), so a reload or the browser's Back lands on the same results, and an answer that arrives after a newer query is dropped. Scope chips filter by the route's `type` (All, Feed, Artifacts, Sessions). A results line gives the hub's own `count` and `took_ms` and is announced politely; a page the limit cut is called the first of more, never a total. Results are grouped by family with their counts, and each row carries the title, the snippet with the matched words marked, the project, and when it changed. The screen sends the query as it was typed, with its case, quotes and operators, and the hub makes it safe for the index: a balanced quoted phrase is searched as a phrase, and other punctuation and the bare operators are left out, so no query is refused. The marked snippet is built from text nodes, so neither a query nor a snippet is read as markup. A project scope, a date scope, and landing on the hit inside its destination are intended design, not yet shipped. |
| Storage | The data path and node, what is used against the volume's own capacity, and a stacked bar by kind (events, sessions, artifacts, knowledge) whose legend carries each kind's byte figure, so the bar is never the only place a number lives. The bar is drawn against the volume; when under 1% of the volume is used no segment would be a pixel wide, so it is drawn against what is used instead and says so beside it and in its text alternative. No segment is ever widened. A row per project shows its total, its own bar and the same split in words, and a Prune button with the bytes its ended sessions would free. A Prune all card reviews the affected projects, session counts and bytes in a dialog first. Either prune is one request, confirmed with Keep focused first, and undoable from the toast for 30 seconds. A hub whose projects hold nothing shows the empty state instead. Pruning a single session stays on the Sessions screen. |
| Project settings | Reached from the gear in a project's header, at `#/projects/<id>/settings`. The name is an editable field; the slug is shown in mono as text, not as a field, because it is read-only after creation; the artifact password policy is a radio group over `off`, `optional` and `required`. Save stays disabled until something differs from the hub's copy and sends only what differs. A blank name is caught on the screen, and a name or policy the hub refuses is reported beside that control with the hub's own reason while the form keeps what was typed. Leaving with edits pending asks first, in the confirmation dialog. Retention is a reserved card that says automatic pruning is not in v1 and links to Storage; it carries no control. Delete project opens the same confirmation the global Settings list uses, and is not offered for an agent's personal space. |

Agent and access management lives under Settings, not a tab. It lists agents
with their trust level, creates an agent and its personal space, promotes or
demotes it, issues or revokes its single token (shown once), and manages
grants. The artifact viewer embeds the artifact page: the host shell shows
an unlock form for a protected artifact and decrypts in the browser, then
renders HTML or rendered markdown inside the same sandboxed frame. Markdown
renders in the page with raw HTML in its source escaped; protected markdown
renders from the decrypted source, so the server never sees it.

The session listing route carries the agent that owns each session, the
handoff note its last owner left, and its lineage: whether the work was adopted
or forked, from which session and which agent, and whether that source has since
been pruned. The PWA does not render those fields yet; that is intended design.
A session opens into a detail view that lists its brain keys and files
and offers End and Prune. Reassigning a session to another agent is the
human's move for an agent that is not coming back, over the reassign route; the
PWA has no control for it yet. Agents pick work up themselves and ask nobody. Project deletion is a destructive action under Settings
and on the project's own settings screen, behind a confirmation; an agent's personal space cannot be deleted. Inbox
notifications are opt-in from Settings, request permission only on that
action, and carry only waiting-on-you items; without permission or support
they degrade silently. True background push is deferred, by
[decision](../adr/0016-push-notifications-deferred.md); an open app reads the
freshness stream and refreshes its waiting badge on a tick, with a slow poll
as the fallback.

The app ships as ES modules with no bundler and no build step. `app.js` is
the entry: it names the screens the router can paint and routes the delegated
click, submit, and change events to the handler that owns each action.
Beside it sit a shared core (the API client, the router, the DOM and escaping
helpers, the relative-time component, preferences, the keyboard map, the
empty-state component, the confirmation dialog, the toast, the reply composer,
the freshness stream and badge, and the project picker) and one module per
screen, with the comments drawer in its own. The keyboard map is one module the
screens with rows register with, so the shortcuts and the roving selection are
defined once rather than per screen. An open dialog holds the map, asked of the
document rather than announced to it, so every dialog holds it and none has to
remember to. The reader can switch the single-key shortcuts off in Settings.
The binary embeds every one of them in the same table it
serves, precaches, and digests for the service worker's cache name, so a
module the hub does not serve cannot ship. Each render carries a number, and
a screen whose fetches resolve after the reader has moved on does not paint
over the screen that replaced it.

### Vendored scripts

External runtime scripts live in `web/vendor/` and are tracked in
`web/vendor/MANIFEST.json`. The manifest records each vendored file's name,
version, upstream URL, license, and SHA-256 digest. Static checks
(`make web/check`) verify that every file in `web/vendor/` matches its manifest
entry, that hashes match, and that no untracked files exist.

To update or add a vendored script:
1. Place the script in `web/vendor/`.
2. Update `web/vendor/MANIFEST.json` with the filename, upstream URL, license,
   and banner-derived version (or state that the banner lacks a version).
3. Compute and record the SHA-256 hash in `MANIFEST.json`.
4. Run `make web/check` to verify the manifest against disk.

Two optional browser gates cover the surface: an accessibility audit over the
rendered screens in both themes, and a smoke pass that visits every route
against a seeded hub and checks the heading, the seeded data, the nav
marking, the actions that write, and that nothing logged an error or left a
request failing. Both skip cleanly where the browser toolchain is absent.

The freshness stream carries no event data. It signals that a write changed
the inbox or the feed, and the client refetches its waiting badge, so it is a
mailbox nudge rather than a chat channel and the asynchronous interaction
model is unchanged.

The waiting queue groups its open items by actor, so an agent that leaves many
items is one block with its own count rather than a run of rows buried in the
list. The number of open items an actor may leave is capped, by
[decision](../adr/0017-inbox-action-item-cap.md); the defaults are generous and
two environment variables set them, with zero disabling the guard.

The interaction model is read, answer, approve, and prune, with no chat
interface, by [decision](../adr/0007-async-mailbox-semantics.md). Alerts have
three levels: quiet, unread, and waiting on you. They are never red, never
animated, and never a modal. Prune is confirmed, then reversible for a short
window, then committed. The app asks and reports in its own components: a
modal confirmation dialog in front of anything destructive, an announced toast
carrying the undo, and a composer for a reply, with no browser prompt, confirm
or alert anywhere, which a static check holds for every path rather than only
the ones a browser run walks. A write that fails says so in the toast or inside
the composer that tried it, and focus follows the control that was pressed
rather than falling to the top of the page. A toast carrying an undo takes
focus so the way back is under the reader's hands, except while they are
writing, when the live region announces it and the caret stays where it was.

The design carries hard invariants: no emoji, WCAG AA in both themes, a 12px
UI text floor, 44px tap targets, a visible focus ring, a full keyboard path,
and no colour-only meaning. Swipe gestures are always non-destructive and
always have a tap equivalent; swipe-down to dismiss a toast ships, with a
dismiss control and Esc beside it, and so do the Inbox row swipes and its pull
to refresh. The edge swipe back, the tab swipe and swipes on feed rows are
intended design, not yet shipped. See the
[human interface](../design/human-interface.md) for the tokens and rules.

## Auth

- The control surface uses a config-set admin token. It is required when the
  bind is not loopback, since the surface rejects every request without one.
- The MCP endpoint uses a per-agent bearer token bound to an identity and a
  trust level, by [decision](../adr/0012-agent-identity-and-trust.md). The
  stdio transport is the local admin and needs no token.
- Shared artifacts use a password and browser-side encryption, so the
  recipient needs nothing else.
- The PWA keeps the admin token in local storage, so a script running in the
  hub origin could read it. The shell is served with a strict content security
  policy, every agent-controlled field is escaped, and agent-authored HTML
  and rendered markdown run only in a sandboxed frame without same-origin
  access, so there is no hub-origin script injection path today. This is
  accepted for a single-operator node; a browser cookie is not a clean fit
  for a static PWA that also reaches the API.

## See also

- [Agent surface](agent-surface.md) - the same data through the agents' eyes
- [Human interface](../design/human-interface.md) - the design system and rules
- [Decision 0005](../adr/0005-installable-pwa-human-surface.md) - why a PWA
- [Decision 0009](../adr/0009-browser-side-artifact-encryption.md) - how
  protected artifacts work
