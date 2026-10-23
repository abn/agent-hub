---
type: Reference
title: Configuration
description: Every hub and client setting, its variable, its default, and its file
tags: [operator, config]
status: stable
---

# Configuration

Every setting the hub and the client read, its environment variable, its
default, and its purpose. This is the one place the settings live; the install
that sets the first of them is in [Install and run the hub](/start/install.md).

## Where the settings come from

The binary reads `config.toml`: a system file at `/etc/agent-hub/config.toml`
(or the path in `AGENT_HUB_SYSTEM_CONFIG`) layered under a user file. The user
file is the first of these that exists:

- `$XDG_CONFIG_HOME/agent-hub/config.toml`, else
  `~/.config/agent-hub/config.toml`
- `~/.agent-hub/config.toml`

`HUB_CONFIG` names a different file and replaces both layers. Environment
variables override the files. A missing file is not an error, and a file that
others can read warns on stderr when it holds a token, and still works.

The file carries two tables:

```toml
[hub]
bind = "127.0.0.1:8080"
admin_token = "..."

[client]
url = "http://hub.lan:8080"
token = "..."
agent_id = "my-agent"
project = "homelab"
timeout = 120
```

## Inspect it

```sh
agent-hub config            # every setting, its value, and where it came from
agent-hub config --path     # the files that would be read, found or not
agent-hub config --check    # parse and validate, exit non-zero on a problem
```

`--check` treats an unknown key under `[hub]` or `[client]` as an error: it
exits non-zero and names the key, rather than ignoring a setting you thought
took effect.

## `[hub]`

| Key | Variable | Default | Purpose |
|---|---|---|---|
| `bind` | `HUB_BIND` | `127.0.0.1:8080` | Socket address the HTTP API binds to; with port `0` the system picks a free port and the startup line names the bound address |
| `data_dir` | `HUB_DATA_DIR` | `./data` | Directory for the hub store, session files, artifact blobs, and the tailnet key state |
| `admin_token` | `HUB_ADMIN_TOKEN` | unset | Admin token for the control surface. Required when the bind is not loopback; a loopback hub without one starts but its app refuses every request, so set it before opening the app |
| `node_name` | `HUB_NODE_NAME` | the host name | Name the human sees for this node; set it when the host name is a generated container id |
| `public_url` | `HUB_PUBLIC_URL` | unset | External origin the hub is reached at, such as `https://hub.example`; overrides the address derived from the request |
| `active_window_secs` | `HUB_ACTIVE_WINDOW_SECS` | `900` | How long after its last tool call a session still counts its owner as an agent at work; 1 to 2592000 seconds |
| `inbox_action_per_agent` | `HUB_INBOX_ACTION_PER_AGENT` | `100` | Open action items one agent may leave waiting in one project; `0` disables the cap |
| `inbox_action_per_project` | `HUB_INBOX_ACTION_PER_PROJECT` | `1000` | Open action items all agents together may leave waiting in one project; `0` disables the cap |
| `events_per_project` | `HUB_EVENTS_PER_PROJECT` | `1000000` | Most events one project may hold; a write past it is refused until events are removed, which pruning a session does; `0` disables the ceiling |
| `enrol` | `HUB_ENROL` | `on` | Whether the unauthenticated enrolment endpoint is open; `off` closes it |
| `enrol_pending_max` | `HUB_ENROL_PENDING_MAX` | `20` | Most pending enrolment requests the hub holds at once, across every source |
| `enrol_pending_ttl_secs` | `HUB_ENROL_PENDING_TTL_SECS` | `86400` | How long a pending enrolment waits before it is treated as abandoned and cleaned up; 1 to 2592000 seconds |
| `integrity_sample_secs` | `HUB_INTEGRITY_SAMPLE_SECS` | `0` (disabled) | Seconds between background store integrity samples; `0` disables it, up to 604800 |
| `trust_proxy` | `HUB_TRUST_PROXY` | unset | Comma-separated peer IP addresses whose forwarded client header the hub trusts, such as `127.0.0.1` when a reverse proxy runs on the same host; unset trusts none |
| `tailnet` | `HUB_TAILNET` | unset | A Tailscale auth key; enables the optional embedded tailnet endpoint |
| `tailnet_port` | `HUB_TAILNET_PORT` | `8080` | Port to serve on the tailnet address |
| `tailnet_control_url` | `HUB_TAILNET_CONTROL_URL` | unset | Control server URL for a self-hosted control plane; the public one is the default |
| `notify_url` | `HUB_NOTIFY_URL` | unset | An `http` or `https` URL the hub POSTs a fixed, contentless sentence to when something starts waiting on the human; unset sends nothing. It may not carry credentials, and any path or query is hidden when the configuration is printed |
| `notify_token` | `HUB_NOTIFY_TOKEN` | unset | Sent to the notify target as `Authorization: Bearer`; masked wherever the configuration is printed |
| `notify_interval_secs` | `HUB_NOTIFY_INTERVAL_SECS` | `60` | Least time between two notify sends; items inside it become one trailing send; 1 to 86400 seconds |
| `backup_dir` | `HUB_BACKUP_DIR` | unset | Existing directory, outside the data directory, where the serving hub writes an online backup; unset turns online backup off |

