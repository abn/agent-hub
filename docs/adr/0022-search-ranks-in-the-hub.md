---
type: Decision
title: Search ranks in the hub, bounded by a fetch cap
description: Why the engine returns search rows unordered and the hub scores them, and what would change it.
tags: [adr, search, engine]
status: stable
---

# 0022. Search ranks in the hub, bounded by a fetch cap

## Context

[ADR 0006](0006-engine-native-search.md) made search engine-native: one
full-text index over `search_docs`, no separate search service. The intent was
that the engine would also order and page a query, so a reader got the most
relevant hits without the hub reading the whole match set.

The engine's index method does not order every query. Its optimizer only keeps
an `ORDER BY fts_score`/`LIMIT` pattern when the statement's `WHERE` is
covered completely by the index, which in practice means a single `fts_match`
term. A query with any second predicate has its ordering and limit dropped, so
the engine returns rows unordered and `fts_score` readable per row.

Every query this hub builds has a second predicate. A soft-pruned session's
brain rows must be excluded immediately and restored on undo, so the query
always carries that exclusion, and callers add project, kind and session
scopes on top. The hub therefore reads the matches, scores them, sorts them,
and cuts the page, bounded by `SEARCH_FETCH_MAX`.

## Decision

Search ranks in the hub over a bounded fetch. The engine is the index, not the
ranker, for any query this hub issues.

A schema column cannot change this. The exclusion cannot be folded into the
single term the engine accepts: it is a second predicate by construction, and
a partial or covering FTS index guarded by a predicate is not something the
engine's index method supports. A no-predicate query (`WHERE fts_match(...)`)
does take the ranked path, but it cannot exclude a soft-pruned session without
post-filtering in the hub, which returns the same read-the-matches shape.

## Consequences

- A corpus with more matches than `SEARCH_FETCH_MAX` can drop a relevant hit
  that sits beyond the arbitrarily ordered fetch. Under the cap the page is
  correct; over it, relevance is approximate. The cap is the honest ceiling,
  not the engine's ordering.
- Raising or removing the cap is the lever for relevance. It costs a larger
  read, not a schema change.
- The soft-prune exclusion stays in the query, so immediate exclusion and undo
  visibility hold, which the escape hatch of a post-filter would put at risk.
- If the engine gains a covered predicate for the exclusion, or a partial FTS
  index, the fast path can come back. Until then the fallback is the design.

This record supersedes the earlier internal note that the dead ranked branch
was a regression to be repaired by a `pruned` column on `search_docs`; that
shape does not work, for the reason above.
