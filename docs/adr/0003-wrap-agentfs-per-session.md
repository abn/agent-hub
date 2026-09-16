---
type: Decision Record
title: Wrap AgentFS per session
description: The hub wraps the real AgentFS per session and is the single writer for each file.
tags: [adr, agentfs, sessions, storage]
status: stable
---

# 0003. Wrap AgentFS per session

## Context

A session brain needs key-value state, an append-only audit log, and
somewhere to keep files. Reimplementing that shape invites drift from a
storage layer that already exists and is maintained. AgentFS already provides
all three in one file, on the same engine the hub uses.

## Decision

Use the real AgentFS, server-side, one file per session, and wrap it. The hub
never reimplements AgentFS. Agents never touch a brain file directly; they
speak MCP to the wrapper, which is the single writer for each session file.

## Consequences

- One file per session holds key-value state, the append-only audit log, and
  a POSIX-like filesystem.
- Serialising writes per file makes cross-session conflicts structurally
  impossible.
- When several agents share one named session, the wrapper uses versioned
  upserts with optimistic locking, and last-writer-wins is acceptable because
  the wake cycle reads before it writes.
- An open question at scaffold time is whether AgentFS is embedded as a Rust
  crate or driven as a subprocess. Both keep a single binary; the choice is
  settled empirically before code depends on it.
