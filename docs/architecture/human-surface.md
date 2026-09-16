---
type: Reference
title: Human surface
description: The REST API, the installable PWA, and the v1 auth model.
tags: [architecture, pwa, rest, human]
status: draft
---

# Human surface

The human reaches the hub through a REST API and an installable,
mobile-first PWA served as static assets from the same binary. Everything
here is intended design; none of it ships yet.

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
POST   /api/v1/sessions/:id/end
GET    /api/v1/sessions?project=
GET    /api/v1/agents
POST   /api/v1/agents
PATCH  /api/v1/agents/:id
GET    /api/v1/agents/:id/grants
POST   /api/v1/agents/:id/grants
DELETE /api/v1/agents/:id/grants/:projectId
GET    /api/v1/storage
DELETE /api/v1/storage/sessions/:id
POST   /api/v1/prune/undo/:token
GET    /api/v1/search?q=&scope=&type=
GET    /healthz
GET    /readyz
GET    /artifacts/:id
```

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

Agent and access management is a sub-screen of Project settings on mobile and
a Settings section on desktop, not a tab. It lists agents with their trust
level, issues tokens, and manages grants.

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

- The control surface uses the tailnet identity when running on a tailnet,
  reverse-proxy auth on a LAN, or a config-set admin token.
- The MCP endpoints use per-agent bearer tokens bound to an identity and a
  trust level, by [decision](../adr/0012-agent-identity-and-trust.md).
- Shared artifacts use a password and browser-side encryption, so the
  recipient needs nothing else.

## See also

- [Agent surface](agent-surface.md) - the same data through the agents' eyes
- [Human interface](../design/human-interface.md) - the design system and rules
- [Decision 0005](../adr/0005-installable-pwa-human-surface.md) - why a PWA
- [Decision 0009](../adr/0009-browser-side-artifact-encryption.md) - how
  protected artifacts work
