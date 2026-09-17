---
type: Decision Record
title: Push notifications are deferred beyond v1
description: The human surface keeps an in-app notification and a freshness stream instead of a browser push service.
tags: [adr, notifications, pwa, privacy]
status: stable
---

# 0016. Push notifications are deferred beyond v1

## Context

The human wants to know when an agent waits on a decision without watching the
app. True background delivery, with the installed app closed, requires a
browser push service. The browser chooses it (FCM, APNs, autopush) and the hub
cannot substitute a service of its own, so a third party sits in the transport
path. That sits badly with the local-first invariant that the node is the
cloud. It also needs a secure context, so a plain-HTTP LAN deployment cannot
use it at all, and the embedded tailnet carries no certificate issuance.

## Decision

v1 ships no background push. The human surface keeps the opt-in in-app
notification, raised while the app runs, and adds a server-sent freshness
stream so an open app refreshes its waiting badge as soon as a write lands
instead of polling. The mailbox stays pull-based.

## Consequences

- No vendor push service and no third party in the path; the token, the
  payload, and the timing stay on the node.
- An installed app that is fully closed raises nothing. The operator learns of
  a waiting item when the app next opens.
- The freshness stream needs a live page and carries no event data, only a
  nudge to refetch, so it is not a chat channel and does not change the
  asynchronous interaction model.
- A later revision can add opt-in Web Push with a contentless, end-to-end
  encrypted payload if a vendor transport is accepted. That would supersede
  this record.
