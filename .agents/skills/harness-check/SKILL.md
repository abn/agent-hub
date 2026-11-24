---
name: harness-check
description: Probe what Claude Code, OpenCode and Antigravity actually send to an MCP server and to their hooks, and which config syntax each one expands. Use before building or changing a plugin, hook or MCP entry that depends on client behaviour (clientInfo, hook payloads, PreToolUse updatedInput, headersHelper, env expansion, list_changed, plugin validation), or after a client upgrade.
---

# Check the harnesses

`scripts/harness_check.py` stands up a logging MCP endpoint and logging hooks,
drives each client non-interactively with a short prompt, and prints the raw
evidence: the `initialize` payload, the headers each request carried, the hook
stdin, and whether a mutated tool input reached the server. It records; the
verdict (holds, fails, not testable) is yours.

The endpoint stands in for the hub. The checks are about the clients, so no hub
binary is built or started.

## Run

```sh
python3 .agents/skills/harness-check/scripts/harness_check.py run claude
python3 .agents/skills/harness-check/scripts/harness_check.py run all
```

| Run | Covers |
|---|---|
| `claude` | clientInfo, hook payloads, PreToolUse `updatedInput` with and without `permissionDecision`, subagent `agent_id`, static `${VAR}` header expansion, `tools/list_changed` |
| `claude-plugin` | plugin `.mcp.json` `headersHelper`: expansion, when it re-runs (connect, retry, 401 mid-session), `CLAUDE_PLUGIN_DATA` |
| `claude-validate` | `claude plugin validate` on a draft copy of the checkout's plugin |
| `opencode` | clientInfo, `{env:VAR}`, `tool.execute.before` arg mutation, child session and `session.deleted`, `ctx.mcp.transform` re-run, `list_changed` |
| `agy` | clientInfo, hook payloads, header, url and env syntax in `mcp_config.json` |
| `agy-validate` | `agy plugin validate` on a draft copy of the checkout's plugin |

## Release runs

`identity-*` runs drive the real clients against a scratch hub started from this
checkout's build (`cargo build` first; the data directory is under
`target/tmp/harness-check/`). They print the hub's own view (sessions and
participants) and a verdict per row. `HC_DRY=1` prints the client commands
without calling a model. They are not part of `all`.

The exit code is the verdict: `0` when every verdict holds (and for the probe runs,
which only record), `1` when any `VERDICT ... FAILS` or `REGRESSED` row printed, so a
scripted caller sees a failure. A dry run always exits `0`.

| Run | Covers |
|---|---|
| `identity-setup` | no-binary setup: Claude `${HUB_TOKEN}`, OpenCode `{env:HUB_TOKEN}`, Antigravity stdio with the parent env; `whoami` names the member and agent, and a write that names a session creates it |
| `identity-children` | a Claude subagent and an OpenCode child session each write in `<parent>/<task>`; the parent's brain is untouched (needs the plugins at head) |
| `identity-swap` | a pending credential approved under another name: what Claude, OpenCode and the Antigravity proxy do next |
| `identity-upgrade` | `migrate --check`, serve, PWA screenshots, kill during the upgrade and restart, then backup, restore and check; `HC_UPGRADE_COPY` names a copy of a data directory |
| `identity-regression` | re-runs `claude`, `claude-plugin`, `opencode` and `agy`, validates the checkout's plugins, and reads the matrix rows off the logs: a row that no longer holds prints `REGRESSED` |

`scripts/identity_gate.py` is the other half: the CI checks, listed by each
runner's own listing and run (`make gate/identity`). It is not a client probe.

Every run writes under `target/tmp/harness-check/<run>/` (`log/requests.jsonl`,
`log/hooks.jsonl`, `log/oc.jsonl`). Each model call is one short prompt on the
cheapest model the client offers.

## Rules

- Never edit a client's real config. Claude gets per-run flags
  (`--settings`, `--mcp-config`, `--strict-mcp-config`, `--plugin-dir`,
  `--setting-sources project`). OpenCode gets its own `XDG_*` directories, a
  project `opencode.jsonc` in the run directory, and `--standalone`. Without
  `--standalone` `opencode run` talks to the user's background service.
  Antigravity gets `HOME` pointed at the run directory; its sign-in survives.
- `--strict-mcp-config` drops plugin MCP servers. The plugin run omits it and
  relies on `--setting-sources project` to keep installed plugins out.
- OpenCode takes the project from `$PWD`, not the process cwd. The script sets
  both.
- The scratch endpoint binds 127.0.0.1 on a free port. Never point a run at the
  live hub or its data directory.
- Dummy token only (`dummy-token-123`). Redact paths and host names before
  quoting evidence anywhere.
- A model that wanders is a flaky probe, not a finding. Re-run before recording
  a fail that depends on the model calling a tool.
