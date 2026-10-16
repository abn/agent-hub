---
type: Decision Record
title: Semantic recall across projects
description: Add on-node semantic search and relation panels over knowledge base pages, artifacts, and promoted feed events, respecting existing project visibility.
tags: [adr, search, knowledge-base, embeddings, cross-project]
status: stable
---

# 0033. Semantic recall across projects

## Status
Proposed (companion to ADR 0006, ADR 0018, and ADR 0022).

## Context
Search is engine-native (ADR 0006) and already ranks in the hub (ADR 0022), but
it is lexical. A reader looking for "the page about release handling" will not
find "the page about the gate" unless the words match. Backlinks (ADR 0018)
help within a knowledge base but discovery across projects is manual, so the
knowledge accumulated in one project is effectively siloed.

## Decision
Add a second, optional recall layer that lives in the same engine:

- Store a small embedding vector per knowledge base page, per artifact version,
  and per promoted feed event, written at write time and rebuilt by a reindex
  command.
- Expose a "related to this" panel on a page, artifact, and feed row, and an
  optional "semantic" mode of the existing search box that blends lexical and
  vector results with a stated score.
- Cross-project results honour `policy::visibility` exactly: an agent or human
  sees only projects they may read, and confidential content never appears for
  an ungranted caller.
- Keep FTS as the default and the authoritative source for counts; the vector
  layer is additive, degrades to lexical search when embeddings are absent, and
  never blocks a write.
- The embedding model is operator-chosen and runs on the node (a small local
  model or an OpenAI-compatible endpoint on the LAN). No content leaves the
  machine.

## Consequences
- Four large bodies (KB pages, artifacts, feed events, comments) become
  cross-linked without a rewrite of the search service.
- Adds a migration, an index, and a reindex path; the schema version guard
  (ADR for schema management) covers upgrades.
- Search stays lean: a missing model leaves the lexical index untouched, so
  the invariant "search works on a fresh database" holds.
- Pushes storage growth; per-page vectors dominate. Per-project quotas and
  prune (ADR 0004) apply equally to the vector index.
- Operators gain one more knob (model endpoint, dimension). Defaults stay
  local and empty.
