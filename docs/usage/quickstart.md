---
type: Guide
title: Quickstart
description: Build, run, and configure a local Agent Hub instance.
tags: [usage, quickstart, container, configuration]
status: draft
---

# Quickstart

This page covers building the hub, running it locally as a binary or through
the container and compose file, and connecting the first agent. The REST API,
the installable PWA, and the MCP surface all ship; the
[agent surface](../architecture/agent-surface.md) and
[human surface](../architecture/human-surface.md) pages describe their
contracts, and [artifacts](artifacts.md) covers authoring.

## Build the binary

The project builds with a recent stable Rust toolchain (1.97 or newer).
`make build` produces a debug binary:

```sh
make build
```

For a release binary, use the locked build:

```sh
cargo build --release --locked
```

## Configure

The binary reads its configuration from `config.toml` (system
`/etc/agent-hub/config.toml` layered with user
`$XDG_CONFIG_HOME/agent-hub/config.toml`, else
`~/.config/agent-hub/config.toml`, fallback `~/.agent-hub/config.toml`, or
named by `HUB_CONFIG`), overridden by environment variables. The inspect
command (`agent-hub config`, `--path`, `--check`) reports the active settings,
search paths, and validation status.

| Key (`[hub]`) | Variable | Default | Purpose |
|---|---|---|---|
| `data_dir` | `HUB_DATA_DIR` | `./data` | Directory for the hub store, session files, artifact blobs, and the tailnet key state |
| `bind` | `HUB_BIND` | `127.0.0.1:8080` | Socket address the HTTP API binds to; with port `0` the system picks a free port and the startup line names the bound address |
| `public_url` | `HUB_PUBLIC_URL` | unset | External origin the hub is reached at, such as `https://hub.example`; overrides the address derived from the request |
| `admin_token` | `HUB_ADMIN_TOKEN` | unset | Admin token for the control surface. Required when the bind is not loopback; a loopback hub without one starts but its PWA control surface refuses every request, so set it before opening the app |
| `active_window_secs` | `HUB_ACTIVE_WINDOW_SECS` | `900` | How long after its last tool call a session still counts its owner as an agent at work; 1 to 2592000 seconds |
| `inbox_action_per_agent` | `HUB_INBOX_ACTION_PER_AGENT` | `100` | Open action items one agent may leave waiting in one project; `0` disables the cap |
| `inbox_action_per_project` | `HUB_INBOX_ACTION_PER_PROJECT` | `1000` | Open action items all agents together may leave waiting in one project; `0` disables the cap |
| `enrol_pending_max` | `HUB_ENROL_PENDING_MAX` | `20` | Most pending enrolment requests the hub holds at once, across every source |
| `enrol_pending_ttl_secs` | `HUB_ENROL_PENDING_TTL_SECS` | `86400` | How long a pending enrolment waits before it is treated as abandoned and cleaned up; 1 to 2592000 seconds |
| `events_per_project` | `HUB_EVENTS_PER_PROJECT` | `1000000` | Most events one project may hold; a write past it is refused until durable work is promoted or the feed is pruned; `0` disables the ceiling |
| `enrol` | `HUB_ENROL` | `on` | Whether the unauthenticated enrolment endpoint is open; `off` closes it |
| `trust_proxy` | `HUB_TRUST_PROXY` | unset | Comma-separated peer IP addresses whose forwarded client header the hub trusts, such as `127.0.0.1` when a reverse proxy runs on the same host; unset trusts none |
| `node_name` | `HUB_NODE_NAME` | the host name | Name the human sees for this node; set it when the host name is a generated container id |
| `tailnet` | `HUB_TAILNET` | unset | A Tailscale auth key; enables the optional embedded tailnet endpoint |
| `tailnet_port` | `HUB_TAILNET_PORT` | `8080` | Port to serve on the tailnet address |
| `tailnet_control_url` | `HUB_TAILNET_CONTROL_URL` | unset | Control server URL for a self-hosted control plane; the public one is the default |

Without `HUB_PUBLIC_URL` the hub reads its own address off each request: the
forwarded scheme and host, then the request host, then the bind. Set it when a
reverse proxy rewrites the host to the upstream address, since the address the
hub then sees is not the one callers use. It takes a bare origin, scheme and
host with an optional port and no path, and a bad value fails startup. The
value is what the artifact frame policy names, what a shared artifact link
carries in its preview tags, and what `GET /SKILL.md` hands a bootstrapping
agent.

