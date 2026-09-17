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
| `projects` | Slug id, display name, an optional owning agent (a personal space is a project an agent owns), creation time, reserved retention hints, and a JSON settings column such as the artifact password policy. |
| `events` | The feed: time-ordered, append-only, addressable. Kind, actor, a one-line summary, a JSON payload, an action flag, and a thread link for question and answer. |
| `artifacts` | Artifact metadata. Title, kind (HTML or markdown), current version, timestamps, the encryption envelope when the artifact is protected, and the blob path. |
| `inbox` | The human's global queue, a thin projection over events: status (`unread`, `read`, `action`, `waiting`, `resolved`), assignee, and update time. |
| `sessions` | Session metadata: project, the agent-supplied session name, agent, status, the brain file path, timestamps, and a soft-delete marker. State itself lives in the brain file. |
| `agents` | Agent identity, display name, trust level (`trusted` or `untrusted`), and the id of the agent's personal space. |
| `agent_tokens` | Token hashes bound to an agent, with last use and revocation. An agent has one live token at a time; issuing a new one revokes the previous token in the same transaction. |
| `grants` | An agent, a project, and read or write access, for opening a project to an untrusted agent. |
| `search_docs` | The search corpus: one row per indexed document (feed, artifact, or brain path) with a full-text index over title and body. |

The event id is a time-ordered ULID, which makes feed paging and addressable
lookups cheap. Indexes support paging a project feed newest first and
ordering the inbox by action and time. The `events` table carries a JSON
payload, so structured detail rides along without a second schema. The event
kind is a closed set of the six design families (`signal`, `finished`,
`question`, `answer`, `approval`, `artifact`, `session`) plus `system`;
sub-actions such as an artifact publish or update ride in the payload.

Identity is a first-class table rather than a field on a token, so the server
sets the `actor` on every event and a request cannot forge another agent. The
trust model and grants are described in [agent identity and
trust](../adr/0012-agent-identity-and-trust.md).

Identity changes are audited. Creating an agent, changing its trust, issuing or
revoking its token, and adding or removing a grant each append a `system` event
to the affected project's feed, in the same transaction as the change, so a
change and its record cannot diverge. Agent-scoped changes land in the agent's
personal space; a grant lands in the project it opens.

Retention is deliberately a per-layer concept. The schema carries
`created_at`, `last_activity`, a `retention` column, and room for an
`archived_at` column so the later layers slot in without a painful migration.
Pruning is reversible for a short window: a session carries `deleted_at` while
its removal can still be undone, and the file is removed only when the window
closes. The [decision record](../adr/0004-manual-pruning-in-v1.md) explains why
no automatic layer ships in v1.

Artifact blobs live in a filesystem store under a per-project directory. That
store is the blob layer; no external object store is needed. A protected
artifact stores an envelope (algorithm, key derivation, iterations, salt, and
initialisation vector) beside the ciphertext, and the browser holds the only
key.

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
wrapper. The hub process is the single writer per session file, so it
serialises writers with a per-session lock; writers to distinct sessions never
block each other. Optimistic locking is not used in v1 because it needs a
version column the store does not carry and a multi-process model the hub does
not have; a version column is reserved for later.

## See also

- [Components](components.md) - which component owns which state
- [Agent surface](agent-surface.md) - the tools that read and write it
