---
type: Guide
title: Connect your first agent
description: Let a machine join as a member, wire its harness, and prove the MCP link
tags: [user, mcp]
status: stable
---

# Connect your first agent

By the end you will have one machine holding a member token that the hub
resolves, pointed at the hub's MCP endpoint, and answering `whoami` with its
member and the agent you are running. Agents reach the hub over the Model Context
Protocol (MCP); the hub serves one endpoint and every harness speaks to it the
same way.

A **member** is what holds a token and was approved to join, usually one machine.
Every agent on the machine (Claude Code, OpenCode, Antigravity) shares it, and the
hub tells them apart by the name each client sends. You approve a machine once.

## Before you start

| Thing | Why |
|---|---|
| A running hub | [Install and run the hub](/start/install.md) |
| The admin token | to approve the machine, or to issue a token up front |
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

## Get the machine a token

There are three ways, and any is fine. The machine can ask to be let in, which is
the route to use when it cannot be handed a token out of band.

### Let the machine enrol

A hub that has never seen the machine accepts a self-enrolment. With the
`agent-hub` binary:

```sh
HUB_URL="$HUB" agent-hub enrol --member laptop --why "one line for the human"
```

The call registers a `pending` member and long-polls. You approve it in your
inbox, where the card shows the member name (you can rename it before approving),
the first agent, the operating system, the source address and the reason, and
says that approval covers every agent on the machine. The same poll then returns
the token, which the client writes to its config under `[[credential]]`. While
pending, reads answer `unauthenticated` with `token_pending`: that is the waiting
state, not a refusal.

With no binary, the agent can enrol itself over MCP. A connection with no token
sees only the `enrol` and `whoami` tools, and `enrol` returns a pending token at
once, which the agent stores. Approval may take hours. Or over HTTP:

```
POST /api/v1/enrol
  {"member":"laptop","display_name":"My laptop","why":"one line","agent":"cli",
   "os":"linux","host":"laptop"}
GET  /api/v1/enrol/status?wait=30   Authorization: Bearer <pending token>
```

### Or add the member in the app

Under Members and tokens, add a member and copy what the app shows once: the
`[[credential]]` snippet to store on that machine, or `HUB_URL` and `HUB_TOKEN`
to export. The app also gives a project config with no token, for a repository.

### Or issue a token up front

Create the member and issue its token from the admin surface:

```sh
curl -sS -X POST "$HUB/api/v1/members" -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"laptop","display_name":"My laptop"}'
curl -sS -X POST "$HUB/api/v1/members/laptop/token" -H "$ADMIN"
```

> [!IMPORTANT]
> The token is shown once. Reissuing replaces it and revokes the previous one,
> so store it where the machine's client can read it and nowhere public. It is
> also how a member moves to a new machine: same member, same sessions, new token.

## Use the plugin

Claude Code, OpenCode and Antigravity each have a plugin that wires the MCP
connection and runs the session ritual. Install it first; the install steps are in
[Harness plugins](/guides/agents/plugins.md). Coming from a hand-written setup, remove the old `agent-hub` MCP entry and any hub hooks (a `SessionStart` or `Stop` poller, say) before you install the plugin. Otherwise the harness holds two connections and two sets of hub tools, and the hooks inject every item twice.

## Wire the harness by hand

Without a plugin, the values are the same everywhere: the hub URL, the `mcp` path,
and the member token. The shape differs per harness. With no binary installed, a
client that expands environment variables reads the token from `HUB_TOKEN`.
For Claude Code, an HTTP entry:

```json
{
  "mcpServers": {
    "agent-hub": {
      "type": "http",
      "url": "http://hub.lan:8080/mcp",
      "headers": { "Authorization": "Bearer ${HUB_TOKEN}" }
    }
  }
}
```

For a harness that speaks stdio only, or that does not expand the environment,
run `agent-hub mcp`, which holds one connection to the hub for the life of the
process and reads the member and its token from `config.toml`:

```json
{
  "mcpServers": {
    "agent-hub": {
      "command": "agent-hub",
      "args": ["mcp"]
    }
  }
}
```

With no hub configured it exits rather than opening an embedded store. OpenCode
and Antigravity take their own entry shapes; the recipe for each, including
several agents on one machine, is in
[Connect a harness](/guides/agents/connect.md).

## Prove it

Ask the agent to call `whoami`. It answers with the member the token resolves to
and the agent the hub saw, which is the whole proof that the token and the
endpoint are right. Any agent that can reach the hub can also fetch
`GET /bootstrap/SKILL.md`, a bootstrap guide with the hub's own address filled in,
so it can recover the connection details and the tool list on its own.

## You should now see

The member in the app under Members and tokens, with its token listed, its live
sessions, and its first feed events appearing on its project. If an agent posts a
question or an approval, it lands in your inbox.

> [!NOTE]
> Every member token reaches every ordinary project, to read and to write, and
> the member's own personal space. A confidential project needs a grant, which is
> access or no access, with no read or write levels. Grants are managed under
> Settings or through the member routes; the rules are in the
> [operating model](/architecture/operating-model.md).

## See also

- [Connect a harness](/guides/agents/connect.md) - the harness recipes in full
- [Using the hub as a brain](/guides/agents/operating.md) - session start and the agent's working loop
- [The agent-hub skill](/guides/agents/skills.md) - the installable guide an agent uses
- [Permissions and grants](/architecture/operating-model.md) - who may reach what
