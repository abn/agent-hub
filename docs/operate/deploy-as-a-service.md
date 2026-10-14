---
type: Guide
title: Deploy as a service
description: Run the hub under Podman Quadlet or Docker, and start it on boot
tags: [operator, operations, install]
status: stable
---

# Deploy as a service

By the end you will have the hub running as a long-lived service that comes back
after a restart and drains cleanly on stop. This page starts from a built image;
building one, the binary, and every setting are in
[Install and run the hub](/start/install.md).

## Before you start

| Thing | Why |
|---|---|
| Podman with Quadlet, or Docker | runs the published image `ghcr.io/abn/agent-hub` |
| An admin token | the control surface refuses every request without one |
| A data volume | `hub.db`, the session brains, and the artifact blobs live there |

Two properties of the image shape every option below. It writes only to its data
directory, so it runs with a read-only root filesystem and one mounted volume.
And it drains for ten seconds on `SIGTERM` before it checkpoints the store and
exits, so a service's stop timeout must be longer than that drain.

## Set the configuration once

Every option below reads the same environment file,
`~/.config/agent-hub/hub.env` at mode `0600`:

```
HUB_ADMIN_TOKEN=<a long random token>
HUB_PUBLIC_URL=https://hub.example
```

Generate the token with `openssl rand -hex 32`. Set `HUB_PUBLIC_URL` whenever
the hub sits behind a reverse proxy that rewrites the host: the served
bootstrap, the artifact frame policy, and the share links all name that address.
[Network and access](/operate/network-and-access.md) covers the proxy.

## Compose

The repository ships `deploy/compose.yaml`. It builds the `Containerfile`,
mounts a named volume at `/data`, keeps the rest of the filesystem read-only,
sets a stop grace period above the hub's drain, and refuses to render without
`HUB_ADMIN_TOKEN`:

```sh
HUB_ADMIN_TOKEN="$(openssl rand -hex 32)" \
  docker compose -f deploy/compose.yaml up --detach
```

The file carries `restart: unless-stopped`, so the container returns after a
host restart unless it was stopped by hand. Podman reads the same file through
`podman compose`.

## Podman Quadlet

Quadlet is the supported way to run a container under systemd, per user or
system-wide, and it supersedes `podman generate systemd`. Save the unit as
`~/.config/containers/systemd/agent-hub.container`. The repository ships it as
`deploy/agent-hub.container`, so copy it rather than retype it:

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
user's systemd starts. A user manager stops at logout unless lingering is on,
so on a headless node turn it on once and the service survives with no login:

```sh
loginctl enable-linger "$USER"
```

For a system-wide service, put the unit in `/etc/containers/systemd/`, use
`WantedBy=multi-user.target`, and enable it with
`sudo systemctl enable --now agent-hub`. Rootless Podman is the better default
when it works on the host.

The healthcheck is the image's own: the binary's `health` subcommand GETs
`/readyz` and exits non-zero the moment the store stops answering. Quadlet
forwards it to Podman, so `podman healthcheck run agent-hub` runs it by hand
and `systemctl --user status` shows the result.

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

## You should now see

The unit enabled or the container running, and a service that comes back after
a restart. Confirm the hub is ready from the node:

```sh
agent-hub health --url http://127.0.0.1:8080
```

It prints `ready` and exits 0 while the store answers.

## See also

- [Network and access](/operate/network-and-access.md) - the bind, TLS, and a tailnet
- [Back up and restore](/operate/backup-and-restore.md) - take a consistent backup
- [Upgrade and roll back](/operate/upgrade-and-rollback.md) - move to a new image tag safely
- [Configuration](/reference/configuration.md) - every setting
