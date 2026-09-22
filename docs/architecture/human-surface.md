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
| `GET /api/v1/home` | `unread`, `waiting`, `waiting_items` (the newest five of the items that wait, newest first, each shaped as an inbox entry with its `project_display_name`; `waiting` stays the size of the whole queue), `node` (`host`, `mode`, the same datum Storage carries), `agents_active`, `last_event_at`, `recent`, `unseen` (per project, the feed events above its cursor), each `recent` event and each `unseen` row with its `project_display_name` beside `project_id`, `storage` (`used_bytes`, `capacity_bytes`, `free_bytes`) and `prunable` (`sessions`, `bytes`), so Home makes one request |
| `GET /api/v1/projects` and `GET /api/v1/projects/:id` | each project with its `id`, `display_name`, `owner_agent`, `created_at` and `unseen_events` |
| `GET /api/v1/projects/:id/feed` | `events`, `next_since`, `next_before`, and `last_seen`, the newest event the human has seen, so a client draws the unread dot on an event whose id is above it |
| `POST /api/v1/projects/:id/feed/seen` | `project_id`, the resulting `last_seen`, and `advanced`, false when the cursor did not move |
| `GET /api/v1/storage` | `total_bytes` (what the projects hold) and `used_bytes` (the whole data directory, hub store included), `capacity_bytes` and `free_bytes` for the volume, `data_path`, `node` (`host`, `mode`), `by_kind` (`events`, `sessions`, `artifacts`, `knowledge`), `events_shared_bytes`, `prunable`, and a row per project with its `project_display_name`, its `events_bytes`, `session_bytes`, `artifact_bytes` and `kb_bytes`, its prunable sessions and bytes, and its optional `last_write` timestamp. Every project has a row, one that holds nothing with zeros, so a project a prune has just emptied keeps its row. How the rows add up to `by_kind` is said below |
| `GET /api/v1/projects/:id/stats` | `events`, `artifacts`, `sessions`, `kb_pages` and `agents_active` for the project header and its tab labels |
| `GET /api/v1/projects/:id/kb/...` | the knowledge base pages, their history, backlinks, lint and derived numbers, route by route in [project knowledge base](../usage/knowledge-base.md) |
| `GET /api/v1/sessions?project=` | each session, its owner, handoff, lineage and `brain_bytes` |
| `GET /api/v1/sessions/:id` | the same fields plus `events`, the count of feed events the session produced, and `last_event`, the newest of them as one line |
| `GET /api/v1/sessions/:id/brain?path=` | `entries[]` with `path`, `type` (`key`, `file` or `dir`) and `size_bytes`, one directory level per request, with `path` echoed and `truncated` when the level held more |
| `GET /api/v1/search` | `count`, the hits on this page before grouping, `truncated` when the limit cut the result, `took_ms` around the store call, and `groups[]` each with its own `count`; every hit carries its `project_display_name` beside `project_id`, and by family: `event_kind` and `actor` on a feed hit, `version` and `size_bytes` (the current version's) on an artifact hit, `session_name` and `session_status` on a session brain hit. They are read after the ranked fetch, one query per family on the page, and never change the order |
| `GET /api/v1/inbox` | `items[]`, each with `event_id`, `project_id` and its `project_display_name` (null for an id no project row carries), `kind`, `actor`, `summary`, `payload`, `status`, `created_at` and `updated_at`; a decided approval also carries `decision` (`decision`, `approved` or `declined`; `note`, null when none was left; `actor`; `event_id`, the answer that records it; `decided_at`); a resolved question carries `answer` (`body`, what was written in reply; `actor`; `event_id`, the answer that records it; `answered_at`) |
| `POST /api/v1/approvals/:id/decision` | takes `decision` (`approve` or `decline`), an optional `note` and an optional `idempotency_key`; answers with `event_id`, the answer appended to the approval's thread, whose payload holds `body`, `decision` and, when one was left, `note` |
| `POST /api/v1/inbox/:id/read` and `.../unread` | `event_id`, the `status` the entry carries now, and `changed`, false when the entry was already there or carries no read state |
| `POST /api/v1/inbox/read-all` | `marked`, how many entries moved |

`count` is the hits the page carries, not how many documents match, and
`truncated` says when the limit cut it, so a capped page is never printed as a
total.

A hit's `snippet` is at most 200 characters of plain text. For an artifact, a
brain entry or a knowledge base page it is the opening of the indexed text.
For a feed hit it is what the agent wrote: the event payload's `body` when
that is a string, otherwise the event's summary, and never the payload's
serialized JSON. The corpus row of an event still holds the whole payload, so
a word anywhere in it finds the event; only what is shown changed, which is
why a store written before the change needs no rebuild.

The volume's capacity and free space come from one `statvfs` on the data
directory, and the free figure is what a writer that is not root can use. When
the call fails, `capacity_bytes` and `free_bytes` are `null` and the rest of the
response is still served, so a surface renders "unknown" rather than 0 of 0.
The numbers that cost a syscall or a file stat are memoised for ten seconds
behind a counter every write bumps; the counts are indexed and never cached.

The storage rows add up to `by_kind`. Sessions, artifacts and knowledge are
files and blobs that each belong to one project, so the rows' `session_bytes`,
`artifact_bytes` and `kb_bytes` sum to `by_kind.sessions`, `by_kind.artifacts`
and `by_kind.knowledge` exactly, and all three to `total_bytes`. The hub store
is one file every project shares, so `by_kind.events` is that file's size and
a row's `events_bytes` is what can honestly be given to one project: the bytes
of its events' summaries and payloads, the hub's own audit events about the
project included, since they go when the project does. The rest of the file (the indexes, the
search corpus, the inbox, the identity tables, free pages) is
`events_shared_bytes`, and the rows' `events_bytes` plus `events_shared_bytes`
is `by_kind.events`. The feed is weighed once, on the first report after a
start; later reports read only the events appended since, and the two things
that remove events, a committed prune and a project delete, start the weighing
over, as does every ten minutes. A report still weighing when events are
removed does not keep what it weighed, and a project that is gone has no row
whatever weights were held for it.

A decision may carry a note saying why. It is trimmed, a blank one is no
note, and one over 2000 characters is a 413 with `payload_too_large` that
decides nothing. The note is stored on the decision's feed event as
`payload.note`, beside the one-line `payload.body` that reads "Declined: ..."
wherever an answer is shown, and it comes back on the approval's inbox entry
as `decision.note`, for the human and for the agent that asked.

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
equally well on desktop. Mobile uses a tab bar of labelled icons ending in a
safe-area bottom edge. Desktop replaces the top bar with a permanent app rail:
a 56px icon rail at 720 to 1099px, and a 200px fixed rail from 1100px.

The rail provides vertical navigation with a brand header and node identity,
primary destinations (Home, Inbox with unread or waiting badge, and Search with
a keyboard shortcut hint), a PROJECTS section listing active projects with live
activity dots (filled dot for active agent work, ring for idle) and waiting count
badges, and a footer with Storage, Settings, and a sync indicator. Agent
personal spaces are omitted from the rail to avoid clutter, remaining accessible
on the Projects register screen (`#/projects`) without deletion controls.

Reading measure is constrained by moving the width cap off the main container
and onto prose containers at 640px. Thread bodies, artifact documents, wiki
pages, settings groups, and empty-state copy keep this measure, while lists,
tables, and multi-pane stages expand to fill available width.

The layout architecture defines four structural zones from left to right: Rail
(200px or 56px), Index (280 to 420px fixed: 280px for artifacts, 340px for
sessions, 420px for inbox and search), Stage (`minmax(640px, 1fr)`), and Aside
(320px). Breakpoints govern zone presence:
- Below 720px: Phone layout with bottom tab bar and stacked single column.
- 720 to 1099px: 56px icon rail with one pane at a time and 640px prose.
- 1100 to 1279px: 200px rail with index and stage; aside remains a header toggle.
- 1280px and above: Aside opens as a 320px column. Above 1600px the stage grows
  while the 640px prose block stays held left against the rail.

Panes are divided by 1px borders, scroll independently, and provide sticky
headers. Each project is an address of its own: the feed, the artifact gallery,
and the sessions list are segmented tabs under `#/projects/<id>/`, every one
with its own route, and the artifact viewer is a route too (`#/artifacts/<id>`),
so reload and the browser Back keep the artifact on screen.

| Screen | Purpose |
|---|---|
| Home | Today at a glance, from the one Home response: below the width the top bar shows at, the node line of the design's status strip (`node`, host and mode, in mono); a title that is the reader's own day and part of day over a summary line (waiting, unread, agents active); on phone widths (below 720px), the header carries a 36px gear icon linking to Settings (`#/settings`), which is omitted on desktop viewports where Settings resides in the app rail footer; a "Waiting on you" card with the queue's count, the first three of `waiting_items` (the head of the queue, newest first, whether or not they are among the newest events), each opening its own item, and "N more in the Inbox" worked out from `waiting` for the rest; "Newest across projects", each row naming its project by its display name (its slug when it has none) and linking to that project's feed, with the unseen dot drawn from the per-project cursor counts; and a storage card linking to Storage, with used against capacity, a bar, the same share in words, and what a prune would free. The bar's fill is to scale and never widened. A volume that cannot be measured shows the used bytes alone, with no bar. Under the Storage screen's threshold, 1% of the volume, the card draws no bar either and says so in the Storage screen's words: a fill to scale would be nothing to see, and Home holds one number, so a bar against what is used would always be full. When nothing waits, nothing is unread and nothing sits above a cursor, the two cards give way to the quiet empty state. On desktop widths (720px and above), Home stays at the 640px measure held left against the app rail rather than centered, with remaining width reading as natural margin. Waiting rows carry inline action controls directly on the row (Approve for pending approvals and Reply for questions) and hide the navigation chevron, making pending items actionable without navigating away. On mobile (< 720px), inline action buttons are hidden and rows retain the navigation chevron. |
| Projects index | At `#/projects`, the index listing every project the hub holds: an H1 with project count and total storage footprint, a 36px labelled hairline "New" button opening the project creation dialog (with the Settings gear having moved to the phone Home header), and a row per project naming it with active agent count, artifact count, and storage size, linking to that project's feed (`#/projects/<id>/feed`). Zero counts read as words ("no agents active", "no artifacts"). When the hub holds no projects, an empty state displays "No projects yet", explanation copy, a 48px primary "New project" button opening the creation dialog, and a note that agents can create projects on first write. The project creation dialog opens as a bottom sheet with a grab handle on phone viewports (< 720px) and a 480px modal on desktop, featuring a focused project name field, live-derived `/p/<slug>` row with an Edit button to unlock manual slug editing, inline conflict detection suggesting an available numeric suffix on 409 Conflict, and automatic navigation to the new project feed upon creation. A project with items waiting on the reader draws an amber badge; one with unread items draws an accent badge, with amber taking priority when both exist. Personal agent spaces (`space-...`) are folded into a collapsible disclosure so they do not crowd user projects. Rows are reachable by keyboard with a roving tabindex, and meet the 44px tap target floor. Below the list, a storage card shows overall disk usage with a proportional bar and ended sessions that can be pruned. |
| Project feed | What happened in one project, filterable by kind on 32px pills (13/500) in one scrolling row with 6px kind dots, sentence case labels, and counts appended ("All · 9", "Questions · 2", "Approvals · 1", "Finished · 3"). Kinds with no events are hidden, and Artifact and Session chips are dropped as they duplicate the tabs; addresses naming dropped kinds resolve to their respective tab. The selected chip carries an ink fill, and the tap target meets the 44px floor. Today and Yesterday are open; older days sit behind an "Earlier" disclosure that carries the hub's own count and pages back on the `next_before` cursor. Unseen events are marked from the project's read cursor, and viewing the feed moves it. Sibling artifact publishes from one agent inside two minutes collapse into one row ("published N artifacts"). Event verbs are lower case and past tense while the object stays as written, and signals stay sentences. An artifact row links to that artifact in the viewer within its project; rows with no destination (signals, finished notices, questions, answers, approvals, sessions, or deleted artifacts) remain unlinked. An empty feed offers "Copy MCP setup", which copies the connection details for this hub's origin and shows them to be copied by hand where the browser has no clipboard. On desktop at 1280px and above (and toggleable from 1100 to 1279px), the project feed displays a 320px aside divided into three sections: RIGHT NOW (active agents and live sessions with relative age), STORAGE (proportional bar across artifact blobs, session brains, and events, with reclaimable byte count and a Prune action), and LATEST ARTIFACTS (quick links with encryption badges and total count). Feed rows display inline Approve (on approval rows) and Reply (on question rows) controls directly on the row, paired with a 'Waiting on you' pill. |
| Inbox | The global queue in groups: "Waiting on you" with its count, its open items grouped by actor; "Snoozed" when items are deferred; "Unread" with its count; and "Earlier", read and resolved items, folded on the desktop with a count of both. A row carries the title, a one-line body on a waiting item (the event payload's `body`, when it is a string), and a footer of project and agent; the project goes by its display name there and on the open card, and by its slug when no name comes with it. An unread row is marked by a dot, its weight and the word beside the dot. An approval offers Decline and Approve and a question offers Reply; each decision is asked for in a dialog. A waiting item can be snoozed for 1 hour, remembered on the device; snoozing leaves Waiting on you, is listed in a Snoozed section with a control to bring it back, returns on its own when the hour expires, and is immediately undoable from a toast. Resolved items, including answered questions and decided approvals, appear under Earlier alongside read items, showing their outcome in words and decision note or answer body without decision controls. Opening a row shows the whole item as a card, with the answers or decision note at full size and a question's composer held open; the open item and the filter live in the address. The card carries its own way out, "Back to inbox" on a phone and "Close" beside the list on the desktop, named by the words it shows; Esc closes it too, except while the card holds a half-written answer, and only while the Inbox is the screen. Closing hands focus back to the row the card was opened from, or to the Earlier disclosure when that row is folded under it, as the row of an item read by opening it is on the desktop. Esc and the card's own control close it the same way: the card's address is replaced, so Back does not reopen it. The header carries "Unread only" and "Mark all read", and a last-synced line with a Refresh control. A swipe right marks a row read or unread, a swipe left uncovers a waiting row's actions, and a pull down refreshes. Quick answers on a question are intended design, not yet shipped. The Approve and Decline dialogs carry an optional field, "Add guidance with your decision", whose trimmed text is sent as the decision's `note`; a blank one sends no `note`. The safe action is still focused first and the field is in the dialog's tab ring. From nine tenths of the 2000 character limit a count shows under the field, and says in words when the note is past it; the field has no hard cap, so a paste is never cut short. The decision is sent with the dialog open: a note the hub refuses as too long is said beside the field, the words stay and nothing is decided, and any other refusal is said there too; the refusal goes once the note is edited. The count is a polite live region. Esc closes the dialog as "Not now" only while the field is empty; over a written note it keeps the dialog and says so. The project feed shows a decision's note on the row of the decision's event, after the word Approved or Declined. |
| Artifacts | A per-project gallery and compact viewer: the gallery presents a list grouped by Day (default), Agent, or Kind with counts in group headers on mobile, a 5-up card grid at desktop widths with Cards/Table view segment, a `Group` trigger button with value pill, and counts in mono (`N artifacts · N versions · X.X GB`). Card previews display the artifact's own first lines at 9px mono as an unselectable, aria-hidden ornament whose text is fully present in the title, meta, or document. Encrypted artifacts show the lock tile rather than ciphertext. The viewer opens into a three-pane desktop layout: a 280px index column for reading runs of artifacts, a 640px document measure, and a 320px comments margin column that prevents the document from shifting when comments open. Thread heads keep the mono anchor quote, and resolved threads carry a check glyph and the word Resolved rather than dimming alone. The viewer chrome provides a 44px top row carrying the back chevron, mono path (`project / slug`), start-a-thread or comments glyph, copy-raw, and overflow. Comments glyph carries its count optically centred inside the glyph. Copy raw confirms via toast. The overflow menu provides Start a thread, Comments, Copy raw, Copy path, Copy link, Share, and Open in browser. The meta line orders author, version, size, and age, where the version button opens the version sheet with whole-save history. The document renders its H1 at 24px semi-bold, starting prose at 104px. Body line-height rises from 1.6 to 1.7 when comments exist. Open comments on the version being read highlight their quoted text with a tint and 2px underline; point anchors place a pin in the gutter. On mobile, tapping a highlight opens a bottom sheet with resolve control and one level of reply; comments toggle opens the list drawer with newest first and resolved comments collapsed. Older-version comments carry a direct link to open that version with anchor placed; re-anchoring is excluded by design. Agent and human authors are distinguished visually. Public markdown artifacts render server-side via `src/markdown.rs` with strict escaping; protected markdown artifacts render client-side via vendored `marked.js` with total-escape contract while supporting callouts and mermaid diagrams. Protected artifacts refuse plaintext text anchors. Chrome links are never underlined, and theme follows system preference with override in Settings. |
| Sessions | Sessions per project, grouped into active and ended sections with counts and a "Prune all · [size]" control on the ended header (confirmed in a dialog and undoable for 30 seconds). Session rows keep a 44px height and two-line structure carrying the middle-truncated session id and working name on the title line, state dot and textual status, an owner-leading meta line with relative timestamp, right-column mono size, and a stretched link making the entire row pressable. Desktop view renders a four-zone layout: navigation rail, 340px session index, 300px brain tree pane, and a file viewer taking the remaining stage width. The brain tree sits left of the file content inside the stage, surviving file switches and rendering long file paths on one line without truncation. Phone width replaces the list with the detail view when opened, offering a back link that restores focus to the opened row. Detail view displays the handoff note directly in the header rather than behind a disclosure, removes separate stat cards in favor of a single unified meta line, middle-truncates the session ID in a 32px copy control with full ID copied to clipboard and 44px tap target, unifies keys and files into a single brain tree with expandable `kv/` and `fs/` folders and leaf names only (clearing selection on blur and without default selection), and pairs a single primary action button (End session or Prune session) with a helper sentence replacing disabled buttons. |
| Search | One field over feed, artifacts, and session brains, answering as it is typed. The query and the scope live in the route (`#/search?q=&type=`), so a reload or the browser's Back lands on the same results, and an answer that arrives after a newer query is dropped. Scope chips filter by the route's `type` on 32px pills (13/500) in one scrolling row with sentence case labels (All, Feed, Artifacts, Sessions); the selected chip carries an ink fill, the tap target meets the 44px floor, and scopes show no count where none is known before a query runs. When a project filter is active, a project pill carrying an × follows a hairline divider and dismisses on click. A results line gives the hub's own `count` and `took_ms` and is announced politely; a page the limit cut is called the first of more, never a total. Results are grouped by family with their counts in one unified list rather than tabs, and each row carries the title, the snippet with the matched words marked, the project by its display name (its slug when it has none), and when it changed. On desktop from 1100px, Search renders a two-pane layout with the permanent rail, a 420px results index, and a preview stage. Selecting a result previews it immediately in the preview stage without opening, allowing the reader to decide without navigating away. The preview stage header displays the path, a match counter ("match 1 of 4") beside previous and next step buttons, and an Open link to the item. The active match carries `--accent-bg` and a 1px accent ring, while other matches carry background only. The screen sends the query as it was typed, with its case, quotes and operators, and the hub makes it safe for the index: matching is by prefix with whole-word matches ranked first, a balanced quoted phrase is searched as a phrase, and other punctuation and the bare operators are left out, so no query is refused. The marked snippet is built from text nodes, so neither a query nor a snippet is read as markup. A date scope and landing on the hit inside its destination are intended design, not yet shipped. A row's last line says what the hit is, in words: a feed hit draws the kind badge of its event, with the kind named for a screen reader, and names its actor; an artifact hit gives its current version and size; a session brain hit names its session and says whether it is active or ended. A key a hit does not carry adds nothing to the line. |
| Storage | The data path and node, what is used against the volume's own capacity, and a stacked bar by kind (events, sessions, artifacts, knowledge) whose legend carries each kind's byte figure, so the bar is never the only place a number lives. The bar is drawn against the volume; when under 1% of the volume is used the whole of it would be a few pixels at most, so it is drawn against what is used instead and says so beside it and in its text alternative. No segment is ever widened. A row per project, titled with its display name (its slug when it has none) and naming it the same way in the prune control and the dialogs, shows its total, its own 4px bar split four ways to scale (events, sessions, artifacts, knowledge) and the same split in words, and a Prune button with the bytes its ended sessions would free. A Prune all card reviews the affected projects, session counts and bytes in a dialog first. Either prune is one request, confirmed with Keep focused first, and undoable from the toast for 30 seconds. A hub whose projects hold nothing, not even events of their own, shows the empty state instead. Pruning a single session stays on the Sessions screen. A row's total is the sum of its four figures. Under the legend the summary card says what `events_shared_bytes` is: the part of the legend's events that is hub database every project shares, which is why the rows' events add up to less. A project a prune has just emptied still holds its events and keeps its row; projects whose four figures are all zero, with nothing to prune, are folded under a count ("2 projects hold nothing") with a link to each, so a run of empty rows does not crowd the list and no project is out of reach. On desktop viewports from 720px, the screen shifts to a full-width multi-column table preceded by four summary tiles: ON DISK, ARTIFACT BLOBS, SESSION BRAINS, and RECLAIMABLE (styled with an action border and a Prune all action button). The table replaces mobile drill-down cards, displaying columns for Project, Share (a proportional bar with accessible labelled numbers), Total, Blobs, Brains, Reclaimable, Last Write, and Prune. Projects with no ended sessions show a dash in Reclaimable rather than 0 B, and Prune buttons appear solely on rows with reclaimable bytes. Free space on the host volume is not drawn, backed by an explanatory footnote. |
| Settings | At `#/settings`, the global settings screen. It is organized into four groups with seven controls total: Appearance, Alerts, Access, and This browser. There is no search, and no sub-pages except Access. Appearance provides segmented controls for Theme (System, Light, Dark) and Density (Comfortable, Compact, with pointer-derived helper and segment copy for touch or mouse), plus a switch toggle for single-key shortcuts. Alerts shows one of four states: Not asked yet (default, with an explicit enable action), Granted (with master switch and three kind toggles), Blocked (with instructions and Check again), or Unsupported (information only). Access summarizes live caller tokens and links to the full Access management screen (`#/access`). This browser identifies that the access token is stored locally, states that signing out forgets it here and nowhere else while other browsers and agents remain unaffected, and provides an ink Sign out button behind confirmation. On mobile (390px) groups stack vertically with 12px mono uppercase labels 8px above cards; on desktop (1100px) the screen uses a two-column layout with 132px label column and 560px cards, 28px page heading, and inline controls. |
| Connect | At `#/connect`, the screen that asks for the access token. A screen the hub refuses to paint sends the reader here, carrying the route it interrupted, so a token entered is followed by the screen they were going to rather than the start. A refused write is not a refused screen: it says so where it was pressed and leaves the reader where they are. The route it carries is followed only when it is one route of this app, and never this screen itself. One password field with a "Show token" control, a hidden username field so a password manager stores the pair, and the copy names where the token comes from. On desktop the card is vertically centred in the available space. The token is checked against the hub before it is kept, so a token the hub refuses never becomes the one every later screen sends. A refusal shows the hub's own words in a live region beside the field, keeps what was typed and returns focus to it, and stores nothing. A browser that refuses to store the token says so rather than asking again on the next load with no explanation. Settings has no field of its own; it links here to change a token. Its sections catch their own refusals, so Settings stays reachable without a token and the way back in is one link away. |
| Project settings | Reached from the gear in a project's header, at `#/projects/<id>/settings`. The name is an editable field; the slug is shown in mono as text, not as a field, because it is read-only after creation. Save stays disabled until something differs from the hub's copy and sends only what differs. A blank name is caught on the screen, and a name the hub refuses is reported beside that control with the hub's own reason while the form keeps what was typed. Leaving with edits pending asks first, in the confirmation dialog. Retention is a reserved card that says automatic pruning is not in v1 and links to Storage; it carries no control. Delete project opens the confirmation dialog, and is not offered for an agent's personal space. |

Access management lives on its own screen reached from Settings (`#/access`),
linked from an Access row carrying the 17px ID-card glyph.
It displays the admin token with a copy control, stating that it originates from
startup configuration and changes on restart with a different `HUB_ADMIN_TOKEN`.
Confidential projects are absent rather than refused. Agents that identified
themselves are displayed as records with their personal space path, alongside
controls to reissue or revoke their token and remove project grants under a
confirmation dialog, and a list of revoked tokens as history. Agent records
lead nowhere and carry no capability switches. The screen provides forms to
create an agent by id and display name, and to grant an agent access to a
project under binary assignment without read or write tiers. The artifact
viewer embeds the artifact page:
the host shell shows an unlock form for a protected artifact and decrypts in
the browser, then renders HTML or rendered markdown inside the same sandboxed
frame. Markdown renders in the page with raw HTML in its source escaped;
protected markdown renders from the decrypted source, so the server never sees
it.

The session listing route carries the agent that owns each session, the
handoff note its last owner left, and its lineage: whether the work was adopted
or forked, from which session and which agent, and whether that source has since
been pruned. The PWA does not render those fields yet; that is intended design.
A session opens into a detail view that lists its brain keys and files
and offers End and Prune. Reassigning a session to another agent is the
human's move for an agent that is not coming back, over the reassign route; the
PWA has no control for it yet. Agents pick work up themselves and ask nobody. Project deletion is a destructive action on the project's own settings screen, behind a confirmation; an agent's personal space cannot be deleted. Inbox notifications are configured from Settings under Alerts, request permission only on explicit enable action, and carry only waiting-on-you items; without permission or support they degrade silently. True background push is deferred, by
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
module the hub does not serve cannot ship. Offline reading for the knowledge
base is to be evaluated: what is cached today is the app shell and its assets,
while page content is not. Each render carries a number, and
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
and no colour-only meaning. A route change moves focus to the new screen's own
heading, so a screen reader announces where the reader has arrived and the
focus ring frames that heading rather than the whole region; a screen that has
already placed focus inside itself keeps it. Swipe gestures are always non-destructive and
always have a tap equivalent; swipe-down to dismiss a toast ships, with a
dismiss control and Esc beside it, and so do the Inbox row swipes and its pull
to refresh. The edge swipe back, the tab swipe and swipes on feed rows are
intended design, not yet shipped. See the
[human interface](../design/human-interface.md) for the tokens and rules.

Component radius follows visual hierarchy: a container is rounder than what it
contains, and a child touching the container's edge is square. A pill is a value
rather than a door: `--r-pill` represents tokens, filter chips, counts, and
status pills, whereas any control that opens a surface (a menu, sheet, or
popover) is an `--r-1` button with a caret glyph. A trigger's label is a fixed
word (such as "Group") rather than its current value, with the selected value
displayed beside the trigger as a pill chip so that changing selections never
resizes the control or shifts adjacent layout. The shared glyph set provides
17px to 20px inline SVG line icons for actions and identity, including the
ID-card glyph for Access navigation, the chevron down caret, trash, sign-out,
bell, bell-off, and check glyphs.

## Auth

- The control surface uses a config-set admin token. It is required when the
  bind is not loopback, since the surface rejects every request without one.
- The MCP endpoint uses bearer tokens. A token is an identity in its own
  right, and grants are binary per project. The stdio transport is the local
  admin and needs no token.
- Shared artifacts use a password and browser-side encryption, so the
  recipient needs nothing else.
- The PWA asks for that token on the Connect screen, and only there, so a
  token is never kept without the hub having accepted it first. Under "This browser",
  Settings identifies that this browser holds the token, states that signing out
  forgets it here and nowhere else while other browsers and agents are unaffected,
  beside an ink Sign out button which asks first and forgets the token rather than
  storing an empty one. Saving a preference does not touch the token. The hub answers a token it does not know and a hub with no token
  configured with the same code, so the screen shows the hub's sentence rather
  than guessing which of the two it met.
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
