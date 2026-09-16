---
type: Reference
title: Terminology
description: The canonical vocabulary used across the design and the code.
tags: [design, terminology]
status: draft
---

# Terminology

Use these terms with these meanings everywhere, in code, docs, and
conversation.

| Term | Meaning |
|---|---|
| **Hub** | The single binary or container that runs the whole operations layer on a node. |
| **Project** | The top-level grouping. Feed events, artifacts, and sessions all belong to a project, except the inbox, which is global. |
| **Event** | One entry in a project feed: time-ordered, append-only, addressable by id. |
| **Feed** | A project's time-ordered list of events, newest first. |
| **Inbox** | The human's single global queue, a thin projection over events that need attention. |
| **Session** | One agent working context, named by the agent. It has metadata in the hub store and a brain file on disk. |
| **Brain** | A session's server-side working state: key-value state, an append-only audit log, and a POSIX-like filesystem, held in one AgentFS file. |
| **Brain file** | The single AgentFS database file backing a session brain. |
| **Marker** | A well-known path inside a brain, such as a recovery handoff document. A convention on a path, not a separate subsystem. |
| **Agent surface** | The MCP tools agents call. |
| **Human surface** | The REST API and the installable PWA the human uses. |
| **Promotion** | Moving durable knowledge out of a brain by explicit action, into a feed event, an artifact, or a memory write. |
| **Prune** | The human's explicit deletion of a session or project, which removes the brain file and its rows. In v1 it is the only deletion path. |
| **Retention layer** | A per-layer expiry policy (feed events, session brains, artifacts). Designed for, not shipped in v1. |

## See also

- [Goals and non-goals](goals.md) - the vocabulary in scope for v1
- [Decision records](../adr/index.md) - the reasoning behind the terms
