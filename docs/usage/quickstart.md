---
type: Guide
title: Quickstart
description: Build, run, and configure a local Agent Hub instance.
tags: [usage, quickstart, container, configuration]
status: draft
---

# Quickstart

This page covers building the hub and running it locally, either as a binary
or through the container and compose file. The hub is not feature complete
yet: the binary starts, opens its store, and answers health and readiness
probes, while the agent and human surfaces are still being built. Run it to
exercise the packaging and the storage engine, not to depend on it.

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
| `HUB_DATA_DIR` | `./data` | Directory for the hub store, session files, and artifact blobs |
| `HUB_BIND` | `127.0.0.1:8080` | Socket address the HTTP API binds to |
| `HUB_ADMIN_TOKEN` | unset | Optional admin token for the control surface |

`HUB_TRUST_DEFAULT` also sets the posture applied to a newly created agent.
Leave it unset for the trusted default.

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

On start the binary creates the data directory and its `sessions/` and
`artifacts/` children, then opens `hub.db` at the top of the data directory.
Back up the whole data directory as one unit.

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

## See also

- [Human interface](../design/human-interface.md) - the intended human surface
- [Architecture overview](../architecture/index.md) - components and layout
