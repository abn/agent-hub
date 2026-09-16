---
type: Reference
title: Data model
description: The hub store tables and the per-session brain file.
tags: [architecture, data-model, schema]
status: draft
---

# Data model

The hub keeps two kinds of durable state: the hub store, one database for
projects, events, artifact metadata, inbox status, and session metadata; and
the per-session brain files. Both run on the same engine.

## Hub store

Projects are the top-level grouping. Everything is per project except the
inbox, which is global.

| Table | Holds |
|---|---|
| `projects` | Slug id, display name, creation time, reserved retention hints, and a JSON settings column such as artifact password policy. |
| `events` | The feed: time-ordered, append-only, addressable. Kind, actor, a one-line summary, a JSON payload, an action flag, and a link to a related event for question and answer threads. |
| `artifacts` | Artifact metadata. Title, kind (HTML or markdown), current version, timestamps, an optional password salt, and the path to the blob. |
| `inbox` | The human's global queue, a thin projection over events: status (`unread`, `read`, `action`, `waiting`, `resolved`), assignee, and update time. |
| `sessions` | Session metadata only, because state lives in the brain file: project, agent, status, the brain file path, and creation and last-activity times. |

The event id is a time-ordered ULID, which makes feed paging and addressable
lookups cheap. Indexes support paging a project feed newest first and
ordering the inbox by action and time. The `events` table carries a JSON
payload, so structured detail rides along without a second schema.

Retention is deliberately a per-layer concept. The schema carries
`created_at`, `last_activity`, a `retention` column, and room for an
`archived_at` column so the later layers slot in without a painful migration.
The [decision record](../adr/0004-manual-pruning-in-v1.md) explains why no
layer ships in v1.

Artifact blobs live in a filesystem store under a per-project directory. That
store is the blob layer; no external object store is needed.

## Session brain file

There is one AgentFS file per session. It hosts three things in one file:

- a **key-value store** for brain state, read and written as paths;
- an **append-only audit log**, the session's signal log of what happened;
- a **POSIX-like filesystem** (inode and dentry tables) for scratch files,
  plans, temporary outputs, and handoff documents.

A brain is arbitrary session-scoped state, not a curated subset. Its life is
session-bound: it survives same-session compaction and resume of the same
named session, and it is removed only when the human prunes the session.
Durable knowledge leaves it only by explicit promotion.

A recovery handoff document is a convention on a well-known path inside the
brain, not a separate subsystem.

## Multi-writer sessions

When several agents share one named session, they still write through the
wrapper. The wrapper uses versioned upserts with optimistic locking, so a
write lands only against the version it read. Last-writer-wins is acceptable
because the wake cycle reads before it writes.

## See also

- [Components](components.md) - which component owns which state
- [Agent surface](agent-surface.md) - the tools that read and write it
