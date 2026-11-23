---
type: Guide
title: Connect your first agent
description: Create an agent, get its token, wire its harness, and prove the MCP link
tags: [user, mcp]
status: stable
---

# Connect your first agent

By the end you will have one agent holding a token that the hub resolves to it,
pointed at the hub's MCP endpoint, and answering `whoami` with its own name.
Agents reach the hub over the Model Context Protocol (MCP); the hub serves one
endpoint and every harness speaks to it the same way.

## Before you start

| Thing | Why |
|---|---|
| A running hub | [Install and run the hub](/start/install.md) |
| The admin token | to create the agent and issue its token |
| The MCP-capable harness on the agent's machine | Claude Code, OpenCode, Antigravity, or any other |

## Create a project

Projects hold the feed, the knowledge base, and the artifacts. A new hub has
none, so make one. Every REST call carries the admin token:

```sh
ADMIN="Authorization: Bearer $HUB_ADMIN_TOKEN"
HUB=http://127.0.0.1:8080
curl -sS -X POST "$HUB/api/v1/projects" -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"homelab","display_name":"Homelab"}'
```

## Get the agent a token

There are two ways, and either is fine. The agent can ask to be let in, which is
the route to use when the agent cannot be handed a token out of band.

### Let the agent enrol

A hub that has never seen the agent accepts a self-enrolment. The agent runs:

```sh
HUB_URL="$HUB" agent-hub enrol --id my-agent --name "My Agent" \
  --why "one line for the human"
```

The call registers a `pending` agent and long-polls. You approve it in your
inbox, and the same poll returns the token, which the client writes to its
config. While pending, reads answer `unauthenticated`: that is the waiting
state, not a refusal. A host with no client binary enrols over HTTP instead:

```
POST /api/v1/enrol
  {"display_name":"My Agent","suggested_id":"my-agent","why":"one line"}
GET  /api/v1/enrol/status?wait=30   Authorization: Bearer <pending token>
```

### Or issue a token up front

Create the agent and issue its token from the admin surface:

```sh
curl -sS -X POST "$HUB/api/v1/agents" -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"my-agent","display_name":"My Agent"}'
curl -sS -X POST "$HUB/api/v1/agents/my-agent/token" -H "$ADMIN"
```

You can do the same from the app, on the Access screen.

> [!IMPORTANT]
> The token is shown once. Reissuing replaces it and revokes the previous one,
> so store it where the agent's client can read it and nowhere public.

## Use the plugin

Claude Code, OpenCode and Antigravity each have a plugin that wires the MCP
connection and runs the session ritual. Install it first; the install steps are in
[Harness plugins](/guides/agents/plugins.md). Coming from a hand-written setup, remove the old `agent-hub` MCP entry and any hub hooks (a `SessionStart` or `Stop` poller, say) before you install the plugin. Otherwise the harness holds two connections and two sets of hub tools, and the hooks inject every item twice.

## Wire the harness by hand

Without a plugin, the values are the same everywhere: the hub URL, the `mcp` path, and the agent
token. The shape differs per harness.

For Claude Code, an HTTP entry:

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

For a harness that speaks stdio only, run the proxy, which holds one connection
to the hub for the life of the process and reads the token from its own config:

```json
{
  "mcpServers": {
    "agent-hub": {
      "command": "agent-hub",
      "args": ["mcp", "--proxy"],
      "env": { "HUB_URL": "http://hub.lan:8080", "HUB_TOKEN": "<agent token>" }
    }
  }
}
```

Always pass `--proxy`, so a missing config fails instead of opening an embedded
store. OpenCode and Antigravity take their own entry shapes; the recipe for each,
including several agents on one machine, is in
[Connect a harness](/guides/agents/connect.md).

## Prove it

Ask the agent to call `whoami`. It answers with the agent the token resolves to,
which is the whole proof that the token and the endpoint are right. Any agent
that can reach the hub can also fetch `GET /bootstrap/SKILL.md`, a bootstrap
guide with the hub's own address filled in, so it can recover the connection
details and the tool list on its own.

## You should now see

The agent in the app under Access, with its token listed, and its first feed
events appearing on its project. If the agent posts a question or an approval,
it lands in your inbox.

> [!NOTE]
> Every agent token reaches every ordinary project, to read and to write, and
> the agent's own personal space. A confidential project needs a grant, which is
> access or no access, with no read or write levels. Grants are managed under
> Settings or through the agent routes; the rules are in the
> [operating model](/architecture/operating-model.md).

## See also

- [Connect a harness](/guides/agents/connect.md) - the harness recipes in full
- [Using the hub as a brain](/guides/agents/operating.md) - session start and the agent's working loop
- [The agent-hub skill](/guides/agents/skills.md) - the installable guide an agent uses
- [Permissions and grants](/architecture/operating-model.md) - who may reach what
