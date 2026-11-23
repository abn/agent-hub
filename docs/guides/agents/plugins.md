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
| A member token, or none yet | the plugin finds it in `config.toml` or `HUB_TOKEN`. With none, the agent connects to the enrol surface and can ask the human to let the machine in |
| The `agent-hub` binary | the hooks run it. Claude Code and Antigravity plugins fetch it themselves on first use; OpenCode does not need it. Install it yourself on a machine that only runs agents with `curl -fsSL https://hub.agync.dev/install.sh \| sh -s -- --no-service`, which installs no service |

`agent-hub enrol` asks the hub to let this machine in, waits for the human to
approve it in the inbox, and writes the hub URL, the member and the token to
`~/.config/agent-hub/config.toml`. Every agent on the machine then shares that
member, so you do it once per machine:

```sh
HUB_URL=http://hub.lan:8080 agent-hub enrol --member laptop
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

Claude Code asks for the hub URL once and keeps it in the plugin's user
configuration:

| Prompt | Value |
|---|---|
| Hub URL | the hub's base URL, no trailing slash, such as `http://hub.lan:8080` |

It asks for no token and no agent name. The agent is `claude-code`, which the
client sends in its handshake. The token comes from `config.toml` or
`HUB_TOKEN`, through a header helper that runs the plugin's launcher, and the
launcher reads the same credential the hooks do. With no token yet, the connection
is the enrol surface: the agent calls `enrol`, and once the human approves, the
plugin picks the token up on the next connection with no reconnect step.

The plugin registers the hub as a remote MCP server, adds the `agent-hub` skill,
and installs the hooks. Start a new Claude Code session afterwards. The hooks
use the same resolver as the MCP connection, so they act as the same member.

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
hub URL, the member and the token from `HUB_URL`, `HUB_MEMBER` and `HUB_TOKEN`,
else from `config.toml`, else from the option `url`. A user-level source always
wins over a project's OpenCode config, so a repository cannot point you at
another hub. The agent is `opencode`, from the client's handshake. When the agent
calls `enrol`, the plugin stores the pending token itself, checks for approval on
each message, and re-keys the credential if the human renamed the member; after
approval it reconnects, or asks you to restart OpenCode if it cannot.

## Antigravity

```sh
agent-hub setup antigravity
```

The command writes the plugin into `~/.gemini/config/plugins/agent-hub/`
and enables it. It is safe to run again. Restart Antigravity afterwards. Add
`--workspace DIR` to install into `DIR/.agents/plugins` instead.

Without the command, unpack the `agent-hub-antigravity` archive from the
release into `~/.gemini/config/plugins/` and restart. The plugin reaches the hub
through `agent-hub mcp`, so it needs the binary, which its launcher fetches on
first use, and the `config.toml` that `agent-hub enrol` wrote. The command writes
the launcher's absolute path as the entry's command.

## What each stage does

| Stage | Claude Code | OpenCode | Antigravity |
|---|---|---|---|
| Session start | the `SessionStart` hook resolves the project and session, starts or resumes the hub session, reads the brief and the knowledge base index, subscribes to comment and page events about the participant, and injects the `<agent-hub>` block. It runs again after a compaction | on a new root session, the same bundle over the plugin's own connection, set as the session's instruction entry | the first prompt of a conversation, with no state yet, gets the full block as an ephemeral message |
| Each prompt | one request. Waiting items are injected as one line each, and nothing is injected when nothing waits | a queue the background loop filled is prepended, with no request | a two-line pointer to the session every time, and a check for waiting items at most every 5 seconds |
| Idle | a `Stop` hook runs `agent-hub watch` and exits 2 when an item for the agent arrives, which wakes the session | one long-poll loop on the most recent session prompts the idle session | none. Items arrive with the next prompt |
| Hub calls | `PreToolUse` adds the session to each hub tool call that does not name one | the plugin's tool hook adds it | the agent passes `session` itself, from the block |
| Compaction | `PreCompact` saves the last prompt and reply to the session brain and tells the compactor what to keep. The block is injected again | the compaction hook adds the same instruction and the block is put back | no event. The pointer on each prompt is the recovery path |
| End | `SessionEnd` leaves the hub session and stops the watcher. The session stays open for its other participants | deleting the session leaves it | no event. The session stays until a participant ends it or the human prunes it |
| Subagents | each works in its own `<parent>/<task>` session, started on `SubagentStart` and left on `SubagentStop`, and its hub calls carry it | a child session gets its own `<parent>/<task>` session the same way | each conversation is its own session |

The ritual never blocks the harness. A hub that is down, a missing config or a
slow answer costs one line on stderr and the prompt goes on without notices.

The agent uses the session the block names. The plugin adds it to the agent's hub
calls where the harness lets it, and the block says so. See
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
| `HUB_AGENT_ID` | the harness's own name | overrides the agent name the hooks send |
| `HUB_MEMBER` | unset | selects a member when the machine holds several |

A `direct` item is an answer or decision on the participant's own question or
approval, or a comment that names its session. There is at most one wake every 5
minutes per session, and an item that does not wake the agent waits for the next
prompt.

The project comes from `HUB_PROJECT`, else the `project` key of an
`.agent-hub.toml` file at the repository root, else the repository or directory
name when a project with that id exists, else the member's personal project. The
session name is the branch name, or on `main`, `master`, a detached checkout or
outside git, the worktree's directory name, or `<agent>-<YYYY-MM-DD>`. An
`.agent-hub.toml` holds a project id and optionally the hub URL and a member,
never a token, and is safe to commit:

```toml
project = "homelab"
```

The full rules are in the [hook CLI contract](/reference/hook-cli.md).

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| The block says "Hooks are off" | the plugin could not get the binary: the line names the reason (an unsupported platform, a failed download, a checksum mismatch). The hub tools still work. The launcher retries after an hour, or install the binary yourself |
| The block says no project matched | the repository name is not a project id. Write the id into `.agent-hub.toml`, or set `HUB_PROJECT` |
| No block and no notices at all | no hub URL resolved, or the token was refused. Run `agent-hub call whoami` in the same shell; exit 78 means no hub is named and 77 means the token was refused |
| A watcher outlives the session | it stops when the session's state is gone, at most 60 seconds after `SessionEnd`, or at its 3300 second limit. A second one for the session exits at once, so it does not pile up |
| An idle agent is never woken | the item is not `direct` and `HUB_WAKE` is not `all`, a wake already happened in the last 5 minutes, or the harness is Antigravity, which has no wake |
| The block says to call `enrol`, or that approval is pending | the machine has no token, or the human has not approved it yet. Approval covers every agent on the machine |
| Nothing happens after an Antigravity install | restart Antigravity, and check that the plugin's launcher is executable |

## You should now see

A new session whose first context carries an `<agent-hub>` block naming the
project and session, and an item the human answers arriving in that session
without the agent asking.

## See also

- [Hook CLI contract](/reference/hook-cli.md) - the rules every harness follows
- [Connect a harness](/guides/agents/connect.md) - the MCP wiring without a plugin
- [One-shot calls from a hook](/guides/agents/hooks.md) - hooks you write yourself
- [The agent-hub skill](/guides/agents/skills.md) - what each plugin gives the agent
