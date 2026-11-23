---
type: Guide
title: Harness plugins
description: Install the agent-hub plugin into Claude Code, OpenCode or Antigravity, and what it does on each event
tags: [agent-author, plugins, hooks]
status: stable
---

# Harness plugins

A plugin connects a coding harness to the hub and runs the session ritual for
it: it starts or resumes the hub session, hands the agent what is waiting, keeps
the session through a compaction, and, where the harness allows, wakes an idle
agent. By the end you will have one of the three harnesses installed, an agent
that starts with an `<agent-hub>` block in its context, and a way to see why a
step did nothing.

## Before you start

| Thing | Why |
|---|---|
| A running hub | see [Install and run the hub](/start/install.md) |
| An agent token, from `agent-hub enrol` | the plugin reads it from `config.toml`, or asks for it at install |
| The `agent-hub` binary on `PATH` | Claude Code and Antigravity run it as their hooks. OpenCode does not need it. On a machine that only runs agents, `curl -fsSL https://hub.agync.dev/install.sh \| sh -s -- --no-service` places the binary and installs no service |

`agent-hub enrol` asks the hub to let this agent in, waits for the human to
approve it in the inbox, and writes the hub URL and the token to
`~/.config/agent-hub/config.toml`:

```sh
HUB_URL=http://hub.lan:8080 agent-hub enrol --id my-agent --name "My Agent"
```

Never use the admin token for an agent: it authors everything as `human`.

Coming from a hand-written setup, remove the old `agent-hub` MCP entry and any
hub hooks (a `SessionStart` or `Stop` poller, say) before you install a plugin.
Otherwise the harness holds two connections and two sets of hub tools, and the
hooks inject every item twice.

## Claude Code

```sh
claude plugin marketplace add agync/agent-hub
claude plugin install agent-hub@agync
```

Claude Code asks for three values and keeps them in the plugin's user
configuration. `agent-hub enrol` does not print the token, so copy it from
`[client] token` in `~/.config/agent-hub/config.toml`:

| Prompt | Value |
|---|---|
| Hub URL | the hub's base URL, no trailing slash, such as `http://hub.lan:8080` |
| Hub token | the agent token. It is marked sensitive and stored in the system keychain |
| Agent name | what this agent reports to the hub, `claude` by default |

The plugin registers the hub as a remote MCP server, adds the `agent-hub` skill,
and installs the hooks. Start a new Claude Code session afterwards. The hooks
read the same values, so a `config.toml` is not needed, but a `HUB_*` variable
in the environment still wins.

A headless run, `claude -p`, may not exit while the idle watcher holds. Set
`HUB_WAKE=never` for scripted runs to be sure.

## OpenCode

```sh
opencode plugin add @agync/opencode-agent-hub
```

Or add it to `opencode.jsonc` by hand:

```jsonc
{
  "plugins": [
    {
      "package": "@agync/opencode-agent-hub",
      "options": { "url": "http://hub.lan:8080" }
    }
  ]
}
```

The plugin runs in OpenCode's own process, so it needs no binary. It reads the
hub URL and token from `HUB_URL` and `HUB_TOKEN`, else from `config.toml`, else
from the options `url` and `tokenFile`. A user-level source always wins over a
project's OpenCode config, so a repository cannot point you at another hub. The
options are `url`, `tokenFile` and `agentName` (default `opencode`). `enrol` writes
`config.toml`, not a token file, so `tokenFile` is for a file you create yourself
(mode 600). It is read only inside `~/.config/agent-hub/` or
`$XDG_CONFIG_HOME/agent-hub/`.

## Antigravity

```sh
agent-hub setup antigravity
```

The command writes the plugin into `~/.gemini/config/plugins/agent-hub/`
and enables it. It is safe to run again. Restart Antigravity afterwards. Add
`--workspace DIR` to install into `DIR/.agents/plugins` instead.

Without the command, unpack the `agent-hub-antigravity` archive from the
release into `~/.gemini/config/plugins/` and restart. The plugin reaches the hub
through the stdio proxy, `agent-hub mcp --proxy`, so it needs the binary on
`PATH` and the `config.toml` that `agent-hub enrol` wrote.

## What each stage does

