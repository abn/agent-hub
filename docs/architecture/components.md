---
type: Reference
title: Components
description: The process model and the boundaries between subsystems.
tags: [architecture, components]
status: draft
---

# Components

The hub is a single process with a small number of internal boundaries. This
page describes those boundaries and the constraints that hold across them.

## Process model

One Rust process serves three client-facing endpoints over a shared core:

- an **MCP server** for agents, over stdio for local agents and streamable
  HTTP for remote agents on a LAN or tailnet;
- an **HTTP API** for the human surface, with the PWA served as static assets
  from the same binary;
- the **core**, which owns the event store, authentication and authorization,
  and search.

The AgentFS wrapper sits between the core and the session files. It holds one
write handle per active session, so writes to a given session file are
serialized by construction and cross-session conflicts are structurally
impossible. The hub event store uses the engine's concurrent journal mode
where available.

## Boundaries

- **Agents never touch files.** An agent speaks MCP to the wrapper, which is
  the single writer for a session file. There is no path by which an agent
  opens a brain file directly.
- **The wrapper is the only AgentFS caller.** No other component embeds or
  reimplements AgentFS; the hub wraps it.
- **Schema migrations run in single-writer mode.** Data definition statements
  are not allowed inside a concurrent write transaction, so migrations take
  the single-writer path.
- **The storage facade is hygiene, not a swap seam.** It exists to keep the
  storage layer testable and portable. It is not engine-swap machinery; the
  engine is fixed.

## Deployment shape

The binary is the unit of deployment, whether run directly on a node or in a
scratch or distroless container as a non-root user with a read-only root
filesystem. Persistence is a single mounted data volume; backups are node or
NAS snapshots.

A tailnet-SDK build flag lets the same binary join a tailnet itself, so no
inbound ports need opening and no reverse proxy is required. The plain
container behind a reverse proxy remains the fallback.

## See also

- [Data model](data-model.md) - what the components read and write
- [Decision 0002](../adr/0002-single-turso-engine.md) - why one engine
- [Decision 0003](../adr/0003-wrap-agentfs-per-session.md) - why the wrapper
  is the single writer
