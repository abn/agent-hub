---
type: Explanation
title: Architecture overview
description: The process shape and the two structural choices behind the hub
tags: [contributor, overview]
status: stable
---

# Architecture overview

Agent Hub is one process on one node. It serves two client surfaces, an MCP
server for agents and an HTTP API with the installable PWA for the human, over
one core that owns the event store, identity, and search. The pages in this
section describe that single process; the interface lookups both surfaces offer
live under [Reference](/reference/index.md).

Two structural choices shape everything else.

- **One storage engine.** The Turso Database Rust engine backs the hub store,
  the per-session AgentFS files, the project knowledge bases, the search
  index, and artifact metadata, so there is no two-engine split. Artifact
  blobs are the one thing outside it, on the data volume. The reasoning is
  [decision 0002](/adr/0002-single-turso-engine.md).
- **Real AgentFS, wrapped per session.** The hub wraps AgentFS rather than
  reimplementing it. One file per session holds that session's key-value
  state, append-only audit log, and POSIX-like filesystem, and one file per
  project holds the shared knowledge base. The wrapper is the single writer
  per file, which is what makes cross-session conflicts structurally
  impossible ([decision 0003](/adr/0003-wrap-agentfs-per-session.md)).

Those two choices are why the result is lean: one binary, one engine, no
microservices, no CRDTs, and no custom distributed consensus.

## Where to go

- [Operating model](/architecture/operating-model.md) - the trust posture and
  who may reach what.
- [Components](/architecture/components.md) - the process model, the
  subsystem boundaries, and the deployment shape.
- [Data model](/architecture/data-model.md) - the hub store tables, the
  session brain file, and the project knowledge base.
- [MCP tools](/reference/mcp-tools.md) and [REST API](/reference/rest-api.md) -
  what each surface exposes.
