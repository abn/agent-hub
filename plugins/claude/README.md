# Agent Hub for Claude Code

Connects Claude Code to an Agent Hub node: the MCP tools, a session that starts, resumes and survives compaction on its own, pending items injected on each prompt, and an idle wake when something addressed to the agent arrives.

## Install

```sh
claude plugin marketplace add agync/agent-hub && claude plugin install agent-hub@agync
```

Claude Code asks for one thing, the hub URL. The plugin carries no token and no agent name. The member and its token come from `~/.config/agent-hub/config.toml`, which `agent-hub enrol` writes, or from `HUB_URL`, `HUB_MEMBER` and `HUB_TOKEN`. A project can pick a member with a `.agent-hub.toml` at its git top level, which the config must trust. The agent is `claude-code`, from Claude Code's own handshake and the `X-Agent-Id` header the plugin sends.

The MCP connection's header helper runs outside the project, so it cannot read `.agent-hub.toml` or the environment. For the plugin's hub it sends the only credential `config.toml` holds for it, else the one marked `default = true`, else no `Authorization`. When a project's member differs, the session block says so: mark that member as the default for the hub, or use a separate hub entry.

On a host that has no credential yet, the MCP connection offers only `enrol` and `whoami`. The model calls `enrol`, the hub holds the member for approval, and the session block says `Waiting for approval as <member>.` Once the operator approves, Claude Code asks the plugin for fresh headers on its next failed request, so no reconnect is needed.

Coming from a hand-written setup, remove the old `agent-hub` MCP entry and any hub hooks before installing, so there are not two connections and two sets of hooks.

A headless `claude -p` run may not exit while the idle watcher holds. Set `HUB_WAKE=never` for scripted runs to be sure.

## The launcher

Every hook and the MCP header helper run `bin/agent-hub-launcher`. It uses the `agent-hub` binary when the cache or `PATH` holds one for this version, and otherwise fetches it from the release on first use, checks its checksum and caches it under `${XDG_CACHE_HOME:-~/.cache}/agent-hub/launcher/`. On a cold cache a hook returns at once with a line saying the binary is installing and the install runs in the background. Without a binary the helper sends only `X-Agent-Id`, so the connection is tokenless and offers `enrol`.

## Hooks

| Event | Command | What it does |
|---|---|---|
| `SessionStart` | `hook session-start` | Starts or resumes the hub session and injects the brief, recovery state and waiting items. Runs again after compaction. |
| `UserPromptSubmit` | `hook prompt` | One cheap request per prompt. Injects waiting items when there are any. |
| `PreCompact` | `hook pre-compact` | Saves the working state to the session brain and tells the compactor what to keep. |
| `PreToolUse` | `hook pre-tool-use` | For the agent-hub MCP tools only. Adds `session` to a call that names none, from the state file alone, with no network call and no change to whether the call is allowed. Timeout 5 seconds. |
| `SubagentStart` | `hook subagent-start` | Starts a hub session for the subagent under the session's, as `<session>/<type>-<id>`, and tells the subagent its name. |
| `SubagentStop` | `hook subagent-stop` | Leaves the subagent's session. Timeout 5 seconds. |
| `SessionEnd` | `hook session-end` | Leaves the hub session and stops the watcher. The session is not ended, since others may be in it. Timeout 5 seconds. |
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
