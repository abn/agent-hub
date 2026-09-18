---
type: Decision Record
title: The project knowledge base
description: One AgentFS file per project behind the same wrapper, reached by the brain tools with a store argument and written with a content-hash compare-and-set.
tags: [adr, knowledge-base, agentfs, projects, search]
status: stable
---

# 0018. The project knowledge base

## Context

A session brain is working state and is garbage-collected when the human
prunes the session, so nothing an agent learns outlives the session that
learned it. The targets for durable knowledge were a feed event, which is a
one-line record on a timeline, and an artifact, which is a versioned snapshot
for the human to read. Neither is a place a fleet keeps what it knows about a
project and finds it again later.

The shape of that place has to answer three things: where the bytes live, how
agents reach them, and what happens when several agents write one page, which
is the normal case rather than an accident.

## Decision

One AgentFS file per project holds the project knowledge base, in `kb/` beside
the session files rather than under them. It is opened through the same
wrapper and the same per-file writer lock as a session brain, so there is no
second store type, no second locking scheme and no new removal path: a session
prune cannot reach it by construction, and project deletion removes it the way
it removes a session brain.

The existing brain tools reach it through a `store` argument rather than a
second family of tools. The operations are identical, and a second family
would be four more descriptions for a caller to disambiguate, differing by one
word. `store` is required on `brain_put` and `brain_delete` and defaults to
`"session"` on the reads: a read that guesses wrong gets `not_found` and
recovers, while a write that guesses wrong is silent and costs either durable
knowledge at the next prune or scratch state in the store the project shares.

A knowledge base holds pages only, under `/fs/`. A key-value namespace in the
same file would be a second store with no rendering surface and no listing.

Concurrent writes are resolved by a content-hash compare-and-set. The version
token is `sha256:` over the bytes a read returns, so a caller can reproduce it
from what it read, and a write carrying `if_version` applies only while the
stored content still hashes to that value. The read, the comparison and the
write happen under the writer lock. The literal `absent` creates a page only
when nothing is there. Without `if_version` the last writer wins, so the
common append-shaped write stays one call.

Authorization is the project's own: read for a read, write for a write, on the
target project, with no ownership rule of its own. An ordinary write emits no
feed event: a shared store with many writers would turn the feed into a change
log, which is not what the feed is for.

## Consequences

- Knowledge survives every prune. The knowledge base is removed only when its
  project is, and it is reported in storage usage and never as prunable.
- An agent's personal space is a project, so it gets a durable store with no
  new mechanism. It is the agent's own to write and every trusted agent and
  the human can read it, so it is not private and is not described as such.
- Pages are a corpus family of their own in the one search index, scoped to
  their project like every other document, so the confinement rules need no
  special case.
- Identical content carries an identical version, so a write of B followed by
  a write back to A is invisible to a compare-and-set holding A's token. That
  is what a content-addressed token means; the alternative is a counter that
  can drift from the content it describes.
- History, review, promotion and a human surface are additive on top of this
  and are not part of it.
