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

The binary reads its configuration from the environment.

| Variable | Default | Purpose |
|---|---|---|
| `HUB_DATA_DIR` | `./data` | Directory for the hub store, session files, artifact blobs, and the tailnet key state |
| `HUB_BIND` | `127.0.0.1:8080` | Socket address the HTTP API binds to |
| `HUB_PUBLIC_URL` | unset | External origin the hub is reached at, such as `https://hub.example`; overrides the address derived from the request |
| `HUB_ADMIN_TOKEN` | unset | Admin token for the control surface; required when the bind is not loopback |
| `HUB_AGENT_ID` | `local` | Actor label recorded for the stdio admin process, effective only with `mcp` |
| `HUB_TRUST_DEFAULT` | `trusted` | Posture applied to a newly created agent, `trusted` or `untrusted` |
| `HUB_INBOX_ACTION_PER_AGENT` | `100` | Open action items one agent may leave waiting in one project; `0` disables the cap |
| `HUB_INBOX_ACTION_PER_PROJECT` | `1000` | Open action items all agents together may leave waiting in one project; `0` disables the cap |
| `HUB_TAILNET` | unset | A Tailscale auth key; enables the optional embedded tailnet endpoint |
| `HUB_TAILNET_PORT` | `8080` | Port to serve on the tailnet address |
| `HUB_TAILNET_CONTROL_URL` | unset | Control server URL for a self-hosted control plane; the public one is the default |

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
An agent reaches the hub over MCP, either stdio for a local process or
streamable HTTP with its bearer token:

```sh
HUB_DATA_DIR=./data HUB_AGENT_ID=my-agent ./target/debug/agent-hub mcp
```

```
POST http://127.0.0.1:8080/mcp
Authorization: Bearer <agent token>
```

Open `http://127.0.0.1:8080/` for the human surface and paste the admin token
in Settings. The hub also serves `GET /SKILL.md`, a bootstrap guide with its
own address filled in, so an agent that can already reach the hub can fetch
the connection details and the tool list. Trust levels and grants are managed
under Settings or through the agent routes; see the
[agent surface](../architecture/agent-surface.md).

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

## See also

- [Human interface](../design/human-interface.md) - the intended human surface
- [Architecture overview](../architecture/index.md) - components and layout
