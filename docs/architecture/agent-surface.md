---
type: Reference
title: Agent surface
description: The MCP tools agents call, and the session bootstrap convention.
tags: [architecture, mcp, agents, tools]
status: draft
---

# Agent surface

Agents reach the hub over the Model Context Protocol, over stdio for local
agents and streamable HTTP for remote agents on a LAN or tailnet. One MCP
server exposes the tools below. All of it is intended design; none of it
ships yet.

## Tools

| Tool | Purpose |
|---|---|
| `feed_read` | Read a project feed, newest first, optionally since a point and filtered by kind. |
| `signal_append` | Append an event to a project feed. |
| `inbox_read` | Read the human's global inbox, optionally by status. |
| `question_post` | Ask the human or another agent a question. It lands in the inbox and the feed. |
| `answer_post` | Reply to a question. The answer lands in the feed and marks the question acted on. |
| `artifact_publish` | Publish an HTML or markdown artifact, optionally password protected. |
| `artifact_update` | Publish a new version of an existing artifact. |
| `artifact_get` | Read an artifact's content and metadata. |
| `artifact_list` | List a project's artifacts. |
| `brain_get` | Read a file or key-value path from the current session brain. |
| `brain_put` | Write a file or key-value entry into the session brain. |
| `brain_list` | List the session brain tree. |
| `brain_delete` | Remove a path from the session brain. |
| `session_start` | Register a session. Idempotent, so a resume reuses the same brain. |
| `session_end` | Mark a session ended. The brain is retained until the human prunes it. |
| `search` | Search feed events, artifacts, and session contents, scoped to a project or global. |

`brain_get` and `brain_put` operate on the current session's brain only. A
session cannot reach another session's brain, and no tool exposes a raw file
handle.

## Bootstrap convention

An agent orients itself with two calls at session start: read the project
feed since it last looked, and read its recovery handoff from the brain. That
is the whole convention. It replaces the system-prompt scaffolding that
individual harnesses use today.

## See also

- [Human surface](human-surface.md) - the same data through the human's eyes
- [Data model](data-model.md) - what these tools read and write
