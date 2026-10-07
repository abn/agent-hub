---
type: Guide
title: Deploy the hub as a service
description: Run the container under systemd with Podman Quadlet or Docker, start it on boot, or bring the stack up with compose.
tags: [usage, deploy, container, systemd]
status: draft
---

# Deploy the hub as a service

The published image is `ghcr.io/abn/agent-hub`. This page runs it as a
long-lived service: under Podman Quadlet or Docker with an auto-start policy, or
through the compose file. It does not repeat how to build or configure the hub;
see the [quickstart](quickstart.md) for the binary and the full settings table.

Two properties of the image shape every option below. It writes only to its data
directory, so it runs with a read-only root filesystem and one mounted volume.
And it drains for ten seconds on `SIGTERM` before it checkpoints the store and
exits, so the service stop timeout must be longer than that drain.

## Prerequisites

| Thing | Why |
|---|---|
| Podman with Quadlet, or Docker | runs the published image |
| an admin token | the control surface refuses every request without one |
| a data volume | `hub.db`, the session files, and the artifact blobs live there |

## Set the configuration once

Both options below read the same environment file, `~/.config/agent-hub/hub.env`
at mode `0600`:

```
HUB_ADMIN_TOKEN=<a long random token>
HUB_PUBLIC_URL=https://hub.example
```

Set `HUB_PUBLIC_URL` whenever the hub sits behind a reverse proxy that rewrites
the host: the served bootstrap, the artifact frame policy and the share links
all name that address. Generate the token with `openssl rand -hex 32`.

To be nudged when something waits while the app is closed, add a notify target
you run, such as a self-hosted ntfy topic on the LAN:

```
HUB_NOTIFY_URL=https://ntfy.lan/agent-hub
HUB_NOTIFY_TOKEN=<an ntfy access token>
```

The hub POSTs one fixed sentence there and nothing about the item itself.
[Operations](operations.md#notify-a-closed-app) covers the coalescing and the
failure behaviour.

## Compose

The repository ships `deploy/compose.yaml`. It builds the Containerfile, mounts
a named volume at `/data`, keeps the rest of the filesystem read-only, sets a
stop grace period above the hub's drain, and refuses to render without
`HUB_ADMIN_TOKEN`:

```sh
HUB_ADMIN_TOKEN="$(openssl rand -hex 32)" \
  docker compose -f deploy/compose.yaml up --detach
```

Podman reads the same file through `podman compose`. The service carries
`restart: unless-stopped`, so the container returns after a host restart unless
it was stopped by hand.

## Podman Quadlet

Quadlet is the supported way to run a container under systemd, per user or
system-wide, and it supersedes `podman generate systemd`. Save the unit as
`~/.config/containers/systemd/agent-hub.container`. The repository ships this
file as `deploy/agent-hub.container`, so copy it rather than retyping it:

```ini
[Unit]
Description=Agent Hub
After=network-online.target
Wants=network-online.target

[Container]
Image=ghcr.io/abn/agent-hub:1.0.0
ContainerName=agent-hub
PublishPort=127.0.0.1:8080:8080
Volume=agent-hub-data:/data
EnvironmentFile=%h/.config/agent-hub/hub.env
Environment=HUB_DATA_DIR=/data
Environment=HUB_BIND=0.0.0.0:8080
ReadOnly=true
Tmpfs=/tmp
HealthCmd=/usr/local/bin/agent-hub health --url http://127.0.0.1:8080
HealthInterval=30s
HealthStartPeriod=10s
HealthRetries=3

[Service]
Restart=always
TimeoutStopSec=20

[Install]
WantedBy=default.target
```

Load and start it:

```sh
systemctl --user daemon-reload
systemctl --user enable --now agent-hub
systemctl --user status agent-hub
```

`enable` writes the `[Install]` target, so the service starts whenever the
user's systemd starts. A user manager stops at logout unless lingering is on, so
on a headless node turn it on once and the service survives with no login:

```sh
loginctl enable-linger "$USER"
```

For a system-wide service, put the unit in `/etc/containers/systemd/`, use
`WantedBy=multi-user.target`, and enable it with
`sudo systemctl enable --now agent-hub`. Rootless Podman is the better default
when it works on the host.

The healthcheck is the image's own: the binary's `health` subcommand GETs
`/readyz` and exits non-zero the moment the store stops answering. Quadlet
forwards it to Podman, so `podman healthcheck run agent-hub` runs it by hand and
`systemctl --user status` shows the result.

## Docker

Docker has no Quadlet. Run the container with an auto-start policy so the daemon
brings it back after a restart:

```sh
docker run --detach --name agent-hub \
  --restart unless-stopped \
  --publish 127.0.0.1:8080:8080 \
  --env-file ~/.config/agent-hub/hub.env \
  --volume agent-hub-data:/data \
  --stop-timeout 20 \
  ghcr.io/abn/agent-hub:1.0.0
```

`unless-stopped` restarts the container after a daemon or host restart, and
`--stop-timeout 20` matches the drain so the store checkpoints on a clean stop.
Compose sets both, which is the shorter path when a compose file is acceptable.
For a unit-managed lifecycle without Podman, run the compose file from a small
systemd unit whose `ExecStart` is `docker compose up`.

## Reaching the hub

The port above is published to loopback only. Put a TLS-terminating reverse
proxy in front and set `HUB_PUBLIC_URL` to the address callers use. An upgrade
is a pull of the new image tag and a restart of the unit; the store migrates
forward on startup. [Operations](operations.md) covers backup, verification,
upgrade and rollback.

## See also

- [Quickstart](quickstart.md) - build the binary, the settings table, compose
- [Operations](operations.md) - backup, restore, upgrade and roll back
- [Components](../architecture/components.md) - the deployment shape and the
  data volume
