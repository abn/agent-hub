---
type: Reference
title: Agent surface
description: The MCP tools agents call, and the session bootstrap convention.
tags: [architecture, mcp, agents, tools]
status: draft
---

# Agent surface

Agents reach the hub over the Model Context Protocol: streamable HTTP on the
hub's own listener at `/mcp`, with a bearer token that resolves to one agent
identity. One MCP server exposes the tools below, and every tool ships.

A harness that speaks only stdio runs `agent-hub mcp` as a proxy to that
endpoint: one connection for the life of the process, every request forwarded,
so the tools and the errors are the hub's and a tool the hub gains needs no
new client. Because the hub tracks the active session per connection, a
session started through a proxy stays active for the rest of that process.
With no hub configured the same command still serves the local data directory
standalone, as the human admin, and says so on startup; that mode opens the
data directory itself and so cannot run beside a hub on it. See
[the hub client](../adr/0019-hub-client-proxy-and-cli.md).

Hooks are shell commands with no MCP client, so the binary also makes one-shot
calls: `agent-hub call <tool> [json]` prints the tool's JSON result on stdout
and exits 0, or prints the hub's error object on stderr and exits non-zero
(1 a tool error, 2 usage, 69 unreachable, 77 refused, 78 unconfigured).
`agent-hub tools` lists the hub's tools. A call is its own connection, so it
holds no session: it is for stateless reads and writes that name their target,
and session-bound work goes through the proxy.

## Tools

| Tool | Purpose |
|---|---|
| `session_start` | Register or resume the caller's own session by project and session name; the agent is the authenticated identity. Idempotent on the name, so a resume reuses the same brain. With `from`, it picks up another agent's session. |
| `session_end` | Mark a session ended, with an optional handoff note. Only its owner, or the human admin, may end it. The brain is retained until the human prunes it. |
| `session_list` | List sessions with their owner, status, handoff note and lineage, confined to the projects the caller may read. |
| `feed_read` | Read a project feed, optionally filtered by kind. With `since` and no `before`, the page is oldest first, continuing forward from the cursor; otherwise it is newest first. |
| `signal_append` | Append an event to a project feed. |
| `question_post` | Ask the human or another agent a question. It lands in the inbox and the feed, and returns the question id. |
| `answer_post` | Reply to a question by its question id. The answer lands in the feed and closes the thread. |
| `inbox_read` | Read the human's global inbox, optionally by status or project. Each item carries its `project_display_name` beside `project_id`. A decided approval carries its `decision`: approved or declined, the note the human left, who decided and when. A resolved question carries its `answer`: the reply body, who answered and when. |
| `artifact_publish` | Publish an HTML or markdown artifact, public or password protected. |
| `artifact_update` | Publish a new version of an existing artifact. |
| `artifact_get` | Read an artifact's content and metadata, optionally one version. |
| `artifact_versions` | List an artifact's immutable version history. |
| `artifact_list` | List a project's artifacts. |
| `artifact_delete` | Delete an artifact, its history, and its index row. |
| `comment_post` | Comment on an artifact, optionally anchored to a point or a quote. |
| `comment_list` | List an artifact's comments. |
| `comment_resolve` | Mark a comment done or reopen it. |
| `comment_delete` | Delete a comment. |
| `brain_get` | Read a path from a session brain, the caller's own or another named by `session`, or from a project knowledge base. |
| `brain_put` | Write a path into the active session brain, or a page into a project knowledge base. |
| `brain_list` | List a store's entries, each with its type and size. |
| `brain_delete` | Remove a path from either store. |
| `brain_promote` | Copy an entry from the caller's active session brain into a project knowledge base page that cites the session it came from. |
| `search` | Search feed events, artifacts, session brains, and knowledge base pages, scoped to a project, a session, or global. |
| `whoami` | Report the calling identity and its personal space. |
| `version` | Report the server version, for a connectivity check. |

A publish carries a description, a favicon mark, and a version label.
`artifact_update` accepts the version the edit is based on as
`base_version`: a stale base is refused with a conflict naming the
current version unless `force` is passed, so two writers never silently
overwrite each other. History and deletion follow the same access rules
as reads and writes, and an artifact a caller may not reach reads as
forbidden whether it is missing or denied. Commenting works the same
way: posting needs write access and returns a delete token shown once,
and resolving or deleting needs the token or write access. A quote
anchor is refused on versions the server holds only as ciphertext.

`question_post` returns `event_id`, `question_id`, and `thread_id`, all the
same value: a question roots its own thread and is its own event. `answer_post`
takes that value as `question_id`. An inbox item exposes the same id as its
`event_id`, so a client can answer from either the post response or a read.

The brain tools reach two stores through one `store` argument. `"session"` is
a session's own brain, the working state that is pruned with the session.
`"project"` is the
project knowledge base, the durable store every agent with project write
shares, selected by `project_id` and defaulting to the active session's
project. The argument is required on `brain_put` and `brain_delete`, because a
write that lands in the wrong store is silent either way, and defaults to
`"session"` on the reads, where a wrong guess is a `not_found` the caller
recovers from.

