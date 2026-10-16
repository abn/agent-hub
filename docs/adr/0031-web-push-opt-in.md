---
type: Decision Record
title: Opt-in Web Push for waiting items
description: Add optional browser Web Push for items the human has waiting, while keeping ADR 0016's contentless nudge and async mailbox intact.
tags: [adr, notifications, pwa, push, privacy]
status: draft
---

# 0031. Opt-in Web Push for waiting items

## Status
Proposed (companion to [ADR 0016](0016-push-notifications-deferred.md) and
[ADR 0026](0026-contentless-notify-target.md)).

## Context
ADR 0016 defers background push so that no third party sits in the transport
path and no secure context is assumed. Since then the hub gained deadlines on
open items (ADR 0027) and a contentless nudge target (ADR 0026). An installed
PWA that is fully closed still raises nothing, which is the single largest
source of latency in the human loop for a homelab operator who is away from the
desk.

## Decision
Offer Web Push as an *opt-in* operator setting, off by default:

- The operator enables it with a VAPID key pair stored on the node and serves
  the hub over a secure context (tailnet HTTPS or a locally trusted
  certificate). A plain-HTTP LAN deployment cannot enable it, and the hub
  reports that clearly.
- A subscription is per device and revocable in Settings. The node stores only
  the subscription endpoint and its renewal timestamp.
- A push carries *no content*: a fixed sentence ("agent-hub: a decision is
  waiting") plus the waiting count, end-to-end encrypted by the browser's own
  mechanism. The payload is recovered only by opening the app, so a push
  service learns nothing beyond timing.
- Push fires only on items the human asked to be notified about or that carry
  an imminent deadline; snooze and per-item mute live in the app.
- The in-app freshness stream (ADR 0016) remains the primary channel for an
  open app; push is the wake-up nudge only.

## Consequences
- ADR 0016's local-first invariant is preserved: the transport is a dumb
  conduit and the plaintext never leaves the node.
- Deployments without a secure context degrade gracefully to the current
  pull-based mailbox, which remains complete on its own.
- Adds a key-management surface (VAPID generation, rotation, revocation) to
  the operator's runbook and to the enrolled-token lifecycle.
- Increases the hub's scheduled work: subscription expiry handling and
  deadline checks on the waiting set.
- Supersedes ADR 0016 only when the operator opts in; otherwise 0016 stands.