The embedded tailnet endpoint is experimental and needs a binary built with
the `tailnet` feature (`cargo build --features tailnet`). Setting `HUB_TAILNET`
is the acknowledgement that it uses early-days software; the hub records that
on startup, so no extra variable is needed. It is addressed by tailnet IP and
carries plain HTTP inside the tunnel, so it needs `HUB_ADMIN_TOKEN` as well.
Leave `HUB_TAILNET` unset for the default deployment: the plain container
behind a reverse proxy.

## Run the binary

Point the process at a data directory and a bind address:

```sh
HUB_DATA_DIR=./data HUB_BIND=127.0.0.1:8080 HUB_ADMIN_TOKEN=change-me \
  ./target/debug/agent-hub
```

On start it prints one line naming the bound address, the data directory, and
whether an admin token is configured, so a port `0` bind is readable without
turning on logs. `RUST_LOG=info` turns on the rest of the tracing output.

The release binary lives at `target/release/agent-hub`. Check the probes from
another shell:

```sh
curl http://127.0.0.1:8080/healthz
curl http://127.0.0.1:8080/readyz
```

The first answers `ok` while the process runs. The second asks the engine for
its schema version and answers with it, or a `503` problem when the store does
not reply, which is the one to point a container healthcheck at.

On start the binary creates the data directory and its `sessions/`, `kb/` and
`artifacts/` children, then opens `hub.db` at the top of the data directory.
Back up the whole data directory as one unit.

## Create a project and connect an agent

Every REST call carries the admin token. Create a project, create an agent,
and issue its token:

```sh
ADMIN="Authorization: Bearer change-me"
curl -sS -X POST http://127.0.0.1:8080/api/v1/projects -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"homelab","display_name":"Homelab"}'
curl -sS -X POST http://127.0.0.1:8080/api/v1/agents -H "$ADMIN" \
  -H 'content-type: application/json' \
  -d '{"id":"my-agent","display_name":"My Agent"}'
curl -sS -X POST http://127.0.0.1:8080/api/v1/agents/my-agent/token -H "$ADMIN"
```

The token is shown once; reissuing replaces it and revokes the previous one.
An agent reaches the hub over MCP with its bearer token:

```
POST http://127.0.0.1:8080/mcp
Authorization: Bearer <agent token>
```

Open `http://127.0.0.1:8080/` for the human surface and paste the admin token
in Settings. The hub also serves `GET /SKILL.md`, a bootstrap guide with its
own address filled in, so an agent that can already reach the hub can fetch
the connection details and the tool list. Grants are managed
under Settings or through the agent routes; see the
[agent surface](../architecture/agent-surface.md).

## Reach the hub from a client machine

The same binary is the client. It reads its settings from the environment
first and then from `config.toml` (`~/.config/agent-hub/config.toml` or
`~/.agent-hub/config.toml`, layered over `/etc/agent-hub/config.toml`);
`HUB_CONFIG` names another file, a missing file is not an error, and a file
holding a token that others can read warns on stderr and still works.

```toml
[client]
url = "http://hub.lan:8080"
token = "..."
agent_id = "my-agent"
project = "homelab"
timeout = 120
```

| Key (`[client]`) | Variable | Default | Purpose |
|---|---|---|---|
| `url` | `HUB_URL` | unset | Base URL of a running hub; with none the proxy serves the local data directory standalone |
| `token` | `HUB_TOKEN` | unset | Bearer token the hub resolves to an agent |
| `agent_id` | `HUB_AGENT_ID` | unset | Advisory agent label; the hub derives the actor from the token |
| `project` | `HUB_PROJECT` | unset | Project the knowledge-base shorthands act on when no flag names one |
| `timeout` | `HUB_TIMEOUT` | `120` | Seconds one call may take, handshake to answer; a number above zero, fractional allowed (for example `2.5`) |

A harness that speaks only stdio MCP runs the proxy, which forwards every
request to the hub over one connection held for the life of the process:

```sh
agent-hub mcp
```

