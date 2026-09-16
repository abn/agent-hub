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

## Amendment (2026-09-16)

The embedding question is settled: AgentFS ships an embeddable Rust crate,
`agentfs-sdk`, so the hub embeds it rather than shelling out. The crate pins
its own engine version, which conflicts with the hub store, so it is vendored
and its engine patched to the single pinned version (see
[decision 0010](0010-one-pinned-engine.md)). The SDK exposes the KV store, the
filesystem, the audit log, and the raw connection the wrapper needs.
