---
type: Decision
title: A knowledge base page keeps every version, in its own file
description: Why the project knowledge base keeps the bytes of every page version beside the pages, how a revert is an ordinary write, and why only the operator's purge takes them away.
tags: [adr, knowledge-base, agentfs, history, retention]
status: stable
---

# 0029. A knowledge base page keeps every version, in its own file

## Context

Many agents write one knowledge base, and a write can be wrong: a bad merge, a
page cut short, a delete of the wrong file. The write log already says who
wrote which page and when ([ADR 0018](0018-project-knowledge-base.md)), but it
answers who and when, never what. AgentFS's append-only audit log is that
write log: one row per write naming the operation, the path, the actor, the
byte count and the version token. It records operations and keeps no content,
so nothing the hub held could show an earlier page or put it back.

The place to keep earlier bytes has three constraints. One engine and one file
per project, so a second database or a side directory of copies is out. The
hub is the single writer per file, so the copies have to be written under the
same lock as the page. And the knowledge base is outside session life: prune
never touches it, and the human is the garbage collector
([ADR 0004](0004-manual-pruning-in-v1.md)).

## Decision

The knowledge base file holds one more table beside the AgentFS tables,
`hub_page_versions`, keyed by version token, holding the bytes and when they
were first kept. The wrapper writes it, under the writer lock, on every page
write and delete: the bytes being replaced and the bytes being stored on a
write, the bytes going on a delete. They are written once the page write has
landed and before the log row, so a write the store refuses keeps nothing and a
log row never names a version the file has no bytes for. Because the key is the content hash, a page that goes back to earlier
bytes stores nothing new. A session brain keeps no versions.

AgentFS writes the page in a transaction of its own on the one connection its
pool holds, so the version, the page and the log row are three writes under one
lock rather than one transaction. A crash between keeping the bytes and
logging the write leaves a kept version that no row names: no history can read
it, but it holds space. A purge sweeps every such version, whatever page it was
for.

A page's history is the write log for its path, each row joined to whether its
bytes are kept. Reading a version, or reverting to one, requires that the
page's own history names it, so a page is never set to bytes from another
path. A revert is an ordinary write of the earlier bytes by the reverting
actor, logged as `kb.revert`, guarded by `if_version` like any other write and
authorized as one: whoever may write the project's knowledge base may revert
it. It never removes a row or a kept version, so a revert is undone by another
revert. A deleted page keeps its history and is restored the same way, with
`absent` as the guard. A revert to the version the page already holds writes
nothing and signals nothing.

Nothing expires. The human collects the history, as they collect everything
else: forgetting a page's history is an admin operation beside prune, over
REST, the CLI and the PWA and never over MCP. It removes the kept bytes of
every version the page's log names except the one the page holds now, and all
of them for a deleted page. The log rows stay, so the history still says who
wrote what and when, and its rows read as not kept. The page is marked
forgotten through its newest row in a second table, `hub_page_forgotten`, so
no row up to there reads back through that page even where the same bytes stay
because another page's history still names them. A purge is recorded as an
audit event by the human.

The size limit is held against the pages the file uses, its free pages left
out, not against the file on disk. A delete hands pages back to the file rather
than to the disk and later writes reuse them, so measured on disk a purge would
never let a full knowledge base write again. A write is projected at its own
size plus a copy of each side it keeps that the file does not hold yet. A delete
is never refused for the copy it keeps, which is at most one page, so a full
knowledge base can still shed a page.

The purge's audit is appended with what it will remove before anything is, so a
purge that cannot be recorded removes nothing. Its removals and its mark are
one transaction; if that rolls back, a second audit event says so.

## Consequences

- Every version written from now on can be read and put back, from either
  surface, and a deleted page can be restored.
- A page written by a hub from before this keeps its newest bytes, which are
  the page, and the next write or delete keeps them. Versions it replaced
  before that are listed as not kept.
- A deleted page's bytes stay readable to every agent with read access until
  the operator forgets its history. That is the point of keeping them, and the
  purge is the answer for a page that should not have been written.
- A purge frees space inside the file, not on the disk: the file keeps its
  size and the next writes reuse it. The engine does not zero freed pages, so
  forgotten bytes can stay on disk until they are overwritten, and a backup
  taken before the purge still holds them.
- Project deletion removes the file and with it every kept version; backup and
  restore carry them, since they are rows in the same file.
