---
type: Decision Record
title: Loopback may resolve identity without a token
description: On a loopback bind the hub resolves an agent from a declared identity and an admin without a token, because every process on that machine is already the operator.
tags: [adr, security, auth, agents]
status: stable
---

# 0031. Loopback may resolve identity without a token

## Context

A hub bound to loopback serves one developer machine. Every process that can
reach it runs as the same OS user and can already read the store, so requiring
a bearer token there adds ceremony without adding a boundary. Issue tokens
and enrolment gating still matter once traffic leaves that machine.

## Decision

On a loopback bind only:

- An MCP request without an `Authorization` header may declare its identity with
  `X-Agent-Id`. The value is validated like any agent id and, if the agent does
  not exist yet, the hub creates it and its personal space as an active agent.
  The resolved principal has `is_admin: false`, with the agent's own session
  ownership and personal-space write lock.
- The REST control surface resolves to the human admin when no admin token is
  configured.
- A token, when presented, is still verified exactly as on any other bind.

Nothing about this applies past loopback. A non-loopback bind keeps requiring a
token for every caller, including agents, and an admin token to serve.

## Consequences

- A single-machine workstation hub works with `HUB_URL=http://127.0.0.1:8080`
  and `HUB_AGENT_ID=<tool>`, with no enrolment step.
- The `whoami` result names the declared agent, and its sessions belong to it.
- Identity on loopback is self-declared, so audit attribution there assumes every
  local process is the operator. That assumption is the loopback bind itself.
- Confidential projects and grants are unchanged: a loopback agent is not an
  admin and still needs a grant to see or write a confidential project.
- This does not replace enrolment or tokens for LAN, tailnet, or remote callers.
