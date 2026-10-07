---
type: Decision
title: An agent may put a deadline on the item it asks
description: Why an agent posting a question or an approval may say how long it can wait, why the hub then resolves that one item as itself, and why this is not retention.
tags: [adr, inbox, approvals, questions, agents]
status: stable
---

# 0027. An agent may put a deadline on the item it asks

## Context

A question or an approval waits on the human
([ADR 0007](0007-async-mailbox-semantics.md)). When the human is away, the
agent that asked is stuck: it can wait, poll, or give up and guess, and none of
those leaves an honest record of what happened. Many asks have a natural
deadline: a maintenance window, a release train, a job that is cheap to skip.
The agent knows it; the hub did not.

The hub ships no automatic expiry, and the human is the garbage collector
([ADR 0004](0004-manual-pruning-in-v1.md)). A feature that resolves items
without the human has to stay clear of that rule rather than wear it down.

## Decision

An agent may pass `expires_in_seconds`, from 60 seconds to 30 days, when it
posts a question or an approval. An approval may name `on_expiry` as `approve`
or `decline`, and declines when it names nothing, because going ahead unasked
is the outcome that costs more to undo. A question closes with no answer.

If the item is still open when the deadline passes, the hub resolves it: it
appends an `answer` on the item's thread with `hub` as the actor and
`expired: true` in the payload, and resolves the inbox entry in the same
transaction. `hub` is reserved, so no agent can enrol under it. Every reader of
a resolution, the agent's inbox read, its wait, the notification trailer, the
feed and the PWA, sees this one as it sees a human's, marked as expired.

The decision path and the expiry take the same immediate transaction on the
item and check that it is still open, so an item resolves once and whichever
lands first wins. A decision that arrives after the deadline is refused even
before the sweep records the expiry. The serving hub sweeps every few seconds
through an index on the deadline, and the reads that show whether an item is
open settle due items first.

## Consequences

- An agent that cannot wait forever has a way to say so, and the record says
  that the hub, not the human, made the call.
- The human can still act at any time before the deadline. A late decision is
  refused with a conflict rather than overwriting an outcome the agent has
  already acted on.
- This is not retention. Nothing is deleted and no other row expires. A
  deadline is one agent's statement about one item it asked, so ADR 0004
  stands: the human remains the garbage collector.
- The resolution is exempt from the project's event ceiling, so a full feed
  cannot hold an item open past the deadline its agent set.
- Migration 24 adds `expires_at` and `on_expiry` to the inbox, with an index
  for the sweep. A self-enrolment request cannot carry a deadline: it has its
  own window.
