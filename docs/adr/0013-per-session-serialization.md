---
type: Decision Record
title: Per-session serialization
description: The hub is the single writer per session file and serialises writers in process.
tags: [adr, concurrency, sessions, agentfs]
status: stable
---

# 0013. Per-session serialization

## Context

Several agents can share one named session and write to the same brain.
Optimistic locking with a version check in SQL would need a version column the
AgentFS store does not have, and the hub is a single process, so a
cross-process lock is not needed either.

## Decision

The hub process is the single writer for each session file, so it serialises
writers with a per-session in-process lock. Multiple agents in one session
write through the hub and are ordered by that lock. No optimistic locking in
v1.

## Consequences

- Writers to distinct sessions never block each other; writers to one session
  serialise.
- A version column is reserved in the schema for a future multi-process model,
  but nothing depends on it.
- Cross-session write conflicts remain structurally impossible.
