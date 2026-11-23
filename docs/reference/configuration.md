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

- on Windows, `%APPDATA%\agent-hub\config.toml`
- `$XDG_CONFIG_HOME/agent-hub/config.toml`, else
  `~/.config/agent-hub/config.toml` (also on macOS)
- `~/.agent-hub/config.toml`

On Windows, `USERPROFILE` stands in where `HOME` is not set.

`HUB_CONFIG` names a different file and replaces both layers. Environment
variables override the files. A service made by `agent-hub service install` pins
`HUB_CONFIG` to the user file the installer wrote, so it does not read
`/etc/agent-hub/config.toml`. A missing file is not an error, and a file that
others can read warns on stderr when it holds a token, and still works.

The file carries two tables:

```toml
[hub]
bind = "127.0.0.1:8080"
admin_token = "..."

[client]
url = "http://hub.lan:8080"
member = "laptop"
project = "homelab"
timeout = 120

[[credential]]
url = "http://hub.lan:8080"
member = "laptop"
token = "..."
```

A third table, `[trust]`, records the project pairings the user allowed (see
below). The credential tables are written by `agent-hub enrol`, the stdio proxy
and the OpenCode plugin, not by hand.

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
| `data_dir` | `HUB_DATA_DIR` | `./data` for `serve`; the platform data directory for `start` and `service install` | Directory for the hub store, session files, artifact blobs, and the tailnet key state. The platform directory is `~/.local/share/agent-hub` on Linux, `~/Library/Application Support/agent-hub` on macOS and `%LOCALAPPDATA%\agent-hub\data` on Windows; `service install` writes it into `config.toml` as an absolute path |
| `log_file` | `HUB_LOG_FILE` | unset (stderr) | File `serve` writes its log to instead of stderr, rotated at 10 MiB with three archives kept. `start` and the macOS and Windows services set it to the platform log path; a Linux service logs to the journal. A path that cannot be opened stops the start, and a name of `hub.lock`, `hub.pid.json` or ending in `.start` is refused |
| `admin_token` | `HUB_ADMIN_TOKEN` | unset | Admin token for the control surface. Required when the bind is not loopback; a loopback hub without one starts but its app refuses every request, so set it before opening the app. `service install` generates one (64 hex characters) when none is set and saves it here |
| `node_name` | `HUB_NODE_NAME` | the host name | Name the human sees for this node; set it when the host name is a generated container id |
| `public_url` | `HUB_PUBLIC_URL` | unset | External origin the hub is reached at, such as `https://hub.example`; overrides the address derived from the request |
| `active_window_secs` | `HUB_ACTIVE_WINDOW_SECS` | `900` | How long after a participant's last call a session still counts as live; 1 to 2592000 seconds |
| `inbox_action_per_agent` | `HUB_INBOX_ACTION_PER_AGENT` | `100` | Open action items one participant may leave waiting in one project; `0` disables the cap |
| `inbox_action_per_project` | `HUB_INBOX_ACTION_PER_PROJECT` | `1000` | Open action items all participants together may leave waiting in one project; `0` disables the cap |
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
| `url` | `HUB_URL` | unset | Base URL of the hub to use. With none, a client takes the only hub the credential file names, and `agent-hub mcp` exits rather than serve a local store |
| `member` | `HUB_MEMBER` | unset | The member to use for the hub, when the credential file holds more than one |
| `token` | `HUB_TOKEN` | unset | A bearer token. It counts only together with its `HUB_URL`, and only when no other member is selected |
| `project` | `HUB_PROJECT` | unset | Project the knowledge-base shorthands act on when no flag names one |
| `timeout` | `HUB_TIMEOUT` | `120` | Seconds one call may take, handshake to answer; a number above zero, fractional allowed (for example `2.5`) |

`agent_id` is read and ignored, with the warning `agent_id no longer names the
member; the agent comes from the client`. The agent is the program, named by its
MCP handshake; `HUB_AGENT_ID` still sets the name the proxy sends in
`X-Agent-Id`, as an override. A legacy `[client] token` with its `url` reads as
one credential whose member is unknown.

`service install` writes `url` as `http://<bind host>:<port>` when it is unset,
so the stdio proxy (`mcp`) and the other client commands reach the local hub
rather than needing a flag. `health` and `backup --url` take their address from
`--url`, not from this key. It never writes the admin token as `token`: members
do not use it.

