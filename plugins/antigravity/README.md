# Agent Hub for Antigravity

Connects Antigravity to an Agent Hub node: the MCP tools, a hub session that starts and resumes on its own, waiting items injected before each model call, and the `agent-hub` skill.

## Install

```sh
agent-hub setup antigravity
```

This writes the plugin, including `bin/agent-hub-launcher` (mode 755), to `~/.gemini/config/plugins/agent-hub/` from files embedded in the binary and sets `plugins.agent-hub.enabled` in `~/.gemini/config/config.json`. Then restart Antigravity, because it builds its hook registry at start.

`agent-hub setup antigravity --workspace DIR` writes to `DIR/.agents/plugins/agent-hub/` instead. It still enables the plugin in the user config.

Running it again changes nothing. A file it replaces, and the config before it is edited, are copied to `agent-hub-backup/` beside the plugin directory first. Other keys in the config are kept. A plugin directory that is a symlink is refused.

Coming from a hand-written setup, remove the old `agent-hub` entry from your `mcp_config.json` and any hub hooks first, so there are not two connections and two sets of hooks.

A release archive, `agent-hub-antigravity-<version>.tar.gz`, holds the same plugin directory for `agy plugin install` or a marketplace. `make plugins/antigravity-dist` builds it.

## Prerequisites

| Need | Why |
|---|---|
| Network on first use, or `agent-hub` on `PATH` | The plugin ships `bin/agent-hub-launcher`. It runs the `agent-hub` binary from the cache or `PATH` when the version matches, and otherwise downloads the release for this plugin's version, checks its checksum and caches it. |
| `config.toml` from `agent-hub enrol` | The plugin carries no URL or token. The proxy and the hook read them from `~/.config/agent-hub/config.toml`, or from `HUB_URL`, `HUB_MEMBER` and `HUB_TOKEN`. |

The MCP server is a stdio `agent-hub mcp` with no `env` block: Antigravity expands no variable in a command, an argument list or an `env` value, so nothing in the entry could carry a URL or a token. `agent-hub setup antigravity` writes the installed launcher's absolute path as the command; a plugin installed by `agy plugin install` keeps `agent-hub` and needs the binary on `PATH`. The server proxies to the hub and never falls back to the local data directory. The agent is `antigravity`, from Antigravity's handshake.

With no credential yet the proxy offers `enrol`; after the operator approves, it swaps to the member's token without a restart.

## Hook

| Event | Command | What it does |
|---|---|---|
| `PreInvocation` | `./bin/agent-hub-launcher hook prompt --harness antigravity` | On the first call of a conversation, starts or resumes the hub session and injects the brief and waiting items. On every later call, injects a short session pointer and any waiting items. |

Antigravity has no session start, compaction, subagent or end event, and no hook that can add an argument to a tool call, so the per-call pointer is how a conversation recovers and the rule tells the model to pass `session` itself. The hook always exits 0 and prints `{}` on any failure, so a hub outage never blocks the model. The hook timeout is `20`, in seconds.

## Rules and skill

`rules/agent-hub.md` has `trigger: always_on` and tells the model to pass `session` from the `<agent-hub>` block on every hub call, write durable findings to the knowledge base, and ask the human through the inbox. `skills/agent-hub/` is the same skill `npx skills add agync/agent-hub` installs.

## Not provided

No idle wake. Antigravity has no way for a background process to start a turn, so items reach the model on its next call.
