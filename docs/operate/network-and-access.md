---
type: Guide
title: Network and access
description: Bind the hub, terminate TLS, or reach it over a tailnet
tags: [operator, security, config]
status: stable
---

# Network and access

By the end you will know which address the hub answers on, what that port
carries, and how to close it. The admin token and the install are in
[Install and run the hub](/start/install.md).

## Before you start

| Thing | Why |
|---|---|
| The hub installed and running | this page adjusts how it is reached, not how it starts |
| A decision on who reaches it | one machine, a trusted LAN, or the internet |
| An admin token | required the moment the bind is not loopback |

## What the published port carries

The hub speaks plain HTTP. The admin token travels in an `Authorization` header
on every control request from the app and every admin call, so on a port bound
to anything but loopback it crosses the network unencrypted, as does everything
the hub returns. On a trusted home network that may be the deployment you want.

> [!WARNING]
> A LAN or public bind carries plain HTTP. Terminate TLS in front of the hub, or
> reach it over a tailnet, before you widen the bind.

## Bind the hub

The hub must be reachable from every machine an agent runs on, and from the
browser you watch it from. Pick one:

| Situation | Bind the hub to | Notes |
|---|---|---|
| Everything on one machine | `127.0.0.1:8080` (the default) | simplest; nothing leaves the machine |
| A trusted home LAN | the host's LAN address | plain HTTP; set an admin token |
| Dynamic address, or a network you do not trust | loopback plus a TLS reverse proxy | set `HUB_PUBLIC_URL` to the address callers use |
| Machines across the internet, no public port | a tailnet | a Tailscale node on the host, or the experimental embedded endpoint |

`bind` and `HUB_BIND` are in [Configuration](/reference/configuration.md). With
port `0` the system picks a free port, and the startup line names the bound
address.

## TLS and a reverse proxy

The supported way to carry TLS is a reverse proxy in front of the hub. Publish
the container port to loopback only, or to a private network, and set
`HUB_PUBLIC_URL` to the address the proxy serves:

```sh
podman run --detach --name agent-hub \
  --publish 127.0.0.1:8080:8080 \
  --env HUB_ADMIN_TOKEN="$HUB_ADMIN_TOKEN" \
  --env HUB_PUBLIC_URL=https://hub.example \
  --volume agent-hub-data:/data \
  ghcr.io/abn/agent-hub:1.0.0
```

`HUB_PUBLIC_URL` takes a bare origin, scheme and host with an optional port and
no path, and a bad value fails startup. Without it the hub reads its own address
off each request: the forwarded scheme and host, then the request host, then the
bind. Set it when a proxy rewrites the host to the upstream address, since the
address the hub then sees is not the one callers use. It is what the artifact
frame policy names, what a shared artifact link carries in its preview tags, and
what `GET /bootstrap/SKILL.md` hands a bootstrapping agent.

A reverse proxy may also mount the hub on a path instead of a dedicated host or
port, such as `https://host/hub/`, as long as it strips that path before
forwarding. The hub always serves from its own root and never needs to know a
prefix exists; there is no `HUB_BASE_PATH` or equivalent, and a proxy that does
not strip the prefix is not supported. The app itself normalises a request that
reaches it without a trailing slash (`/hub`) to one that has it (`/hub/`) before
it loads anything else, since every asset it loads resolves relative to that
address.

When a proxy runs on the same host, the hub sees the proxy as the client unless
you list it. Set `trust_proxy` (`HUB_TRUST_PROXY`) to the comma-separated peer
IP addresses whose forwarded client header the hub trusts, such as `127.0.0.1`.
Unset trusts none, and the socket peer is the identity.

## A tailnet

A tailnet is the other supported way to reach the hub without a public port.

- **A Tailscale node on the host.** Run Tailscale on the node and reach the hub
  by its tailnet address. The hub itself needs no tailnet setting.
- **The embedded tailnet endpoint.** Set `HUB_TAILNET` to a Tailscale auth key
  and the hub joins the tailnet itself. It is addressed by tailnet IP and
  carries plain HTTP inside the tunnel, so it needs `HUB_ADMIN_TOKEN` as well.

> [!CAUTION]
> The embedded tailnet endpoint is experimental and is compiled into the default
> build. Setting `HUB_TAILNET` is the acknowledgement that it uses early-days
> software; the hub records that on startup, so no extra variable is needed. The
> container image builds with `--no-default-features`, so it does not carry the
> endpoint. Leave `HUB_TAILNET` unset for the default deployment: the plain
> container behind a reverse proxy.

`HUB_TAILNET_PORT` (default `8080`) is the port served on the tailnet address,
and `HUB_TAILNET_CONTROL_URL` names a control server for a self-hosted control
plane. Over the embedded endpoint the socket peer is not available, so
enrolment requests share one identity.

## You should now see

Callers reaching the hub at the address you chose, with TLS or the tunnel in
front of it. Confirm the address the hub reports:

```sh
curl -sS https://hub.example/bootstrap/SKILL.md | head
```

The guide inside names the public origin rather than the internal bind.

## See also

- [Deploy as a service](/operate/deploy-as-a-service.md) - start on boot
- [Configuration](/reference/configuration.md) - `bind`, `public_url`, `trust_proxy`, the tailnet keys
- [Operating model](/architecture/operating-model.md) - the trust posture behind the boundaries
