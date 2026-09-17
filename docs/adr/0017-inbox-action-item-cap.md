---
type: Decision Record
title: The inbox caps open action items
description: An agent cannot leave unbounded open items on the human; the writer refuses past a configurable cap.
tags: [adr, inbox, agents, safety]
status: stable
---

# 0017. The inbox caps open action items

## Context

A question and an approval both wait on the human. Each one lands in the inbox
as an open item and raises the waiting badge. Nothing expires, by
[decision](0004-manual-pruning-in-v1.md), and the human is the only garbage
collector, so an agent that appends in a loop can fill the queue with its own
items and push the meaningful ones out of view. The realistic source is not a
hostile agent but a runaway one, and a single operator on one node is exactly
where a loop can run unattended.

A rate limit bounds how fast items arrive, not how many accumulate. Since the
harm is the standing backlog that buries the real items, the guard has to bound
the count, not the rate.

## Decision

The hub caps the open items one actor may hold in one project, and the open
items all actors together may hold in one project. An open item is one whose
inbox status is `action` or `waiting`; resolving an item frees its slot, and an
unread or read item never counts. The defaults are generous, 100 per actor and
1000 per project.

The check runs inside the writer's immediate transaction, before any row is
written, so the count and the insert commit together and two concurrent writers
cannot both slip past. A refused write returns `rate_limited` (HTTP 429) and
changes nothing.

Questions count toward the cap. An agent with a full queue of open questions is
the case the cap exists for.

Two environment variables set the caps, `HUB_INBOX_ACTION_PER_AGENT` and
`HUB_INBOX_ACTION_PER_PROJECT`. A value of zero disables that check, which is
the operator's valve to turn the guard off. A negative or non-numeric value
fails startup, so a typo is loud rather than a silent disable.

## Consequences

- The human's queue stays bounded and the badge keeps meaning, even when an
  agent runs away. The human remains the only thing that drains it. The
  per-project ceiling is per project, so the fleet ceiling scales with the
  number of projects; the per-actor cap is what stops one agent.
- A capped write is refused, not dropped or coalesced, so the agent learns the
  write failed and can stop. The hub does not offer an agent a way to withdraw
  an item; freeing a slot is the human's action today.
- The response carries no `Retry-After` and the tool error marks the write not
  retryable. The wait is state-dependent, on the human resolving or pruning an
  item, not a time the server can name, so neither a delay nor a retry hint
  would be honest.
- The cap is a policy ceiling, not a retention rule. It expires nothing and
  needs no schema change.
- An operator who wants no ceiling sets both to zero. An operator whose
  workflow genuinely needs more raises them.
