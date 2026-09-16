---
type: Decision Record
title: Lean by construction
description: A single binary and a single engine, with no microservices, CRDTs, or consensus.
tags: [adr, architecture, lean]
status: stable
---

# 0008. Lean by construction

## Context

Agent infrastructure tends to accumulate processes: a queue, a search
service, a worker fleet, a coordination layer. Each addition is individually
reasonable and collectively unshippable on a NAS by one person.

## Decision

Stay lean by construction. One binary, one engine, no microservices, no
CRDTs, and no custom distributed consensus. Where a feature would require a
new process, prefer a smaller design inside the binary.

## Consequences

- The storage facade and the single engine keep the data story simple.
- Per-session single-writer semantics remove the need for a coordination
  layer.
- Search, artifacts, and the feed all live in the same process and engine.
- A future feature that genuinely needs a distributed design has to justify
  itself against this decision rather than being added by default.
