---
type: Decision
title: Deliver notifications on the tool result, not by polling
description: Why the hub carries what needs an agent's attention on the next tool result it makes, and offers standing subscriptions, instead of a push channel or a per-harness poll.
tags: [adr, mcp, notifications, agents]
status: stable
---

# 0024. Deliver notifications on the tool result, not by polling

## Context

Agents have no push channel. The hub exposes a freshness stream
(`GET /api/v1/stream`, [ADR 0016](0016-push-notifications-deferred.md)) for the
human's PWA, but an agent over MCP has only request and response. Until now the
only way an agent learned that its question was answered or its approval
decided was to call `inbox_read` or `inbox_wait`, and the only way it learned
of a feed event was `feed_read` with its own cursor.

That put the notification logic in each harness. A client that wanted to watch
artifact comments wrote its own poller with its own seen-file, and every client
would reinvent the same timer, cursor, and de-duplication differently. The
notification mechanism belongs in the hub, which already holds the events, the
threads, and the freshness signal.

## Decision

Every successful MCP tool result may carry a top-level `notifications` member
whose `pending` list holds what needs the caller's attention. It is the
delivery channel, and it has two sources:

- **Attention.** An answer to a question the caller posted, or a decision on an
  approval it posted, that it has not been shown. This is on by default: an
  agent that never asks is still told when its own question is answered.
- **Subscriptions.** `notify_subscribe` registers a standing interest in feed
  events by kind, optionally scoped to one project, and `notify_unsubscribe`
  removes it. A subscription is the `feed_read` a client would otherwise poll
  for, kept on the hub's side.

Each item is `{source, kind, id, project_id, title, at}`. It is a nudge, not
the record: the answer body, the decision note, and the full event stay in the
inbox and the feed, read by `inbox_read`, `inbox_wait`, or `feed_read`.

Each item is delivered once. Two server-side cursors, keyed on the resolved
actor and never the token, record what has been shown: one for the attention
queue, one per subscription. A delivered read advances its cursor; an empty
read moves nothing. The member is absent when there is nothing to report and on
an error result, so a caller that never has anything pending pays one indexed
read and nothing more. `notify_subscribe` seeds its cursor at the newest
matching event, so a subscription reports from the moment it was made rather
than replaying history, and an unscoped drain keeps to the projects the caller
may read at drain time, so an event in a project it may not read is not
consumed and arrives if a grant is later given.

The member rides on the result of a call the agent was making anyway. It is not
a push: nothing is delivered between calls, and an agent that stops calling is
not notified. `inbox_wait` remains for an agent that is idle and wants to block
until something lands.

## Consequences

- The hub owns notification, so a harness needs no timer, no cursor, and no
  seen-file. Wiring a harness to the hub is one MCP connection.
- The trailer is a nudge and at most once. An agent that ignores it does not
  lose the record: `inbox_read` and `feed_read` keep their own cursors and
  return everything. This is why the item carries an id and a title rather than
  the body.
- A tool result now varies with the caller's pending state, so a test that
  asserts an exact result shape must account for the member. It is absent when
  there is nothing pending, which is the common case.
- Two migrations (21 and 22) add the cursors and the subscriptions, both keyed
  on the resolved actor, so a rotated token keeps its read position.
