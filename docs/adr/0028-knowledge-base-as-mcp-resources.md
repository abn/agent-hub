---
type: Decision
title: The knowledge base is a read-only view as MCP resources
description: Why knowledge base pages are served as MCP resources, the URI scheme that names them, and why resources stay a view over the tools rather than a second surface.
tags: [adr, mcp, knowledge-base, agents]
status: stable
---

# 0028. The knowledge base is a read-only view as MCP resources

## Context

An agent reaches the project knowledge base through the brain tools with
`store: "project"` ([ADR 0018](0018-project-knowledge-base.md)). A tool call is
the model's own move: before it can use a page, the model has to decide to call
`brain_list` and `brain_get`, and the page lands in the conversation as a tool
result. Many MCP clients can also attach resources, which the user or the
client puts into context directly, without a tool round trip. The hub already
serves its bootstrap and its skill files as resources, so the capability is
advertised; the knowledge base was not on it.

## Decision

Every knowledge base page the caller may read is a resource, named
`agenthub://kb/<project_id>/<path>`, where `<path>` is the page's path under
`/fs/` with each segment percent-encoded. `resources/templates/list` returns
the one template, `agenthub://kb/{project_id}/{+path}`, for a client that knows
the page it wants. The existing names stay as they are: `agenthub://skill` is
the bootstrap and `skill://agent-hub/` holds the skill.

Resources are a view over the tools, not a second surface:

- A read is `brain_get` on the project store under another name. It takes the
  same access check, so a confidential project needs a grant, a missing project
  answers an agent token as a denied one does, and the operating model
  ([model](../architecture/model.md)) needs no new rule.
- A listing walks the knowledge bases of the projects the caller can see, the
  same set a search or a project listing is confined to.
- Nothing is written and nothing is stored: no subscription, no cache, no new
  table. Writing stays with the tools.

A listing is paged, 100 resources at a time. The bootstrap and the skill files
open the first page, so it holds that many fewer knowledge base pages, and
pages follow in project and path order. A fuller
listing carries a cursor, the URI of the last page it names, and a request with
that cursor resumes after it. The cursor holds no state on the hub, so it
survives a restart, and a page written behind it between two requests is left
out of that walk rather than breaking it.

## Consequences

- A client that attaches resources can put a runbook into context in one step,
  and the stdio proxy forwards the listing, the template list and the read, so
  the same pages arrive however the agent is wired.
- A page an agent may not read is not listed and cannot be read, by the same
  code that refuses it to a tool.
- `resources/subscribe` is not offered. A client that wants a page current
  reads it again; change notification would be state the hub does not keep.
- A listing reads the id and name of each active project, and for an agent
  token which of them it can see, without the feed and session counts a
  project listing carries. It then opens each visible knowledge base from the
  cursor's project on and walks its tree in path order, stopping once it holds
  one page more than it returns. A directory whose every path sorts before the
  cursor is not read. Each directory the walk enters is read whole, names and
  kinds without a size for a child directory, and sorted, so a listing costs
  the directories it passes through. A flat directory of many pages is read and
  sorted on every listing page that reaches into it, since the store hands
  back no directory in name order or from a starting name.
- A knowledge base the engine finds locked fails the listing with a retryable
  error, so the client asks again with the same cursor and misses nothing. One
  that cannot be walked for any other reason, one deleted during the listing or
  a file that cannot be opened, is left out of that listing with a warning in
  the hub's log, so one broken store never blocks the listing of every other.
- A page is served as `text/markdown` when its name ends in `.md` and as
  `text/plain` otherwise, so the template names no media type.
