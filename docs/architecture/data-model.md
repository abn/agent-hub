---
type: Reference
title: Data model
description: The hub store tables, the session brain files, and the project knowledge bases.
tags: [architecture, data-model, schema]
status: draft
---

# Data model

The hub keeps two kinds of durable state: the hub store, one database for
projects, events, artifact metadata, inbox status, and session metadata; and
the AgentFS files, one per session and one per project. All of them run on the
same engine.

## Hub store

Projects are the top-level grouping. Everything is per project except the
inbox, which is global.

| Table | Holds |
|---|---|
| `projects` | Slug id, display name, an optional owning agent (a personal space is a project an agent owns), creation time, reserved retention hints, and a JSON settings column such as the artifact password policy. |
| `events` | The feed: time-ordered, append-only, addressable. Kind, actor, a one-line summary, a JSON payload, an action flag, and a thread link for question and answer. |
| `artifacts` | Artifact metadata. Title, description, favicon mark, version label, kind (HTML or markdown), current version, timestamps, the encryption envelope when the artifact is protected, and the blob path. |
| `artifact_versions` | One immutable row per artifact version: the same display metadata plus the per-version envelope, size, blob path, and timestamp, so any version stays addressable. |
| `comments` | Discussion on artifacts: author, body, an optional point or quote anchor with its version, resolution state, and a delete-token hash. |
| `inbox` | The human's global queue, a thin projection over events: status (`unread`, `read`, `action`, `waiting`, `resolved`), assignee, and update time. |
| `sessions` | Session metadata: project, the agent-supplied session name, the agent that owns it, status, the brain file path, timestamps, a soft-delete marker, the handoff note its owner left, and the session it was adopted or forked from. A live name is unique per owner inside a project. State itself lives in the brain file. |
| `agents` | Agent identity, display name, trust level (`trusted` or `untrusted`), and the id of the agent's personal space. |
| `agent_tokens` | Token hashes bound to an agent, with last use and revocation. An agent has one live token at a time; issuing a new one revokes the previous token in the same transaction. |
| `grants` | An agent, a project, and read or write access, for opening a project to an untrusted agent. |
| `search_docs` | The search corpus: one row per indexed document (feed, artifact, session brain path, or knowledge base page) with a full-text index over title and body. |

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
personal space; a grant lands in the project it opens. The trail is the human's
to read: a `system` event is left out of the search corpus, and an agent's feed
read never returns one, whatever kinds it asks for. The admin reads it through
the project feed route, by kind.

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
session-bound: it belongs to the agent that started the session, survives
same-session compaction and resume of that name by its owner, can be adopted or
forked by another agent, and is removed only when the human prunes the session.
Durable knowledge leaves it only by explicit promotion.

A brain is written only through its owner's active session, and read by anyone
who may read the session's project. The single writer is what keeps one working
state coherent; the open read is what lets a fleet of agents see what a sibling
is working from. Reads never create a brain file, so a session that wrote
nothing leaves nothing on disk, and a pruned session is unreadable from the
moment it is marked.

Ownership moves two ways, and the hub picks which from the source's state.
Adopting an ended session moves the owner and nothing else: the id, the file
and every search row stay as they are. Forking a running one copies the file
through the engine, which takes the audit log with it, and writes a second set
of search rows under the new session id; the source is not told and not
touched. Either way the lineage is recorded on the row, and it stays recorded
after the source is pruned, where the surfaces render it as a session that is
gone rather than repairing it.

The handoff note a session leaves when it ends lives on the session row and in
the feed event, never in the brain: a session that never wrote must not get a
brain file merely because it ended. A recovery handoff document is a separate
convention, on a well-known path inside the brain, not a separate subsystem.

## Project knowledge base

There is one AgentFS file per project, beside the session files rather than
under them, holding that project's knowledge base. It is the same file format
behind the same wrapper and the same single-writer lock, so it needs no store,
no table and no locking of its own. It holds pages only, under `/fs/`, and
carries no key-value namespace: a knowledge base is content with a shape, not a
second scratch store.

Its life is the project's, not a session's. No prune reaches it, and it is
removed only when the project is deleted, together with its pages' search rows.
Pages are indexed as their own corpus family, so a search can ask the shared
knowledge base a question without every session brain answering first.

Every agent with write access to the project may write every page, which is
what a shared store means. A write can carry the version it is based on, a
content hash of the bytes the writer read, and the hub applies it only while
the stored content still hashes to that value; the comparison and the write
happen under the same writer lock, so two writers holding one version cannot
both win. An agent's personal space is a project, so it gets a knowledge base
like any other: a durable store the agent alone writes, and every trusted agent
and the human can read.

## Multi-writer stores

Agents write through the wrapper, never the file. The hub process is the single
writer per file, so it serialises writers with a per-file lock; writers to
distinct files never block each other.
The same lock orders the many writers a project knowledge base has by design,
and carries its compare-and-set. No version column is used for this: the token
is a hash of the content itself, so it cannot drift from what it describes. A
reserved version column in the session schema is left for a future
multi-process model and nothing depends on it.

## See also

- [Components](components.md) - which component owns which state
- [Agent surface](agent-surface.md) - the tools that read and write it
