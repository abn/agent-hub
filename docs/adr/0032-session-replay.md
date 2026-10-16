---
type: Decision Record
title: Session replay and time travel
description: Expose a session brain's audit log and versioned KV state as a replay timeline, and let a reader fork a new session from any captured point.
tags: [adr, session, brain, audit, replay, time-travel]
status: stable
---

# 0032. Session replay and time travel

## Status
Proposed (companion to ADR 0003 and ADR 0020).

## Context
A session brain holds a versioned KV store and an append-only audit log
(ADR 0003). Today the human reads the final state and the feed events a
session chose to promote, but cannot see how a brain got there, or rewind to
inspect an earlier state. Agent adoption and fork (ADR 0020) already create a
lineage, yet there is no surface that walks that lineage.

## Decision
Add a read-only replay surface over a single session brain:

- The session view gains a "Replay" mode that walks the audit log as a
  timeline, showing each mutation with its actor, key, and value diff against
  the previous entry.
- A scrubber (keyboard and pointer) sets the as-of time; the KV panel renders
  the state at that point, and reading a value opens its version history.
- A "Fork from here" action starts a new named session whose brain is seeded
  from the captured KV snapshot and recorded as a fork of the original.
- Replays are scoped to readers who may read the session's project, exactly as
  `brain_get` and `brain_list` already are.
- No mutation or rewind of the original brain: replay never writes to it.

## Consequences
- Agent mistakes become forensic and reversible; the audit log becomes a
  first-class UI instead of a debug artifact.
- Adds a per-session snapshot read path. The versioned KV already stores
  enough to reconstruct state; the scrubber indexes rather than exports.
- Forking produces a new brain with a distinct session id and its own lifecycle
  (ADR 0020), so GC on prune still works unchanged.
- Keeps the "human is the garbage collector" rule: snapshots remain in the
  session's brain and are pruned with it.