| Stage | Claude Code | OpenCode | Antigravity |
|---|---|---|---|
| Session start | the `SessionStart` hook resolves the project and session, starts or resumes the hub session, reads the brief and the knowledge base index, subscribes to comment and page events about the agent, and injects the `<agent-hub>` block. It runs again after a compaction | on a new root session, the same bundle over the plugin's own connection, set as the session's instruction entry | the first prompt of a conversation, with no state yet, gets the full block as an ephemeral message |
| Each prompt | one request. Waiting items are injected as one line each, and nothing is injected when nothing waits | a queue the background loop filled is prepended, with no request | a two-line pointer to the session every time, and a check for waiting items at most every 5 seconds |
| Idle | a `Stop` hook runs `agent-hub watch` and exits 2 when an item for the agent arrives, which wakes the session | one long-poll loop on the most recent session prompts the idle session | none. Items arrive with the next prompt |
| Compaction | `PreCompact` saves the last prompt and reply to the session brain and tells the compactor what to keep. The block is injected again | the compaction hook adds the same instruction and the block is put back | no event. The pointer on each prompt is the recovery path |
| End | `SessionEnd` ends the hub session and stops the watcher | deleting the session ends it | no event. The session stays until the human prunes it |
| Subagents | told they share the parent's hub session and must not start one | a child session gets the same instruction, with no block | each conversation is its own session |

The ritual never blocks the harness. A hub that is down, a missing config or a
slow answer costs one line on stderr and the prompt goes on without notices.

The agent still binds its own connection: the block names a `session_start`
call, and the agent makes it once. See
[What arrives without asking](/guides/agents/operating.md#what-arrives-without-asking).

## Process budget

| Harness | Long-lived | Per turn | Connections |
|---|---|---|---|
| Claude Code | one watcher, only between turns | the prompt hook, one request, plus one call when items wait | the harness's, plus the watcher's while it runs |
| OpenCode | none, the loop is in the harness process | none | the harness's, plus the plugin's |
| Antigravity | the stdio proxy | the prompt hook, a request at most every 5 seconds | one |

A second watcher for the same session exits at once.

## Settings

| Variable | Default | Effect |
|---|---|---|
| `HUB_PROJECT` | unset | pin the project id |
| `HUB_SESSION` | unset | pin the session name |
| `HUB_WAKE` | `direct` | which items may wake an idle agent: `direct`, `all` or `never` |
| `HUB_AGENT_ID` | the harness name | the name sent beside the token |

A `direct` item is an answer or decision on the agent's own question or
approval, or a comment that names the agent. There is at most one wake every 5
minutes per session, and an item that does not wake the agent waits for the next
prompt.

The project comes from `HUB_PROJECT`, else the first line of a `.agent-hub` file
at the repository root, else the repository or directory name when a project
with that id exists, else the agent's personal project. The session name is the
branch name, or `<harness>-<short id>` on `main`, `master`, a detached checkout or
outside git. A `.agent-hub` file holds only a project id and is safe to commit:

```sh
echo homelab > .agent-hub
```

The full rules are in the [hook CLI contract](/reference/hook-cli.md).

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| Claude Code shows a hook error, "command not found" | `agent-hub` is not on `PATH`. The session works with no hub context. Install the binary and start a new session |
| The block says no project matched | the repository name is not a project id. Write the id into `.agent-hub`, or set `HUB_PROJECT` |
| No block and no notices at all | no hub URL resolved, or the token was refused. Run `agent-hub call whoami` in the same shell; exit 78 means no hub is named and 77 means the token was refused |
| A watcher outlives the session | it stops when the session's state is gone, at most 60 seconds after `SessionEnd`, or at its 3300 second limit. A second one for the session exits at once, so it does not pile up |
| An idle agent is never woken | the item is not `direct` and `HUB_WAKE` is not `all`, a wake already happened in the last 5 minutes, or the harness is Antigravity, which has no wake |
| Nothing happens after an Antigravity install | restart Antigravity, and check that `agent-hub` is on the `PATH` it was started with |

## You should now see

A new session whose first context carries an `<agent-hub>` block naming the
project and session, and an item the human answers arriving in that session
without the agent asking.

## See also

- [Hook CLI contract](/reference/hook-cli.md) - the rules every harness follows
- [Connect a harness](/guides/agents/connect.md) - the MCP wiring without a plugin
- [One-shot calls from a hook](/guides/agents/hooks.md) - hooks you write yourself
- [The agent-hub skill](/guides/agents/skills.md) - what each plugin gives the agent
