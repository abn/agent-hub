---
type: Decision Record
title: One storage engine everywhere
description: The hub store, session brains, and artifacts all run on the Turso Database Rust engine.
tags: [adr, storage, engine]
status: stable
---

# 0002. One storage engine everywhere

## Context

The hub needs a store for feed events, a substrate for per-session brains,
and storage for artifacts. A natural but costly shape uses a different store
for each: a relational database for events, files for brains, and an object
store for artifacts. Each addition multiplies operational surface and splits
the engine story.

Turso Database is a Rust rewrite of SQLite with concurrent write support
under a journal mode. AgentFS itself is built on it, and it offers embedded
full-text search.

## Decision

One engine everywhere. The hub event store, the per-session AgentFS files,
and artifact storage all run on the Turso Database Rust engine. No libSQL, no
SQLite C bindings, and no two-engine split.

## Consequences

- A single dependency and a single consistency model to reason about.
- Concurrent write support is an available property of the chosen engine,
  not a future migration.
- Data definition statements are not allowed inside a concurrent write
  transaction, so schema migrations run in single-writer mode.
- The choice is fixed. A storage facade exists for testing and portability,
  not to enable swapping the engine.