These settings are read by the binary's client commands: the stdio proxy
(`mcp`), the one-shot `call`, the `kb` shorthands, `enrol` and `token`.
[CLI](/reference/cli.md) lists every subcommand that reads them.

### Credentials

A credential is a hub URL, a member and the member's token. The file keeps one
per pair:

```toml
[[credential]]
url = "https://hub.example.net"
member = "laptop"
token = "..."
```

When several credentials share one hub URL, mark the one to use with
`default = true`:

```toml
[[credential]]
url = "https://hub.example.net"
member = "laptop"
token = "..."
default = true
```

Every client entry point, which is the hooks, `agent-hub mcp`,
`agent-hub token --headers` and the OpenCode plugin, resolves the credential with
one rule, so a hook and the MCP connection never act as different members.
URLs compare after lower-casing the scheme and host and dropping a default port
and a trailing slash.

| Step | Rule, first match wins |
|---|---|
| url | a pinned URL (`--url`, or the Claude plugin's hub URL), then `HUB_URL`, then the project file's `url`, then `[client] url`, then the only distinct URL in the credential file |
| member | `HUB_MEMBER`, then the project file's `member` (when its `url` is absent or equal), then `[client] member`, then the only named member for the URL, or the one marked `default = true`; several members with none selected is `several_members` |
| token | `HUB_TOKEN`, when `HUB_URL` equals the URL and `HUB_MEMBER` is unset or equal; then the credential for the pair; then a legacy credential with no member, only when no member was selected |
| trust | a project file that supplied the URL or member selects a pairing the user has not trusted: no token is used, and the state is `needs_trust` |

The Claude Code header helper (`agent-hub token --headers`) takes the only
credential for the hub URL, else the one marked `default = true`, else sends no
`Authorization`. A hook prints a mismatch line in its block when the project's
member differs from the one the helper picked.

The result carries a state: `ok`, `tokenless`, `no_hub`, `several_members` or
`needs_trust`. A client with no token still connects and sees only the enrol
surface.

### The project file

`.agent-hub.toml` at the git top level pins a repository to a hub:

```toml
project = "homelab"
url = "https://hub.example.net"   # optional
member = "laptop-secret"          # optional; never a token
```

The file is shared, so a `token` key is an error. A legacy `.agent-hub` file is
ignored, with a context line saying which file the release reads. A pairing the
file selects is sent a token only after the user trusts it, on this machine:

```toml
[trust]
pairings = [ { path = "/path/to/secret-repo", url = "https://hub.example.net", member = "laptop-secret" } ]
```

`agent-hub trust [--path DIR]` records the pairing that the project file at `DIR`
selects.

### How the file is written

Several writers share `config.toml`: `agent-hub enrol`, the stdio proxy, and the
OpenCode plugin, which is JavaScript. They cannot share `flock`, so each uses one
lock-directory protocol, and the plugin launcher's install uses the same one.

| Step | Rule |
|---|---|
| Acquire | `mkdir config.toml.lock`, which is atomic on every platform, then write `owner` inside it as `<pid> <host> <epoch_ms>` through a temporary file and a rename |
| Busy | `mkdir` fails because the directory exists: read `owner` and retry every 50 ms for up to 5 seconds, then fail with `config.toml is locked by pid <p> on <host>`. The launcher does not wait: it reports that it is installing and exits |
| Stale | a lock older than the age limit (30 seconds for the config, 10 minutes for the launcher; age from `owner`, or the directory's modification time when `owner` is missing or unreadable) **and** one of: `owner` missing, a different host, or a pid that is not alive |
| Break | rename `config.toml.lock` to `config.toml.lock.stale.<pid>.<epoch_ms>`, which exactly one breaker wins, remove it, and try to acquire once more |
| Release | only the holder whose line is in `owner`: remove `owner`, then remove the directory |

A writer takes the lock, re-reads the file, changes its entry, writes
`config.toml.tmp.<pid>` with mode `0600`, flushes it, renames it over the file,
and releases the lock. A lock that is held by a live process is never broken,
however old.

## See also

- [CLI](/reference/cli.md) - the commands that read these settings
- [Network and access](/operate/network-and-access.md) - `bind`, `public_url`, `trust_proxy`
- [Deploy as a service](/operate/deploy-as-a-service.md) - the environment file a service reads
