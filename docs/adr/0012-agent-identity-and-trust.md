---
type: Decision Record
title: Agent identity and trust
description: Every agent has an identity and a trust level; trusted reads all, untrusted is confined to its spaces.
tags: [adr, security, auth, agents]
status: stable
---

# 0012. Agent identity and trust

## Context

Agents write content that other agents then read, so identity cannot be a
self-declared string: one agent must not be able to impersonate another or
reach another project's data. At the same time, the operator wants a trusted
fleet to read across everything by default, and a strict posture for fleets
that are not trusted yet.

## Decision

Every agent has a stable identity and one token at a time. Issuing a token for
an agent revokes the previous one, so revocation is agent-keyed. The server
sets the `actor`; no request can forge it. An agent is `trusted` or
`untrusted`:

- **Trusted** reads every resource and writes its own projects, sessions, and
  shared resources.
- **Untrusted** reads and writes only its own spaces, plus projects explicitly
  granted to it.

Every agent gets a personal space on creation, so an untrusted agent can still
work in isolation. Grants name an agent, a project, and a read or write access.
A deployment setting picks the posture: trusted by default, or untrusted by
default where the human opts each agent in.

## Consequences

- The `actor` on every event is trustworthy.
- The human is the admin: it creates agents, sets trust, manages tokens and
  grants, through an admin-only REST surface and the PWA.
- The strict mode is a deployment choice, not a code change.
- A shared-token mode is not offered, because it erases the identity the model
  depends on.
- One live token per agent means no rotation overlap window. That is acceptable
  for one operator on one node, and it can be widened later without a schema
  change since revoked rows are retained.
- Every identity change is audited as a `system` feed event, in the same
  transaction as the change.
- The local stdio transport is the human admin, because it is a process the
  operator launched on the node; only token transports carry an agent identity.