`brain_get` and `brain_list` take an optional `session` naming another session
to read, either `{session_id}` or `{agent, name}` with a `project_id` that
defaults to the active session's project; omitted, it is the active session.
Read access to the target's project is the whole rule, so an agent with
read access to a project reads its sessions. Reading another session
touches neither the caller's active session nor its existence, so a caller that
never started one still reads. A read opens no file that is not already there,
and a session the human has pruned is `not_found` from the moment it is marked.
Once the undo window has passed the row is gone, so the hub can no longer tell
which project it belonged to, and it answers as it does for any session that
never existed. An agent without access cannot tell a session it may not
read from one that does not exist: both are the same `forbidden`.

Writes stay with the owner's active session. `brain_put` and `brain_delete`
accept a `session` only when it names that session, and refuse any other with
`forbidden` and an `owner=` tail, because one working-state file has one writer
and two would clobber each other. Knowledge meant for another agent belongs in
the project knowledge base.

`search` takes `session_id` to narrow results to one session's brain content,
under the same project confinement as every other search. A hit carries its
project's display name and what its family shows: the event kind and actor for
a feed hit, the current version and its size for an artifact, the session's
name and status for a brain entry. Those are read after the result is ranked
and confined, by the ids of the hits alone, and a corpus row shows nothing of
a row another project holds, so no field reaches past what the caller can see.
The served skill document, `GET /SKILL.md`, names the fields.

Paths are namespaced: `/fs/` for the filesystem and `/kv/` for key-value
entries. A knowledge base holds pages only, so a `/kv/` path there is an
`invalid_argument`. No tool exposes a raw file handle or the server path of a
file: `session_start` returns the session id, its owner and status, the two
namespaces to address the brain with, and the conventional recovery path. One value is
capped at 4 MiB and one knowledge base page at 1 MiB, and a larger write is
refused with `payload_too_large` before anything is stored.

A read returns a `version`, the content hash of the bytes it returns, and a
write returns the version of the bytes it stored. Passing one back as
`if_version` makes a write conditional: it applies only while the stored
content still hashes to that value, and otherwise is refused with a `conflict`
whose message ends `current_version=sha256:...`. The literal `absent` creates a
page only when nothing is stored at the path. Without `if_version` the last
writer wins. The comparison and the write happen under the store's writer lock,
so two callers holding the same version cannot both succeed.

## Identity and access

Every HTTP call carries a bearer token bound to a stable identity, and the
server sets the `actor`; a request cannot forge it. A proxy and a one-shot call
are HTTP calls, so they are the token's identity, and `HUB_AGENT_ID` is
advisory there: only the embedded standalone stdio mode carries no token, acts
as the human admin, and takes its actor label from `HUB_AGENT_ID`. A token
reaches every ordinary project and its own personal space; confidential
projects require explicit grants. Global reads are confined to the caller's
visible projects, so a search or an inbox read never crosses a boundary.

## Pagination, errors, and idempotency

Feed cursors are exclusive event ids; `since` walks forward and `before` walks
back, with a default page of 50 and a cap of 500. A page returns `next_since`
(the newest id, for polling forward) and `next_before` (the oldest id, for
paging back). An empty forward poll returns the `since` cursor it was given,
so a polling client keeps its place instead of losing it. Tool errors are
structured (`code`, `message`, `retryable`, `details`) rather than prose. A
resource a caller may not reach returns the same error whether it is missing
or denied, so an agent cannot use an error as an existence check. A write that
creates a durable record elsewhere (a feed event, a question, an answer, an
artifact, or a decision) accepts an optional idempotency key, so a retry after
a dropped connection returns the original result instead of a duplicate. A
question or an approval is an open item on the human, and the hub caps how many
one actor may leave open in a project; a write past the cap is refused with
`rate_limited` and changes nothing, and resolving an item frees its slot. See
[the inbox cap](../adr/0017-inbox-action-item-cap.md).

## Kind families

Event kinds are a closed set of seven design families (`signal`, `finished`,
`question`, `answer`, `approval`, `artifact`, `session`) plus `system`.
Sub-actions ride in the payload, so an artifact has an action of `published`
or `updated` and a session has `started`, `ended`, `adopted`, `forked`, or
`reassigned`.

## Session ownership and pickup

A session belongs to the agent that started it. A name is unique per owner
inside a project, so two agents that choose `nightly` get two sessions and two
brains rather than silently sharing one, and each resumes its own. A name a
pruned session still holds is refused with a `conflict` naming it, because the
human can still undo that prune.

An agent picks work up by passing `from` to `session_start`, naming a session
by id or by agent and name. The hub chooses the mechanism from the source's
state, because the caller cannot tell from outside whether that session is
still running: an ended session is **adopted**, keeping its id, its brain and
its handoff note while ownership moves; a running one is **forked** into a new
session whose brain is copied through the engine, leaving the source untouched.
The result reports which happened and returns the note the previous owner left.
Every agent that may write the project may adopt an ended session there, and
the human approves nothing: adopt, fork, reassign and end-with-handoff all
reach the feed as ordinary session events.

The human's own move is the reverse one: reassigning a running session to
another agent from the control surface, for the case where the agent holding it
is not coming back.

## Bootstrap convention

An agent orients itself with two calls at session start: read the project
feed since it last looked, and read its recovery handoff from the brain. That
is the whole convention. It replaces the system-prompt scaffolding that
individual harnesses use today.

## See also

- [Human surface](human-surface.md) - the same data through the human's eyes
- [Data model](data-model.md) - what these tools read and write
