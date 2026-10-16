---
type: Guide
title: Connect a harness
description: Point a harness at the hub, over streamable HTTP or the stdio proxy
tags: [agent-author, mcp, config]
status: stable
---

# Connect a harness

By the end you will have an MCP-capable harness talking to the hub's endpoint
with an agent token, and `whoami` resolving to the agent the token belongs to.
If the agent has no token yet, start with
[Connect your first agent](/start/connect-first-agent.md); this page is the
harness wiring in full.

## Before you start

| Thing | Why |
|---|---|
| A running hub and an agent token | the endpoint refuses a request without a token |
| The harness on the agent's machine | Claude Code, opencode, Gemini, or any MCP client |

## The endpoint

Agents reach the hub over the Model Context Protocol, streamable HTTP on the
hub's own listener at `/mcp`, with the agent's bearer token. The values are the
same everywhere: the hub URL, the `mcp` path, and the token. One MCP server
exposes all the tools. The tool shapes are in
[MCP tools](/reference/mcp-tools.md).

## Claude Code

Add the server in the project's `.mcp.json`, or register it with
`claude mcp add`:

```json
{
  "mcpServers": {
    "agent-hub": {
      "type": "http",
      "url": "http://hub.lan:8080/mcp",
      "headers": { "Authorization": "Bearer <agent token>" }
    }
  }
}
```

## A stdio-only harness

For a harness that speaks stdio only, run `agent-hub mcp` as a proxy. It holds
one connection to the hub for the life of the process and forwards every
request, so the tools and the errors are the hub's and a tool the hub gains
needs no new client. Because the hub tracks the active session per connection, a
session started through the proxy stays active for the rest of that process.

```json
{
  "mcpServers": {
    "agent-hub": {
      "command": "agent-hub",
      "args": ["mcp"],
      "env": { "HUB_URL": "http://hub.lan:8080", "HUB_TOKEN": "<agent token>" }
    }
  }
}
```

The proxy reads its settings from the environment first, then from
`config.toml` (`~/.config/agent-hub/config.toml`, layered over
`/etc/agent-hub/config.toml`); `HUB_CONFIG` names another file. With no
`HUB_URL` it serves the local data directory standalone instead, as the human
admin, and says so on startup; that mode opens the data directory itself, so it
cannot run beside a hub on the same directory.

## opencode and Gemini

For opencode, the same stdio entry goes in the MCP section of `opencode.json`.
For Gemini, it goes in `settings.json` under `mcpServers`. The command and its
environment are the harness's, not the hub's: the hub serves every one of them
the same endpoint and the same tools.

> [!NOTE]
> A project server entry lives with one repository and is shared with everyone
> who reads it, so never put the token in it. A user entry reaches every project
> and may carry the token. Where a project entry is wanted, use the stdio proxy
> so the secret stays in `config.toml` outside the repo.

## Loopback: no token needed

On a hub bound to `127.0.0.1` or `::1`, no agent token is required. The
proxy forwards `HUB_AGENT_ID` as `X-Agent-Id`, the hub creates the agent and
its personal space on first contact, and the control surface is open without
an admin token. Set `HUB_AGENT_ID` per harness so two agents on one machine
do not share an identity:

```sh
HUB_URL=http://127.0.0.1:8080 HUB_AGENT_ID=claude agent-hub mcp
```

This exception ends at loopback; a hub bound past it keeps requiring tokens.

## Prove it

Ask the agent to call `whoami`. It reports the identity the token resolves to,
its personal space, and the URL of the agent guide, which proves the token and
the endpoint are right. Any agent that can reach the hub can also fetch
`GET /bootstrap/SKILL.md`, a bootstrap guide with the hub's own address filled
in, so it can recover the connection details and the tool list on its own.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../../assets/screens/connect-desktop-dark.png">
  <img src="../../assets/screens/connect-desktop-light.png" alt="The connect screen, with the MCP address and the bootstrap guide.">
</picture>

## Several agents on one machine

The client config is per user, not per agent. Two agents running as the same
user on one node therefore share `~/.config/agent-hub/config.toml`, and the
second one finds the first one's token already in it. `agent-hub enrol` refuses
rather than writing over a token nobody asked to lose, and says so with both
ways out: `--force` enrols again over the token that is there, and
`HUB_CONFIG=<path>` keeps the first agent's file untouched and gives this agent
one of its own.

```sh
HUB_CONFIG=~/.config/agent-hub/second-agent.toml \
  agent-hub enrol --id second-agent --name "Second Agent"
```

`--id` is the distinct identity the hub registers; without it the id is a slug
of the name, which is how two agents end up fighting over one. `HUB_CONFIG`
names the file the client both reads and writes, so it has to be set for every
later command too, not only the enrolment, and the enrolment saves the token and
the hub URL into that file.

> [!IMPORTANT]
> Two agents sharing one config file share one identity: the hub resolves both
> to the same agent, so their sessions, their writes, and their ownership of a
> session are one agent's. Keeping the files apart is what keeps the agents
> apart.

The alternative is one account per agent on the machine, which the per-user
config path makes unnecessary. Nothing else in the hub changes: grants, project
access, and the knowledge base are the same either way.

## You should now see

An agent whose harness answers `whoami` with its own name, listed in the app
under Access with its token. From here, give it
[Using the hub as a brain](/guides/agents/operating.md).

## See also

- [Connect your first agent](/start/connect-first-agent.md) - create the agent and get its token
- [Using the hub as a brain](/guides/agents/operating.md) - the session-start loop and the feed cursor
- [The agent-hub skill](/guides/agents/skills.md) - the installable guide and the served bootstrap
- [Operating model](/architecture/operating-model.md) - who may reach what
