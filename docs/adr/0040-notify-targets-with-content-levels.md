---
type: Decision Record
title: Notify targets with presets and content levels
description: Why the one contentless notify URL becomes a list of targets the operator adds, each with a service preset, the events it takes and how much a notice may say, and why every target still starts at a fixed sentence.
tags: [adr, notifications, privacy, operations]
status: stable
---

# 0040. Notify targets with presets and content levels

## Context

[ADR 0026](0026-contentless-notify-target.md) gave the hub one outbound URL
that receives one fixed sentence when something starts waiting. It works for an
operator who runs ntfy, and for nobody else without a bridge: Gotify, Matrix and
the chat services each want their own payload. The sentence also cannot say
which project is asking, so an operator with several busy projects opens the
app to find out every time.

Two of the services operators ask for, Slack and Discord, are vendor clouds. The
node is the cloud ([operating model](/architecture/operating-model.md)), and no
brain leaves it, so a target there must not quietly receive more than the
operator chose.

## Decision

The hub keeps a list of notify targets. The operator adds them from Settings or
the admin REST surface; `notify_url` stays and is listed as a read-only target.

- **A preset is a payload shape and a URL pattern.** Webhook (a stable JSON
  body), ntfy, Gotify, Matrix, Slack, Discord, Telegram and Pushover. Each is a
  few lines that build one request; there is no SDK and no new crate. The
  sender is the hyper client 0026 introduced.
- **A content level per target.** `none` sends the event's fixed sentence and
  nothing else, as 0026 did. `title` adds the kind, the project, the session
  name and a link to the item's short address on `public_url`. `full` adds the
  question or approval text. Every preset starts at `none` except Gotify, which
  has no hosted service and so is always the operator's own.
- **A confidential project is always sent at `none`.** A grant gate on the node
  is not undone by a chat message.
- **Brains never leave.** No kv, file, knowledge base page, artifact or handoff
  note is sent at any level.
- **Three events.** An ask starts waiting (as 0026), an open item nears its
  deadline (once per item, recorded on the item), and an agent ends a session.
  A target takes the first two by default.
- **Delivery stays off the write path.** A write hands a notice to a
  background task and returns. Each target sends at most once per
  `notify_interval_secs` and folds what arrives meanwhile into one trailing
  send, drops a notice it already sent, and retries a transport failure, a 429
  or a 5xx twice with a backoff. The outcome of the last send is kept per target
  and shown to the operator.
- **The URL is a secret.** Slack, Discord, Telegram and Pushover carry their
  key in the URL, so the hub stores the URL and the token in `hub.db` and never
  reads either back out: reads show the origin and whether a token is set, and
  logs carry the origin only.

## Consequences

- An operator can point the hub at the service they already use, and choose
  per target how much it learns. A hub with no targets sends nothing, and one
  with only `notify_url` behaves as under 0026.
- A notice at `title` or `full` puts project and session names, and at `full`
  the text of an ask, on a service the operator chose. The interface says so
  when the service is a vendor cloud.
- A target's secrets are in `hub.db`, so they are in every backup of it.
- Delivery status lives in memory and starts empty after a restart.
- Answering or approving from a chat service, routing by project, templated
  bodies, email and Web Push are out of scope. 0016's decision against a vendor
  push service still holds.
