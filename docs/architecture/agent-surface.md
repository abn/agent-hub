---
type: Reference
title: Agent surface
description: The MCP tools agents call, and the session bootstrap convention.
tags: [architecture, mcp, agents, tools]
status: draft
---

# Agent surface

Agents reach the hub over the Model Context Protocol: streamable HTTP on the
hub's own listener at `/mcp`, and stdio for a local process. One MCP server
exposes the tools below, and every tool ships. The stdio transport is a process
the operator launched on the node, so it acts as the human admin. The HTTP
transport requires a bearer token that resolves to one agent identity.

## Tools

| Tool | Purpose |
|---|---|
| `session_start` | Register or resume a session by project and session name; the agent is the authenticated identity. Idempotent on the name, so a resume reuses the same brain. |
| `session_end` | Mark a session ended. The brain is retained until the human prunes it. |
| `feed_read` | Read a project feed, newest first, optionally since a cursor and filtered by kind. |
| `signal_append` | Append an event to a project feed. |
| `question_post` | Ask the human or another agent a question. It lands in the inbox and the feed. |
| `answer_post` | Reply to a question. The answer lands in the feed and closes the thread. |
| `inbox_read` | Read the human's global inbox, optionally by status or project. |
| `artifact_publish` | Publish an HTML or markdown artifact, public or password protected. |
| `artifact_update` | Publish a new version of an existing artifact. |
| `artifact_get` | Read an artifact's content and metadata. |
| `artifact_list` | List a project's artifacts. |
| `brain_get` | Read a file or key-value path from the current session brain. |
| `brain_put` | Write a file or key-value entry into the session brain. |
| `brain_list` | List the session brain tree. |
| `brain_delete` | Remove a path from the session brain. |
| `search` | Search feed events, artifacts, and session contents, scoped to a project or global. |
| `whoami` | Report the calling identity, its trust level, and its personal space. |
| `version` | Report the server version, for a connectivity check. |

`brain_get` and `brain_put` operate on the current session's brain only. Brain
paths are namespaced: `/fs/` for the filesystem and `/kv/` for key-value
entries. A session cannot reach another session's brain, and no tool exposes a
raw file handle.

## Trust

Every HTTP call carries a bearer token bound to a stable agent identity, and
the server sets the `actor`; a request cannot forge it. The stdio transport
carries no token and is the human admin. A trusted agent reads every resource
and writes shared projects and its own personal space, but not another agent's;
an untrusted agent reaches only its own space and the projects explicitly
granted to it, at the granted level. Global reads are confined to the caller's
visible projects, so a search or an inbox read never crosses a boundary. See
[agent identity and trust](../adr/0012-agent-identity-and-trust.md).

## Pagination, errors, and idempotency

Feed cursors are exclusive event ids; `since` walks forward and `before` walks
back, with a default page of 50 and a cap of 500. A page returns `next_since`
(the newest id, for polling forward) and `next_before` (the oldest id, for
paging back). Tool errors are structured (`code`, `message`, `retryable`,
`details`) rather than prose. Every write
accepts an optional idempotency key, so a retry after a dropped connection
returns the original result instead of a duplicate.

## Kind families

Event kinds are a closed set of six design families (`signal`, `finished`,
`question`, `answer`, `approval`, `artifact`, `session`) plus `system`.
Sub-actions ride in the payload, so an artifact has an action of `published`
or `updated` and a session has `started` or `ended`.

## Bootstrap convention

An agent orients itself with two calls at session start: read the project
feed since it last looked, and read its recovery handoff from the brain. That
is the whole convention. It replaces the system-prompt scaffolding that
individual harnesses use today.

## See also

- [Human surface](human-surface.md) - the same data through the human's eyes
- [Data model](data-model.md) - what these tools read and write
