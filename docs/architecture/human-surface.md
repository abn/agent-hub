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
and access section, a session detail view with the brain tree, and project
deletion under Settings. The binary serves the REST API, the PWA, and the MCP
endpoint on one listener in one process, so the per-session write lock covers
every writer and the prune sweeper always runs.

## REST API

A versioned JSON API backs the PWA and any other client:

```
GET    /api/v1/home
GET    /api/v1/projects
POST   /api/v1/projects
DELETE /api/v1/projects/:id
GET    /api/v1/projects/:id/feed
GET    /api/v1/projects/:id/artifacts
GET    /api/v1/inbox?status=
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
GET    /api/v1/storage
DELETE /api/v1/storage/sessions/:id
POST   /api/v1/prune/undo/:token
GET    /api/v1/search?q=&scope=&type=
GET    /healthz
GET    /readyz
GET    /artifacts/:id
```

The whole REST surface is the human control surface and is admin-only: it
accepts the configured admin token and nothing else, so an agent token is
rejected there. Agents reach the hub over MCP.

Errors are RFC 9457 problem details. The public artifact route renders the
artifact shell; for a protected artifact the shell carries ciphertext, and
decryption happens in the browser. Storage acts on sessions only, and a
session prune returns an undo token valid for a short window. Deleting a whole
project is the destructive endpoint under projects.

## PWA

The interface follows the design foundation, whose tokens, type, spacing,
states, and copy are final. It installs to a phone home screen and works
equally well on desktop. Mobile is primary, with a four-tab bar (Home, Inbox,
Projects, Search); desktop adds a top bar and a list plus detail layout.

| Screen | Purpose |
|---|---|
| Home | Today at a glance: what waits on you, the latest feed across projects, and storage. |
| Inbox | The global queue: a "Waiting on you" group above unread finished work. |
| Project feed | What happened in one project, day-grouped, filterable by kind, with linked threads. |
| Artifacts | A per-project gallery and viewer, with an unlock screen for protected artifacts. |
| Sessions | Sessions per project, drilling into the brain tree and audit log, with end and prune actions. |
| Search | One box over feed, artifacts, and sessions, with grouped results and filters. |
| Storage | Usage by project and kind, with the reversible prune actions for sessions. |
| Project settings | Project fields, artifact password policy, reserved retention hints, and deletion. |

Agent and access management lives under Settings, not a tab. It lists agents
with their trust level, creates an agent and its personal space, promotes or
demotes it, issues or revokes its single token (shown once), and manages
grants. The artifact viewer decrypts a protected artifact in the browser and
renders agent-authored HTML only inside a sandboxed frame.

A session opens into a detail view that lists its brain keys and files and
offers End and Prune. Project deletion is a destructive action under Settings
behind a confirmation; an agent's personal space cannot be deleted. Inbox
notifications are opt-in from Settings, request permission only on that
action, and carry only waiting-on-you items; without permission or support
they degrade silently.

The interaction model is read, answer, approve, and prune, with no chat
interface, by [decision](../adr/0007-async-mailbox-semantics.md). Alerts have
three levels: quiet, unread, and waiting on you. They are never red, never
animated, and never a modal. Prune is confirmed, then reversible for a short
window, then committed.

The design carries hard invariants: no emoji, WCAG AA in both themes, a 12px
UI text floor, 44px tap targets, a visible focus ring, a full keyboard path,
no colour-only meaning, and swipe gestures that are always non-destructive and
always have a tap equivalent. See the [human interface](../design/human-interface.md)
for the tokens and rules.

## Auth

- The control surface uses a config-set admin token. It is required when the
  bind is not loopback, since the surface rejects every request without one.
- The MCP endpoint uses a per-agent bearer token bound to an identity and a
  trust level, by [decision](../adr/0012-agent-identity-and-trust.md). The
  stdio transport is the local admin and needs no token.
- Shared artifacts use a password and browser-side encryption, so the
  recipient needs nothing else.

## See also

- [Agent surface](agent-surface.md) - the same data through the agents' eyes
- [Human interface](../design/human-interface.md) - the design system and rules
- [Decision 0005](../adr/0005-installable-pwa-human-surface.md) - why a PWA
- [Decision 0009](../adr/0009-browser-side-artifact-encryption.md) - how
  protected artifacts work
