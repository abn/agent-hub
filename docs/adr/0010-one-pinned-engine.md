---
type: Decision Record
title: One pinned engine version
description: AgentFS is vendored and its engine patched so the binary links exactly one engine.
tags: [adr, engine, agentfs, storage]
status: stable
---

# 0010. One pinned engine version

## Context

[Decision 0002](0002-single-turso-engine.md) fixes one engine everywhere. In
practice the embeddable AgentFS SDK pins its own engine version, and the hub
store needs a newer one for native full-text search. Depending on both links
two copies of the engine into the binary, which breaks the one-engine and lean
invariants and roughly doubles the storage surface.

## Decision

Vendor the AgentFS SDK and patch its engine dependency to the single engine
version the hub pins. The dependency tree must contain exactly one engine
version. The upstream API drift from the pinned version to the current one is
sized before any product code is wired.

## Consequences

- `hub.db` keeps native full-text search, and session files run the same
  engine.
- A dependency that reintroduces a second engine version fails the build.
- The engine is a pre-release, so the version is pinned exactly, never
  auto-upgraded, and any bump is tested for concurrent writes before it lands.
- Vendoring carries a maintenance duty: when the upstream SDK moves, the patch
  is reapplied and the drift reassessed.
