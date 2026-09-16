---
type: Decision Record
title: Optional embedded tailnet
description: The default deployment is a container behind a reverse proxy; an embedded tailnet endpoint is optional and experimental.
tags: [adr, deployment, tailnet, networking]
status: stable
---

# 0014. Optional embedded tailnet

## Context

The vision described the binary joining a tailnet directly, so no inbound port
would be needed and a tailnet name would address it. A Rust implementation of
Tailscale, `tailscale-rs`, now exists and is officially maintained, with a
userspace network stack and an in-process device API, which would keep the
single-binary promise. It is pre-1.0, and it does not yet provide MagicDNS,
HTTPS certificate issuance, or working NAT traversal, so a device is reachable
by tailnet IP and traffic may relay through public relays.

## Decision

Two deployment modes. The default is the plain container behind a reverse
proxy, which owns TLS. The optional embedded tailnet endpoint uses
`tailscale-rs`: the process joins the tailnet in userspace and listens there,
addressed by tailnet IP, with TLS terminated by the hub. Embedded mode is
experimental until NAT traversal, MagicDNS, and certificates are available
upstream.

## Consequences

- The default path needs no Tailscale code and no open ports beyond the proxy.
- Embedded mode keeps one binary and no sidecar, at the cost of MagicDNS
  naming and relay latency.
- The tailnet integration is a build concern isolated behind one seam, so the
  plain and embedded builds share the rest of the code.
