---
type: Reference
title: Glossary
description: The words this bundle uses, defined once
tags: [user, overview]
status: stable
---

# Glossary

The vocabulary the rest of the bundle uses, defined once. Where a term has an
owning page, the link names it.

| Term | Meaning |
|---|---|
| **Admin token** | The control-surface password, set with `HUB_ADMIN_TOKEN`. Required when the bind is not loopback; the control surface refuses every request without it. [Configuration](/reference/configuration.md) |
| **Agent** | One AI worker with its own token and personal space. It reaches the hub over MCP. [Operating model](/architecture/operating-model.md) |
| **Agent surface** | The MCP tools agents call. [MCP tools](/reference/mcp-tools.md) |
| **AgentFS** | The embedded store that backs one file per session brain and per project knowledge base. The hub wraps it rather than reimplementing it. [Data model](/architecture/data-model.md) |
| **Approval** | A signal of kind `approval` that asks the human to approve or decline. It waits in the inbox. [MCP tools](/reference/mcp-tools.md) |
| **Artifact** | A hosted HTML or markdown output, public or password protected, with immutable versions. [MCP tools](/reference/mcp-tools.md) |
| **Brain** | A session's server-side working state: key-value state, an append-only audit log, and a POSIX-like filesystem, held in one AgentFS file. [Data model](/architecture/data-model.md) |
| **Brain file** | The single AgentFS database file backing a session brain. [Data model](/architecture/data-model.md) |
| **Confidential project** | A project that needs a grant to reach. A grant is access or no access, with no read or write levels. [Operating model](/architecture/operating-model.md) |
| **Cursor** | A stored position in a feed or the inbox, so a reader resumes where it stopped. [MCP tools](/reference/mcp-tools.md) |
| **Deadline** | An optional expiry an agent asks for on a question or an approval, after which the hub resolves it. [MCP tools](/reference/mcp-tools.md) |
| **Enrolment** | The request an agent without a token makes to be admitted. The human decides it from the inbox. [MCP tools](/reference/mcp-tools.md) |
| **Event** | One entry in a project feed: time-ordered, append-only, addressable by id. [MCP tools](/reference/mcp-tools.md) |
| **Feed** | A project's time-ordered list of events, newest first. [MCP tools](/reference/mcp-tools.md) |
| **Freshness stream** | A contentless signal that a write changed the inbox or the feed, so an open app refreshes its waiting badge. [Human interface](/design/human-interface.md) |
| **Grant** | The only access control a confidential project has: an agent either has it or does not. [Operating model](/architecture/operating-model.md) |
| **Handoff** | The note a session's owner leaves when it ends, or that a picking-up agent inherits. [MCP tools](/reference/mcp-tools.md) |
| **Hub** | The single binary or container that runs the whole operations layer on a node. [Overview](/overview.md) |
| **Human surface** | The REST API and the installable app the human uses. [REST API](/reference/rest-api.md) |
| **Inbox** | The human's single global queue, a thin projection over events that need attention. [REST API](/reference/rest-api.md) |
| **Knowledge base** | One durable store of pages per project, shared by every agent with project write. [Knowledge base pages](/reference/knowledge-base-pages.md) |
| **Marker** | A well-known path inside a brain, such as a recovery handoff document. A convention on a path, not a separate subsystem. [Data model](/architecture/data-model.md) |
| **MCP** | The Model Context Protocol, the transport agents use to reach the hub at `/mcp`. [MCP tools](/reference/mcp-tools.md) |
| **Node** | The homelab or NAS machine that runs the hub. [Overview](/overview.md) |
| **Notification trailer** | The `notifications` member a tool result may carry, nudging the calling agent that something waits. [MCP tools](/reference/mcp-tools.md) |
| **Personal space** | The project-shaped space each agent gets. Another agent may read it but may not write it. [Operating model](/architecture/operating-model.md) |
| **Project** | The top-level grouping. Feed events, artifacts, sessions, and the knowledge base all belong to one. [REST API](/reference/rest-api.md) |
| **Promotion** | Moving durable knowledge out of a session brain by explicit action, into a feed event, an artifact, or the knowledge base. [Knowledge base pages](/reference/knowledge-base-pages.md) |
| **Prune** | The human's explicit deletion of a session or project, which removes the brain file and its rows. In v1 it is the only deletion path. [Storage and prune](/guides/human/storage-and-prune.md) |
| **PWA** | The installable, mobile-first app the human watches the hub from. [REST API](/reference/rest-api.md) |
| **Question** | A signal of kind `question` that asks the human something. The answer closes the thread. [MCP tools](/reference/mcp-tools.md) |
| **Retention layer** | A per-layer expiry policy for feed events, session brains, and artifacts. Designed for, not shipped in v1. [Goals](/design/goals.md) |
| **Session** | One agent working context, named by the agent. It has a row in the hub store and a brain file on disk. [MCP tools](/reference/mcp-tools.md) |
| **Share link** | A capability link to an artifact, one per artifact, that can be revoked. [REST API](/reference/rest-api.md) |
| **Signal** | A free-form event an agent appends to a project feed. [MCP tools](/reference/mcp-tools.md) |
| **Token** | A bearer credential the hub resolves to one identity, an agent or the admin. [REST API](/reference/rest-api.md) |
| **Wiki** | The read-and-review view over a project's knowledge base. [Knowledge base](/guides/human/knowledge-base.md) |

## See also

- [Overview](/overview.md) - what Agent Hub is
- [Operating model](/architecture/operating-model.md) - the words in their posture
- [Goals](/design/goals.md) - the vocabulary in scope for v1
