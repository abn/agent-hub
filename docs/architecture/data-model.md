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
| `projects` | Slug id, display name, an optional owning agent (a personal space is a project an agent owns), creation time, the artifact password policy, reserved retention hints, and a JSON settings column for what comes later. |
| `events` | The feed: time-ordered, append-only, addressable. Kind, actor, a one-line summary, a JSON payload, an action flag, a thread link for question and answer, and the session the write happened during when one was open. |
| `artifacts` | Artifact metadata. Title, description, favicon mark, version label, kind (HTML or markdown), current version, timestamps, the encryption envelope when the artifact is protected, and the blob path. |
| `artifact_versions` | One immutable row per artifact version: the same display metadata plus the per-version envelope, size, blob path, and timestamp, so any version stays addressable. |
| `comments` | Discussion on artifacts: author, body, an optional point or quote anchor with its version, resolution state, and a delete-token hash. |
| `inbox` | The human's global queue, a thin projection over events: status (`unread`, `read`, `action`, `waiting`, `resolved`), assignee, and update time. |
| `project_feed_cursors` | One row per project: the newest feed event the human has seen there, and when it was recorded. There is one human operator, so the project is the key. |
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

An event also names the session it was written during, indexed, so a session
detail screen counts what the session produced without reading a payload. The
column is wider than the lifecycle events: an agent's signals, questions and
approvals carry it too. Pruning is not wider for it. A prune still removes only
the session's own lifecycle events, because storage acts on sessions and never
on feed events or artifacts, and the work a session left in the feed outlives
the session.

An inbox status carries two independent things, and the human's read verb
touches only one of them. `unread` and `read` are the read axis: the human
marks an entry read, unmarks it, or marks every unread entry read. `action`
and `waiting` mean the item waits on a decision, and `resolved` means that
decision was made; none of the three has a read state, and marking one read is
answered as unchanged rather than converting it. Read state is never inferred
from a scroll position.

It does not reach agents at all. The routes that set it are part of the
admin-only human surface, and the agents' own inbox read collapses the axis:
an entry the human has read is reported as `unread`, carrying the timestamp it
had before, and `read` is not a status an agent can filter on. An agent
therefore cannot poll the inbox to learn which of its reports the human has
opened, or when. The listing is ordered by event id, which is minted in commit
order, so reading an item never moves it or shifts the page a limit cuts.

The display name is the only name a project keeps in one place: every other
table, every MCP call and every blob path names the slug, so renaming a project
changes one column and nothing goes stale behind it. The slug itself is
read-only after creation for the same reason. The artifact password policy is
its own column rather than a key in the reserved settings JSON, because every
publish reads it.

A feed is not read the same way, and the two must not be confused. Nothing in
a feed is marked by hand: a project carries one cursor, and every event above
it is unseen. The cursor moves only forward, only when the human opens that
feed, and only to an event of that project, so it cannot be dragged backwards
or pointed at another project's event. Counting what lies above it seeks into
the feed index on both the project and the cursor, so it touches only the
events above the cursor and never reads the feed as a whole; across projects
that is one small read of the cursors and one such seek each. Deleting a
project takes its cursor with everything else scoped to that project. The hub's
own audit events are not counted, since the human feed does not show them.

Upgrading seeds each existing project's cursor at its newest event. The dot
means "new since you last looked", and a hub that has been running was being
looked at, so starting at nothing seen would light up every project with a
backlog the human cannot clear in one gesture. A project created afterwards has
no cursor, and its first events are new, which is the same rule read forward.

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

A session's last activity means activity. Every tool call that resolves the
caller's active session touches it, whether it writes a brain, posts a signal
or asks the human, coalesced so a busy agent writes the timestamp at most once
a minute. An agent counts as active while it owns a live session touched inside
the active window. The touch is bookkeeping: a store too busy to take it never
fails the call that triggered it. An agent with two sessions
counts once, an agent with no session never counts, and a token last used a
moment ago is not the same thing: `agents.last_seen_at` answers that question
and is not this one.

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

Every write to a page is one row in the file's own append-only tool call log:
the operation, the canonical path, the actor the hub authenticated, the time
and the version stored. The row is written under the same hold of the writer
lock as the page, so the log is in the order the writes landed. It is what a
page's history, its last writer and its trust tier are read from. It holds no
content, it is never trimmed, and it is bounded only by the file's own size
limit, so reading it is a scan. The routes over it are in
[project knowledge base](../usage/knowledge-base.md).

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
