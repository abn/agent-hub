---
type: Decision Record
title: A name declared beside a token is attribution only
description: A request with an agent token and X-Agent-Id acts as a sub-identity of the token's agent, so a host shares one token and still attributes work per agent.
tags: [adr, identity, tokens, attribution, agents]
status: stable
---

# 0036. A name declared beside a token is attribution only

## Status

Accepted (companion to ADR 0021 and ADR 0031).

## Context

ADR 0021 lets several agents share one token and says a declared name sets
attribution only. Nothing implemented the second half. With a token the actor
was always the token's agent, so a host that gave all its agents one token got
one author on every event, one owner on every session, and one cursor and one
set of subscriptions for all of them. The only way to tell the agents apart was
a token each, which turns a per-host credential into a per-agent chore.

The wish is the other way round: authentication per host, attribution per
agent. Each host holds one token, and every agent on it sends its own name.

ADR 0031 already reads `X-Agent-Id`, but only when no token is presented, on a
loopback bind with no admin token. A client that has a token sends the header
nowhere, and the hub would ignore it if it did.

## Decision

**Reuse `X-Agent-Id`.** The CLI already forwards `HUB_AGENT_ID` in it, and
`HUB_AGENT_ID` is already documented as the caller's advisory label. A harness
configured for a loopback hub keeps its configuration when it moves to a
networked hub with a token. What the header means depends on whether a token
came with it, and the two never overlap: without a token it names the agent
(ADR 0031), beside a token it names a sub-identity of the token's agent.

**The actor is `<agent>/<name>`.** The token still decides who the caller is.
The hub resolves it exactly as before, and the principal keeps the token's
`agent_id`, which is all authorization reads. Only the label changes: the actor
on events, artifacts and comments, the owner of a session, the key of a feed
cursor, a notification cursor and a subscription. The label carries the
authenticating agent in itself, so an audit line says both who vouched for the
call and which agent made it, and it needs no new column.

**The prefix is what keeps a name from becoming an agent.** A name that spells
another agent's id, such as `agent-b`, becomes `agent-a/agent-b`, which is not
`agent-b` and owns none of its sessions. The one way to land on a real agent id
is an agent whose id already contains the slash, as `claude-code/laptop` does.
The hub checks for that on each request and answers a `conflict` rather than let
two callers share a label. The check is made when the request arrives, so an id
registered later takes over the label's history from then on. Agent ids stay
free to hold a slash, and an operator who names agents that way avoids token
agents whose names would complete one.

**A name is free-form and needs no registration.** It creates no agent row and
no personal space. A sub-identity works in its token agent's personal space and
reaches exactly what the token reaches, no more and no less. Names run 1 to 64
characters: ASCII letters and digits, then letters, digits, `.`, `_` or `-`.
There is no slash, which is what joins the two parts, and nothing a header, a
log line or a query would need to escape. Case is kept as sent.

**A bad name is refused, not dropped.** A name that fails the spelling is `400
invalid_argument`, and one that is not plain text is the same. Dropping it
silently would file an agent's work under its host without telling either of
them. A blank header declares nothing. The token is resolved before the header
is read, so a bad token is still a `401` and the admin token never meets a
header error.

**Three cases are not sub-identities.** The admin token acts as `human` and
ignores the header, since the human's own actions are not attributed to an
agent's name. A name equal to the token's own agent id is the agent itself. A
request with no token is unchanged: refused past loopback, and on loopback
resolved by ADR 0031.

**ADR 0031 is not widened.** Local identity without a token still needs a
loopback bind and no admin token. A token presented on loopback is verified
exactly as on any bind, and the header beside it is a sub-identity, never an
agent: it creates no agent row. A hub with an admin token still refuses a
caller that sends only a name.

**It applies where an agent's call is recorded.** The MCP endpoint at `/mcp`,
the REST project routes that accept an agent token, and the CLI, which sends
`HUB_AGENT_ID` beside its token when the hub would accept it and says so on
stderr when it does not. The live-update stream takes agent tokens too but
records nothing, so it ignores the header. The REST
control surface takes the admin token only, so the header means nothing there.

## Consequences

- One token per host works. Each agent on it has its own session names, its own
  `inbox_wait` and feed cursors, its own subscriptions, and its own name in the
  feed. Two agents that pick the same session name get two sessions.
- Authorization is untouched. A name grants no project, borrows no grant, and
  switches no agent, whatever it says. Revoking the token removes every
  sub-identity under it at once.
- Names are self-declared, as ADR 0021 says they are. A holder of the host's
  token can use any name under that host, including a sibling's. That was
  already true of the token, since the siblings shared it. It cannot reach
  another token's agent, because the label begins with its own.
- A call with the bare token and a call with a name are different actors. The
  bare token cannot write a session a sub-identity started, and the other way
  round, the same as two agents. The human can reassign a session, but only to
  a registered agent, so a sub-identity is not a reassignment target.
- A client that already sent a token and a `HUB_AGENT_ID` different from the
  token's agent changes owner on upgrade. Its work is attributed to
  `<agent>/<id>`, a session name it reuses starts a new session, and its feed
  and notification cursors and subscriptions start over under the new label.
  The earlier work stays under the token's agent, and `session_start` with
  `from` picks it up. An id equal to the token's agent is unchanged, and one
  the hub would refuse is left off by the CLI with a warning, so no working
  configuration starts failing.
- A sub-identity is not an entry in Access. It appears as the actor on what it
  did and is not listed or managed anywhere else.
- Rejected: a separate header, which splits one concept across two names and
  breaks every harness already sending this one. A bare name as the actor,
  which lets a name land on another agent's sessions and cursors. A registry
  of sub-agents, which turns attribution into the enrolment chore this
  removes. A new column beside the actor, which touches every table that
  records one for a fact the label already carries.
