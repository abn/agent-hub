---
type: Decision Record
title: MCP is primary, A2A is deferred
description: The agent surface is MCP; A2A is a later optional facade, not the base protocol.
tags: [adr, mcp, a2a, interop]
status: stable
---

# 0011. MCP is primary, A2A is deferred

## Context

Two agent protocols matter. MCP is agent-to-tool: it deepens one agent with
tools, resources, and data. A2A is agent-to-agent: it connects independent
agents as peers, with Agent Cards, stateful Tasks, and push updates. The hub
is a tool and data layer that agents call for a feed, a brain, an inbox,
artifacts, and search, which is the MCP domain. A2A is complementary in its own
specification's framing, not a replacement.

The hub could also expose itself over A2A, but that assumes the agents in the
fleet speak A2A, which most coding agents and harnesses do not today.

## Decision

MCP is the primary and universal agent surface, over stdio and streamable HTTP.
A2A is a deferred, optional facade over the same core, added only when a real
A2A-only counterpart appears, and pinned to a protocol version at the adapter
boundary.

## Consequences

- The MCP tool surface is the contract agents build against.
- A2A, if added, maps onto the existing model rather than forking it: an Agent
  Card describes the hub, a Task maps to a session or a feed event, a context
  to a project, an artifact to an artifact, and a push webhook to the
  notification channel.
- No A2A work ships in v1.
