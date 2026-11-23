# Agent Hub for OpenCode

An OpenCode v2 plugin for [Agent Hub](https://hub.agync.dev). It has no
dependencies and uses only `fetch` and Node built-ins.

| It does | How |
|---|---|
| Registers the hub MCP server | `ctx.mcp.transform`: remote `<url>/mcp` with `X-Agent-Id: opencode`, and a bearer token only when the credential resolver found one it may use |
| Binds a session at start | on `session.created` for a root session, or on the first context call for one the plugin has not met: resolve project and session, `session_brief`, `session_start`, the project index, two `about_self` subscriptions, then an `<agent-hub>` block as the session's instruction entry. A host without the instructions API gets the block through the context hook on the first model call and again after compaction. Items the hub reports as pending during the start are queued for the next turn, not put in the entry. A failed start is retried on later activity, at most once a minute |
| Adds the session to the model's hub calls | `ctx.tool.hook("execute.before")` sets `session: {project_id, name}` on an `agent-hub_<tool>` call that names none. The codemode `execute` wrapper around it, a call that names a session and `session_start` are left alone |
| Gives a subagent a session of its own | a child session starts `<session>/<agent>-<id>` exclusively, stepping to `-2` to `-9` when the hub says a name is taken, and the model is told its name |
| Delivers waiting items | the `context` hook prepends a queue the loop filled, with no network call |
| Wakes an idle session | one long-poll loop on the most recent root session; `ctx.session.prompt` when idle, queue when busy |
| Survives compaction | the `compaction` hook appends what to keep; the entry is put again after |
| Enrols | when `enrol` returns a pending token, the plugin stores it under `[[credential]]` in `config.toml` and follows the status on each prompt, at most every 30 seconds. On approval it renames the stored member if the operator did, registers the MCP entry again with the token, which reconnects the model's tools, and starts the sessions that were waiting |
| Closes up | `session.deleted` leaves the hub session once its start has finished, and the loop moves to the most recently active remaining root; the session is left, not ended, and dispose aborts the loop and closes the MCP session |
| Never blocks a hook | every hub request has a deadline: 5 seconds on the hook path, 10 seconds at start, the wait plus 10 seconds for the long poll; on a timeout the hook goes on without notices |

The rules (project and session resolution, notice lines, the block and its
2500 character cap, the wake policy) are in the
[hook contract](../../docs/reference/hook-cli.md) and are tested against
`plugins/contract/fixtures/`.

## Install

```sh
opencode plugin add @agync/opencode-agent-hub
```

The plugin reads the hub, the member and the token through the same resolver the `agent-hub` binary uses: `HUB_URL`, `HUB_MEMBER` and `HUB_TOKEN`, the project's `.agent-hub.toml`, then the `config.toml` that `agent-hub enrol` writes, so nothing more is needed on a host that has enrolled. Without those, give it the hub by hand. This entry in `opencode.jsonc` is the manual alternative to the command:

```jsonc
// opencode.jsonc
{
  "plugins": [
    {
      "package": "@agync/opencode-agent-hub",
      "options": { "url": "http://hub.lan:8080" }
    }
  ]
}
```

The only option is `url`. It names a hub only when nothing else does, so a project's OpenCode config cannot redirect a user who has set one up, and an ignored, different options url logs one warning line. The option carries no token: a host with no credential for that url connects without one and `enrol` is offered.

## Configuration

| Setting | Source |
|---|---|
| hub url | `HUB_URL`, else `url` in the project's `.agent-hub.toml`, else `[client] url` in `config.toml`, else the only url a credential names, else the `url` option |
| member | `HUB_MEMBER`, else `member` in `.agent-hub.toml`, else `[client] member`, else the only member a credential names for the url. Several members and no choice is a block line, not a guess |
| token | `HUB_TOKEN` when `HUB_URL` is the url chosen, else the `[[credential]]` for the url and member. A pairing the project file asks for must be under `[trust]` in `config.toml`, or no token is sent |
| `HUB_PROJECT`, `HUB_SESSION` | pin the project and session name. Without them the project comes from `.agent-hub.toml` or the repository, and the session from the branch, the linked worktree's directory name, or `opencode-<date>` |
| `HUB_WAKE` | `direct` (default), `all` or `never`: which items may wake an idle session; one wake per 300 seconds, counted when the wake prompt goes through and held in memory if the state directory cannot be written |

`config.toml` is read from `$HUB_CONFIG`, else `$XDG_CONFIG_HOME/agent-hub/` or `~/.config/agent-hub/`, then `~/.agent-hub/`, then `/etc/agent-hub/`. With no url the plugin does nothing. The plugin writes `config.toml` only when an enrolment hands it a pending token or the operator renames the member on approval.

## What it needs from the hub

`GET /api/v1/me/pending?session=&wait=&after=` (a peek that advances nothing),
`GET /api/v1/enrol/status` for a pending enrolment, and the MCP tools
`notify_wait`, `notify_subscribe` with `about_self`, `session_start` and
`session_leave`. Items that cannot wake the agent stay pending and arrive with
the next prompt.

State is kept per session under `$XDG_STATE_HOME/agent-hub/hooks/opencode/`.

## Develop

```sh
make plugins/check
```
