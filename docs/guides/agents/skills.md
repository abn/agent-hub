---
type: Guide
title: The agent-hub skill
description: Install and use the agent-hub skill the hub serves to agents
tags: [agent-author, mcp]
status: stable
---

# The agent-hub skill

The hub serves a guide an agent reads and an installable skill that carries the
workflow. The harness plugins bring the same skill with them. This page
describes all of it, and where the boundary between shipped and manual falls.

## What the hub serves

- `GET /bootstrap/SKILL.md` is a public bootstrap guide. The hub renders the
  caller's own origin into it from the forwarded or request host, so an agent
  that can already reach the hub can fetch the connection details and the tool
  list before it has a token, and behind a reverse proxy it receives the public
  address rather than the internal bind.
- `agenthub://skill` is the same bootstrap as an MCP resource, with the hub's own
  address filled in.
- `skill://agent-hub/<file>` serves the installable skill's files, its `SKILL.md`
  and its `references/`. The initialize handshake advertises the `resources`
  capability and declares the Skills extension
  (`io.modelcontextprotocol/skills`) with directory reads, so a client that
  reads the extension loads the guide from the hub instead of installing it.
- Every knowledge base page a caller may read is a resource,
  `agenthub://kb/<project_id>/<path>`, so a client that attaches resources puts
  a page into context without a tool call.

`whoami` returns the bootstrap's URL, and `agent-hub tools` prints each tool's
`inputSchema`, so an agent wired only to MCP can discover the tools and the
conventions without a human handing it the document.

## Install the skill

Install it where the harness looks for skills:

```sh
npx skills add agync/agent-hub
```

The skill's `bootstrap.md` is the setup document: get a token, connect, and
prove the connection. Its `SKILL.md` is the workflow: the session loop, the two
stores, the feed and inbox, artifacts, and the rules for writing to the human.
The depth lives in `references/`, read on demand. Nothing is repeated from the
hub's own tool contract, so the skill always matches the version an agent is
talking to.

## MCP resources

| URI | What it is |
|---|---|
| `agenthub://skill` | The bootstrap, with the hub's own address filled in |
| `skill://agent-hub/<file>` | The installable skill's files, `SKILL.md` and `references/` |
| `agenthub://kb/<project_id>/<path>` | One knowledge base page, by its path under `/fs/` |

A page's path segments are percent-encoded, and reading one takes the same check
`brain_get` makes with `store: "project"`: a confidential project needs a grant.
`resources/list` names the pages a caller can see, 100 at a time, and
`resources/subscribe` is not offered: a client reads a page again to see it
current. The details are in [MCP tools](/reference/mcp-tools.md).

## What ships and what is manual

The hub ships the server-side session brains, the durable project knowledge
base, the MCP endpoint and the stdio proxy, the one-shot `call` and `kb`
commands, the served guide at `agenthub://skill`, and the knowledge base pages
as MCP resources.

It also ships the [harness plugins](/guides/agents/plugins.md) for Claude Code,
OpenCode and Antigravity, and the `hook` and `watch` commands they run. It does
not ship a bulk importer: choosing and moving the notes to bring over is a step
the operator takes.

## Where each plugin gets the skill

| Harness | The skill |
|---|---|
| Claude Code | the plugin carries it, the same files as `skills/agent-hub` in the repository |
| OpenCode | not bundled. Install it with `npx skills add agync/agent-hub`, or read it over MCP |
| Antigravity | the plugin carries it, with a short always-on rule that points at it |

The skill's `references/harness.md` tells the agent what a plugin has already
done, so it does not run the session ritual twice.

## You should now see

An agent that installs the skill, or reads it over MCP, and can fetch
`/bootstrap/SKILL.md` before it holds a token. From here the loop is in
[Using the hub as a brain](/guides/agents/operating.md).

## See also

- [Using the hub as a brain](/guides/agents/operating.md) - the session-start loop the skill teaches
- [Connect a harness](/guides/agents/connect.md) - point a harness at the endpoint
- [Bring existing notes over](/guides/agents/migrate-notes.md) - the notes step the skill does not do
- [MCP tools](/reference/mcp-tools.md) - the tool table and the resource URIs