With no `HUB_URL` configured that command instead serves the local data
directory standalone, as the human admin, and says so on stderr. Standalone
mode opens the data directory itself, so pointing it at a directory a hub is
already serving fails at startup with `a hub is already using this directory; set HUB_URL to reach it instead`.

A hook has no MCP client, so it calls one tool at a time. The result is JSON
on stdout and nothing else; logs and errors go to stderr, and the exit code is
0 for success, 1 for a tool error, 2 for usage, 69 when the hub is unreachable,
77 when the token itself was refused, and 78 when nothing names a hub. A denied
project or a missing resource is a tool error (1): the token was accepted, so
only an unrecognised token is 77, the code a hook re-enrols on.

```sh
agent-hub tools
agent-hub call whoami
agent-hub call feed_read '{"project_id":"homelab","limit":20}'
```

The project knowledge base has a shorthand, because a session-start hook
reads it on every machine and should not have to quote a JSON object.
`agent-hub kb get` prints `/fs/index.md` as markdown, ready to pipe into a
context window; `--json` prints the tool's result instead, with the version a
conditional write needs. A path outside `/fs` is taken as relative to it, and
the project comes from `--project` or from `HUB_PROJECT`.

```sh
agent-hub kb get                            # the index page, as markdown
agent-hub kb get runbooks/deploy.md
agent-hub kb put runbooks/deploy.md --file deploy.md
agent-hub kb put runbooks/deploy.md --if-version "$V" - < deploy.md
agent-hub kb list                           # one page path per line
agent-hub kb delete runbooks/deploy.md
```

A failed read prints nothing on stdout and says why in its exit code: 1 means
the hub answered that the page is not there, anything else means the hub did
not answer or refused the token. A hook should branch on that rather than end
the line with `|| true`, which would hide a hub that is down.

Each call is its own connection and holds no session, so `session_start`
in one call is not active in the next. Session-bound work belongs in the
proxy; the CLI is for reads and writes that name their target.

The client is a default-on `client` cargo feature. A build with
`--no-default-features` serves only, which is what the container image needs,
and then `agent-hub mcp` says it was built without the client.

## Run with compose

The compose file builds the image from the `Containerfile`, mounts a named
volume at `/data`, and publishes port 8080. It sets `HUB_DATA_DIR=/data` and
`HUB_BIND=0.0.0.0:8080`, and restarts the container unless it was stopped by
hand.

```sh
docker compose -f deploy/compose.yaml up --build
```

The container runs as a non-root user and writes only to the mounted volume,
so the rest of the filesystem can be read-only. Pass `HUB_ADMIN_TOKEN` in the
environment if the control surface needs it. Stop the stack with
`docker compose -f deploy/compose.yaml down`; the named volume keeps the data.

### What the published port carries

The compose file publishes 8080 on every interface of the host, and the hub
speaks plain HTTP. The admin token travels in an `Authorization` header on
every call from the PWA and every admin request, so on that port it crosses
the network unencrypted, as does everything the hub returns. On a trusted
home network that may be the deployment you want; two supported ways to close
it are:

- Put a TLS-terminating reverse proxy in front, publish the container port to
  loopback (`"127.0.0.1:8080:8080"`) or a private network only, and set
  `HUB_PUBLIC_URL` to the address the proxy serves. This is the default
  deployment in [ADR 0014](../adr/0014-optional-embedded-tailnet.md).
- Reach the hub over a tailnet, either through a Tailscale node on the host or
  through the optional embedded endpoint (`HUB_TAILNET`), which carries plain
  HTTP inside the tailnet's own tunnel.

A reverse proxy may also mount the hub on a path instead of a dedicated host
or port, such as `https://host/hub/`, as long as it strips that path before
forwarding: the hub always serves from its own root and never needs to know a
prefix exists. The PWA itself normalises a request that reaches it without a
trailing slash (`/hub`) to one that has it (`/hub/`) before it loads anything
else, since every asset it loads resolves relative to that address. This
needs no configuration; there is no `HUB_BASE_PATH` or equivalent, and a proxy
that does not strip the prefix is not supported.

## See also

- [Human interface](../design/human-interface.md) - the intended human surface
- [Architecture overview](../architecture/index.md) - components and layout
