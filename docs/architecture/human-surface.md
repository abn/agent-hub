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
GET    /api/v1/projects/:id/feed
GET    /api/v1/projects/:id/artifacts
POST   /api/v1/artifacts/:id/versions
GET    /api/v1/inbox
POST   /api/v1/questions/:id/answer
POST   /api/v1/sessions/:id/end
GET    /api/v1/sessions
GET    /api/v1/storage
DELETE /api/v1/storage/sessions/:id
DELETE /api/v1/storage/projects/:id
GET    /api/v1/search
GET    /artifacts/:id
```

The public artifact route renders the artifact shell. For a protected
artifact the shell carries ciphertext, and decryption happens in the browser.

## PWA screens

The PWA installs to a phone home screen and works equally well on desktop.

| Screen | Purpose |
|---|---|
| Home | Today at a glance: unread count, latest feed across projects, storage meter. |
| Inbox | The human's global queue: unread finished work, and action or waiting items for approvals and questions. |
| Project feed | What happened in one project, newest first, filterable by kind, with a detail view and linked threads. |
| Artifacts | A per-project gallery and viewer, with a password entry screen for protected artifacts. |
| Session explorer | Sessions per project with status, last activity, and size, drilling into the brain tree and audit preview with a prune action. |
| Search | One box over feed, artifacts, and sessions, with grouped results and filters. |
| Storage | Usage by project and kind, with the explicit prune actions. |
| Project settings | Create and edit projects, artifact password policy, and reserved retention hints. |

The interaction model is read, answer, approve, and prune. There is no chat
interface with agents in v1, by [decision](../adr/0007-async-mailbox-semantics.md).
Everything is asynchronous and calm, and the PWA notification channel is the
signal when something needs the human.

## Auth

- The control surface uses the tailnet identity when running on a tailnet,
  reverse-proxy auth on a LAN, or a config-set token. It stays minimal.
- The MCP endpoints are authenticated by their transport. Stdio is local
  trust; streamable HTTP uses a tailnet allowlist or an API token.
- Shared artifacts use a password and browser-side encryption, so the
  recipient needs nothing else.

## See also

- [Agent surface](agent-surface.md) - the same data through the agents' eyes
- [Decision 0005](../adr/0005-installable-pwa-human-surface.md) - why a PWA
- [Decision 0009](../adr/0009-browser-side-artifact-encryption.md) - how
  protected artifacts work
