---
type: Guide
title: Choose your setup
description: Pick how to run the hub and how machines reach it, then follow one install
tags: [user, install, config]
status: stable
---

# Choose your setup

Two decisions set your whole setup: how the hub runs, and how your agents and
your browser reach it. Make them here, then follow exactly one install page.

## How the hub runs

| You want | Use | Read |
|---|---|---|
| The simplest durable setup, with auto-start on boot | The container under Podman Quadlet or Docker | [Deploy as a service](/operate/deploy-as-a-service.md) |
| To bring a stack up with one file | The compose file | [Install and run the hub](/start/install.md) |
| A single file with no container runtime | A release binary | [Install and run the hub](/start/install.md) |
| To build and change the code | Source | [Install and run the hub](/start/install.md) and [Contributing](/contribute/index.md) |

The container is the default and the one to choose unless you have a reason not
to. It runs as a non-root user, writes only to its data volume, needs no shell,
and answers a healthcheck with the binary's own `health` subcommand.

## How machines reach it

The hub must be reachable from every machine an agent runs on, and from the
browser you watch it from. Pick one:

| Situation | Bind the hub to | Notes |
|---|---|---|
| Everything on one machine | `127.0.0.1:8080` (the default) | Simplest; nothing leaves the machine |
| A trusted home LAN | The host's LAN address | Plain HTTP; set an admin token |
| Dynamic address, or a network you do not fully trust | Loopback plus a TLS reverse proxy | Set `HUB_PUBLIC_URL` to the address callers use |
| Machines across the internet, no public port | A tailnet | A Tailscale node on the host, or the experimental embedded endpoint |

> [!WARNING]
> A LAN or public bind carries plain HTTP. The admin token travels in an
> `Authorization` header on every control request, so on that port it crosses
> the network unencrypted. Terminate TLS in front, or reach the hub over a
> tailnet. [Network and access](/operate/network-and-access.md) covers both.

## Two things every setup needs

**An admin token.** It is the control-surface password. A hub bound to anything
other than loopback refuses to start without one, and a loopback hub without one
starts but its app refuses every request. Set a long random value:

```sh
HUB_ADMIN_TOKEN="$(openssl rand -hex 32)"
```

**A place for the data.** The hub store, the per-session brains, the project
knowledge bases, and the artifact blobs all live in one directory: `./data` by
default, or `/data` in the container. Back it up as one unit. See
[Back up and restore](/operate/backup-and-restore.md).

## Next

- Running on one machine right now: [Try it in five minutes](/start/try-it.md).
- A durable setup: [Install and run the hub](/start/install.md), then
  [Deploy as a service](/operate/deploy-as-a-service.md).
- Reach it over the network: [Network and access](/operate/network-and-access.md).
- The full settings list: [Configuration](/reference/configuration.md).
