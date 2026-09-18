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
Settings. The session detail view lists brain keys and files as a flat list
today; a tree with expand, collapse, and keyboard navigation is intended
design, not yet shipped. The binary serves the REST API, the PWA, and the MCP
endpoint on one listener in one process, so the per-session write lock covers
every writer and the prune sweeper always runs.

## REST API

A versioned JSON API backs the PWA and any other client. A request body is
capped at 4 MiB:

```
GET    /api/v1/home
GET    /api/v1/projects
POST   /api/v1/projects
DELETE /api/v1/projects/:id
GET    /api/v1/projects/:id/feed
GET    /api/v1/projects/:id/artifacts
GET    /api/v1/inbox?status=&project=&limit=
GET    /api/v1/stream
POST   /api/v1/questions/:id/answer
POST   /api/v1/approvals/:id/decision
POST   /api/v1/sessions/:id/end
GET    /api/v1/sessions?project=
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

## PWA

The interface follows the design foundation, whose tokens, type, spacing,
states, and copy are final. It installs to a phone home screen and works
equally well on desktop. Mobile is primary, with a four-tab bar (Home, Inbox,
Projects, Search); desktop adds a top bar and shows the same single-column
screens. A desktop list plus detail layout is intended design, not yet
shipped.

| Screen | Purpose |
|---|---|
| Home | Today at a glance: what waits on you, the latest feed across projects, and storage. |
| Inbox | The global queue: a "Waiting on you" group, its open items grouped by actor, above unread finished work. |
| Project feed | What happened in one project, day-grouped, filterable by kind, with linked threads. |
| Artifacts | A per-project gallery and viewer: the viewer embeds the artifact page with its unlock form, themes, and version picker, plus a comments drawer with compose, resolve, and delete. |
| Sessions | Sessions per project, with end and prune actions. Session detail lists brain keys and files as a flat list today; a drill-down brain tree and an audit log are intended design, not yet shipped. |
| Search | One box over feed, artifacts, and sessions, with grouped results and filters. |
| Storage | Usage by project and kind, with the reversible prune actions for sessions. |
| Project settings | Deletion ships, under the global Settings screen. A dedicated Project settings screen with project fields, artifact password policy, and reserved retention hints is intended design, not yet shipped. |

Agent and access management lives under Settings, not a tab. It lists agents
with their trust level, creates an agent and its personal space, promotes or
demotes it, issues or revokes its single token (shown once), and manages
grants. The artifact viewer embeds the artifact page: the host shell shows
an unlock form for a protected artifact and decrypts in the browser, then
renders HTML or rendered markdown inside the same sandboxed frame. Markdown
renders in the page with raw HTML in its source escaped; protected markdown
renders from the decrypted source, so the server never sees it.

A session opens into a detail view that lists its brain keys and files and
offers End and Prune. Project deletion is a destructive action under Settings
behind a confirmation; an agent's personal space cannot be deleted. Inbox
notifications are opt-in from Settings, request permission only on that
action, and carry only waiting-on-you items; without permission or support
they degrade silently. True background push is deferred, by
[decision](../adr/0016-push-notifications-deferred.md); an open app reads the
freshness stream and refreshes its waiting badge on a tick, with a slow poll
as the fallback.

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
window, then committed.

The design carries hard invariants: no emoji, WCAG AA in both themes, a 12px
UI text floor, 44px tap targets, a visible focus ring, a full keyboard path,
and no colour-only meaning. Swipe gestures that are always non-destructive and
always have a tap equivalent are intended design, not yet shipped; no swipe or
touch gesture exists today. See the
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
