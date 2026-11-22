# Agent Hub for Antigravity

Connects Antigravity to an Agent Hub node: the MCP tools, a hub session that starts and resumes on its own, waiting items injected before each model call, and the `agent-hub` skill.

## Install

```sh
agent-hub setup antigravity
```

This writes the plugin to `~/.gemini/config/plugins/agent-hub/` from files embedded in the binary and sets `plugins.agent-hub.enabled` in `~/.gemini/config/config.json`. Then restart Antigravity, because it builds its hook registry at start.

`agent-hub setup antigravity --workspace DIR` writes to `DIR/.agents/plugins/agent-hub/` instead. It still enables the plugin in the user config.

Running it again changes nothing. A file it replaces, and the config before it is edited, are copied to `agent-hub-backup/` beside the plugin directory first. Other keys in the config are kept. A plugin directory that is a symlink is refused.

A release archive, `agent-hub-antigravity-<version>.tar.gz`, holds the same plugin directory for `agy plugin install` or a marketplace. `make plugins/antigravity-dist` builds it.

## Prerequisites

| Need | Why |
|---|---|
| `agent-hub` on `PATH` | The MCP server and the hook both run the binary. |
| `config.toml` from `agent-hub enrol` | The plugin carries no URL or token. The proxy and the hook read them from `~/.config/agent-hub/config.toml`, or from `HUB_URL` and `HUB_TOKEN`. |

The MCP server runs `agent-hub mcp --proxy` as agent `antigravity`. With `--proxy` it never falls back to the local data directory, so it cannot open a store a serving hub already holds.

## Hook

| Event | Command | What it does |
|---|---|---|
| `PreInvocation` | `agent-hub hook prompt --harness antigravity` | On the first call of a conversation, starts or resumes the hub session and injects the brief and waiting items. On every later call, injects a short session pointer and any waiting items. |

Antigravity has no session start, compaction, subagent or end event, so the per-call pointer is how a conversation recovers. The hook always exits 0 and prints `{}` on any failure, so a hub outage never blocks the model.

The hook timeout is `10`. Antigravity documents hook timeouts in seconds, default 30. Another locally installed plugin sets `10000`, which reads as milliseconds; that was not measured here, and the value this plugin sets is correct only if the documented unit holds.

## Rules and skill

`rules/agent-hub.md` has `trigger: always_on` and tells the model to bind the session with `session_start`, write durable findings to the knowledge base, and ask the human through the inbox. `skills/agent-hub/` is the same skill `npx skills add agync/agent-hub` installs.

## Not provided

No idle wake. Antigravity has no way for a background process to start a turn, so items reach the model on its next call.
