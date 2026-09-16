---
type: Decision Record
title: Search is engine-native
description: Full-text search over the same engine backs both the MCP tools and the PWA.
tags: [adr, search, fts]
status: stable
---

# 0006. Search is engine-native

## Context

Both agents and the human need to search feed events, artifacts, and session
contents. A dedicated search service is the common answer, and it is the
opposite of a single lean binary: another process, another index to keep
consistent, and another thing to operate.

## Decision

Search is engine-native. Full-text search over the same engine backs both the
MCP search tool and the PWA search box. There is no separate search service.

## Consequences

- One index, updated with the data, and one fewer moving part.
- Results rank on recency and text relevance, grouped by type, with snippets
  and filters for scope.
- Encrypted artifacts are searchable by metadata only, such as title and
  summary, because their plaintext is never on the server.
- Whether the engine's embedded full-text support covers every need is an
  open question to confirm at scaffold time. If it falls short, the fallback
  is a search-text column on the event store, still inside the same engine.