### `public_url`

Without `HUB_PUBLIC_URL` the hub reads its own address off each request: the
forwarded scheme and host, then the request host, then the bind. Set it when a
reverse proxy rewrites the host to the upstream address, since the address the
hub then sees is not the one callers use. It takes a bare origin, scheme and
host with an optional port and no path, and a bad value fails startup. The value
is what the artifact frame policy names, what a shared artifact link carries in
its preview tags, and what `GET /bootstrap/SKILL.md` hands a bootstrapping agent.
[Network and access](/operate/network-and-access.md) covers the proxy.

### The embedded tailnet

The embedded tailnet endpoint is experimental and is compiled into the default
build. Setting `HUB_TAILNET` is the acknowledgement that it uses early-days
software; the hub records that on startup, so no extra variable is needed. It is
addressed by tailnet IP and carries plain HTTP inside the tunnel, so it needs
`HUB_ADMIN_TOKEN` as well. The container image builds with
`--no-default-features`, so it does not carry the embedded endpoint. Leave
`HUB_TAILNET` unset for the default deployment: the plain container behind a
reverse proxy.

### The notify target

Set `HUB_NOTIFY_URL` to have the hub POST a fixed, contentless sentence to a
target you run when something starts waiting. It may not carry credentials (set
`HUB_NOTIFY_TOKEN` instead), and the token is sent only in the `Authorization`
header and never logged. [Notifications](/operate/notifications.md) covers the
coalescing and the failure behaviour.

### Online backup

`HUB_BACKUP_DIR` names an existing directory, outside the data directory, where
the serving hub writes an online backup. The hub refuses a value inside the data
directory, and checks again with symlinks resolved before each backup.
[Back up and restore](/operate/backup-and-restore.md) has the procedure.

## `[client]`

| Key | Variable | Default | Purpose |
|---|---|---|---|
| `url` | `HUB_URL` | unset | Base URL of a running hub; with none the proxy serves the local data directory standalone |
| `token` | `HUB_TOKEN` | unset | Bearer token the hub resolves to an agent |
| `agent_id` | `HUB_AGENT_ID` | unset | The name the client declares in `X-Agent-Id`. Beside a token it is a sub-identity of the token's agent, for attribution only; on a loopback hub with no token it is the agent's identity |
| `project` | `HUB_PROJECT` | unset | Project the knowledge-base shorthands act on when no flag names one |
| `timeout` | `HUB_TIMEOUT` | `120` | Seconds one call may take, handshake to answer; a number above zero, fractional allowed (for example `2.5`) |

These settings are read by the binary's client commands: the stdio proxy, the
one-shot `call`, the `kb` shorthands, and `enrol`. [CLI](/reference/cli.md)
lists every subcommand that reads them.

## See also

- [CLI](/reference/cli.md) - the commands that read these settings
- [Network and access](/operate/network-and-access.md) - `bind`, `public_url`, `trust_proxy`
- [Deploy as a service](/operate/deploy-as-a-service.md) - the environment file a service reads
