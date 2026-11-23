# Agent Hub for Claude Code

Connects Claude Code to an Agent Hub node: the MCP tools, a session that starts, resumes and survives compaction on its own, pending items injected on each prompt, and an idle wake when something addressed to the agent arrives.

## Install

```sh
claude plugin marketplace add agync/agent-hub && claude plugin install agent-hub@agync
```

Claude Code asks for the hub URL, the agent token (kept in the system keychain) and an agent name (default `claude`). `agent-hub enrol` does not print the token; copy it from `[client] token` in `~/.config/agent-hub/config.toml`.

Coming from a hand-written setup, remove the old `agent-hub` MCP entry and any hub hooks before installing, so there are not two connections and two sets of hooks.

A headless `claude -p` run may not exit while the idle watcher holds. Set `HUB_WAKE=never` for scripted runs to be sure.

## Prerequisites

The hooks run the `agent-hub` binary, so it must be on `PATH`. The MCP connection and the skill work without it.

If the binary is missing, each hook fails with a "command not found" error that Claude Code shows as a hook error. The session keeps working, with no hub context injected and no idle wake. Installing the binary and starting a new session fixes it.

## Hooks

| Event | Command | What it does |
|---|---|---|
| `SessionStart` | `hook session-start` | Starts or resumes the hub session and injects the brief, recovery state and waiting items. Runs again after compaction. |
| `UserPromptSubmit` | `hook prompt` | One cheap request per prompt. Injects waiting items when there are any. |
| `PreCompact` | `hook pre-compact` | Saves the working state to the session brain and tells the compactor what to keep. |
| `SubagentStart` | `hook subagent-start` | Tells a subagent it shares the parent's hub session. |
| `SessionEnd` | `hook session-end` | Ends the hub session and stops the watcher. Timeout 5 seconds. |
| `Stop` | `watch --max-seconds 3300` | Runs with `asyncRewake` while the session is idle. Exits 2 to wake Claude when an item addressed to the agent arrives. Timeout 3600 seconds. |

The behaviour of each command is specified in the hook CLI reference in the wiki.

## Process budget

| Process | Lifetime |
|---|---|
| Claude's MCP connection | one per session |
| Watcher | at most one per session, only between turns, and a second one exits at once |
| Hook commands | short-lived, one at a time |

## Skill

The `agent-hub` skill is a copy of the repository's `skills/agent-hub`, because an installed plugin does not follow a link out of its folder. `make plugins/sync` refreshes it, and `make plugins/check` fails when it drifts.
