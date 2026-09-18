---
type: Decision Record
title: The local hub is the cloud
description: There is no vendor cloud and no brain off the node; the node is the central solution.
tags: [adr, deployment, local-first]
status: stable
---

# 0001. The local hub is the cloud

## Context

An agent operations layer can be built as a hosted service the operator
connects to, or as software the operator runs. The first shape moves agent
state and session brains off the machine and makes an external service the
centre of the system. It also tends to make the useful parts a paid tier.

## Decision

The homelab or NAS node is the central solution. There is no vendor cloud and
no brain off the node: the node holds every brain. The hub is one binary or
container the operator runs, and agents on any machine report in over a LAN or
tailnet.

## Consequences

- All data stays on the operator's hardware. Backups are node or NAS
  snapshots.
- The hub must be deployable with almost no setup, which drives the single
  binary and the optional tailnet build flag.
- Availability is the operator's responsibility, which is the intended
  trade.
- There is no tier to pay for and no service to depend on.
