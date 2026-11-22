# Agent Hub for OpenCode

An OpenCode v2 plugin for [Agent Hub](https://hub.agync.dev). It has no
dependencies and uses only `fetch` and Node built-ins.

| It does | How |
|---|---|
| Registers the hub MCP server | `ctx.mcp.transform`: remote `<url>/mcp`, bearer token, `X-Agent-Id: opencode` |
| Binds a session at start | on `session.created` for a root session: resolve project and session, `session_brief`, `session_start`, the project index, two `about_self` subscriptions, then an `<agent-hub>` block as the session's instruction entry. Items the hub reports as pending during the start are queued for the next turn, not put in the entry. A failed start is retried on later activity, at most once a minute |
| Delivers waiting items | the `context` hook prepends a queue the loop filled, with no network call |
| Wakes an idle session | one long-poll loop on the most recent root session; `ctx.session.prompt` when idle, queue when busy |
| Survives compaction | the `compaction` hook appends what to keep; the entry is put again after |
| Handles subagents | a child session gets the subagent instruction and no bundle |
| Closes up | `session.deleted` ends the hub session once its start has finished, and the loop moves to the most recently active remaining root; dispose aborts the loop and closes the MCP session |
| Never blocks a hook | every hub request has a deadline: 5 seconds on the hook path, 10 seconds at start, the wait plus 10 seconds for the long poll; on a timeout the hook goes on without notices |

The rules (project and session resolution, notice lines, the block and its
2500 character cap, the wake policy) are in the
[hook contract](../../docs/reference/hook-cli.md) and are tested against
`plugins/contract/fixtures/`.

## Install

```jsonc
// opencode.jsonc
{
  "plugins": [
    {
      "package": "@agync/opencode-agent-hub",
      "options": { "url": "http://hub.lan:8080", "tokenFile": "/path/to/home/.config/agent-hub/token" }
    }
  ]
}
```

Options are `url`, `tokenFile` and `agentName` (default `opencode`). `tokenFile` is read only when its real path (symlinks resolved) is inside `$XDG_CONFIG_HOME/agent-hub/` or `~/.config/agent-hub/`; any other path is ignored with one warning line and no token is sent.

## Configuration

First non-empty value wins. The user-level sources come first, so a project-scoped OpenCode config cannot redirect a user who has configured a hub: the plugin options `url` and `tokenFile` apply only when neither `HUB_URL` nor `config.toml` has a url, and an ignored, different options url logs one warning line. A token is only ever sent to the url from its own layer; a layer with a url and no token sends no `Authorization` header.

| Setting | Order |
|---|---|
| hub url and token | one layer, the first with a url: `HUB_URL` with `HUB_TOKEN`, then `url` with `token` in `[client]` of `config.toml`, then options (`url`, `tokenFile`) |
| agent name | option `agentName`, else `opencode` |
| `HUB_PROJECT`, `HUB_SESSION` | pin the project and session name |
| `HUB_WAKE` | `direct` (default), `all` or `never`: which items may wake an idle session; one wake per 300 seconds, counted when the wake prompt goes through and held in memory if the state directory cannot be written |

`config.toml` is read from `$HUB_CONFIG`, else `$XDG_CONFIG_HOME/agent-hub/` or
`~/.config/agent-hub/`, then `~/.agent-hub/`, then `/etc/agent-hub/`. The first file with a `url`
supplies the url and its own token; nothing is merged across files. With no
url the plugin does nothing.

## What it needs from the hub

`GET /api/v1/me/pending?session=&wait=&after=` (a peek that advances nothing)
and the MCP tools `notify_wait`, `notify_subscribe` with `about_self`, and
`session_start`. Items that cannot wake the agent stay pending and arrive with
the next prompt.

State is kept per session under `$XDG_STATE_HOME/agent-hub/hooks/opencode/`.

## Develop

```sh
make plugins/check
```
