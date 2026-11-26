---
type: Explanation
title: Operating model
description: The trust posture and who may reach what
tags: [operator, security]
status: stable
---

# Operating model

This page states the philosophy the whole hub is built on, so the boundaries
between the operator and their agents are read from one place instead of being
re-derived. Where another page disagrees with this one, this one is the intent.

## The shape

A local node runs the hub. One human operator runs the node, and their agents
reach it over the LAN or a tailnet. This is not a multi-tenant service: there is
one tenant, the operator, and the agents are theirs.

## What the boundaries are for

The boundaries guard against mistakes and bad behaviour inside one household,
not against a determined adversary or another tenant. A caller that already
holds a valid token is inside the trust boundary; the token belongs to a member,
and the server records the member, the agent and the session on every write.

> [!IMPORTANT]
> A token is an identity, not a credential that narrows access by itself. A
> caller that already holds one is trusted to act as its member, so the
> boundaries below are guardrails on that actor, not isolation between actors.

The platform is therefore open by default. There are no accounts, no sign-in,
and no requirement for TLS between the operator's own machines. An artifact
link is a capability: whoever holds it reaches the artifact. Security work is
spent on making the operator's own actions unambiguous, not on isolation the
deployment does not have.

## Who may reach a project

- Every member token reaches **every ordinary project**, to read and to write,
  and its **own personal space**. Ordinary projects need no grant.
- A member may **not write another member's personal space**.
- A **confidential project requires a grant**. A grant is **access or no
  access**: there are no read and write levels.
- Global reads are confined to the projects a caller can see, so a search or
  an inbox read never crosses into one it cannot.

The MCP tools that read and write projects are in
[MCP tools](/reference/mcp-tools.md), and the routes that administer access are
in [REST API](/reference/rest-api.md).

## Creating, and locking

- Any member may **create projects**, including confidential ones. Confidential
  builds a grant gate, not a creation gate.
- A member may **make a project confidential**. Only the operator, through the
  admin token, may **make it public again**. The asymmetry is the guardrail: an
  agent can tighten, only the human can loosen.
- Project ids are **not secret**. Because creation is open, creating an id that
  already exists says so.

## The admin boundary is privilege, not use

Some operations belong to the operator alone, not because agents cannot be
trusted with projects, but because they are the operator's controls:

- prune and its undo, and forgetting a knowledge base page's history;
- creating and revoking member tokens;
- managing grants;
- making a confidential project public;
- deleting a project;
- taking an online backup.

Everything else an agent needs is available to it under the rules above.

## The two surfaces

The **REST API is the operator's control surface**, and it accepts the admin
token. The **MCP server is the agent surface**, and it accepts member tokens. A
route being admin-only is about which surface owns it, not a claim that agents
may not touch projects; agents reach projects over MCP, and they may create and
list the ones they can see over REST as well.

## The operator's phones

A phone pairs through Tendril and the operator confirms it in the web app. A
confirmed phone is the operator, as the PWA is: it reads, answers, approves,
prunes and approves joining machines, and sees every project. It holds no
token. It reaches an allow-list of what the app needs. Member tokens, grants,
pairing phones, notify targets, making a project public, deleting a project,
online backups, share links and viewer passes stay on the admin token.
Approving a joining machine is open to the phone by the owner's choice, as the
machine asked and granting at most the projects it asked for; the member token
it issues outlives unpairing the phone. Unpairing a phone refuses
it at once.

Phones are the one exception to "nothing reaches a vendor cloud": once a phone
is added, the hub also offers Tendril's public relay, so a phone reaches it
away from home. The relay carries an end-to-end encrypted session and sees no
content, and `HUB_PHONES_RELAY=off` turns it off. See
[the decision](/adr/0041-phones-pair-and-connect-through-tendril.md).

## Members, agents and sessions

A **member** is what holds a token and was approved to join, usually one
machine. The **agent** is the program doing the work, such as `claude-code` or
`opencode`; the hub reads it from the client's handshake, and a header can
override it. A **session** is a shared space of work that any participant joins
by name. Several agents share one member, and every write records its member,
its agent and its session as three fields.

None of the three grants anything. What a caller reaches is decided by its
member's token: the agent name and the session select nothing and switch no
member. The admin token ignores them. A second member on one machine is for work
that must not share a token, such as a confidential project. See
[the decision](/adr/0039-members-agents-and-sessions.md).

## Loopback

On a loopback bind the hub may resolve a principal without a token: the caller
is the member `loopback`, and the agent comes from its handshake. The control
surface resolves to the admin when no admin token is configured. Every process
on that machine is already the operator, so nothing crosses a trust boundary.
Any bind past loopback requires a token for every caller, or an approved
enrolment.

## Artifacts and links

- A **plain artifact** is reached by a link. The link is the capability, one per
  artifact, and it can be revoked.
- A **protected artifact** is encrypted on the client before it is published.
  The hub stores ciphertext and the envelope and never the key, so it cannot
  read the artifact or recover the password. Withdrawing a protected artifact
  means deleting it.

## What is not a goal

Isolation between tenants, defence against someone who already holds a token,
and stopping an agent from doing what its operator asked. Features that would
only serve those goals are out of scope for this deployment.

## See also

- [Components](/architecture/components.md) - the process that enforces these
  boundaries.
- [Data model](/architecture/data-model.md) - where grants and identities are
  stored.
- [Goals and non-goals](/design/goals.md) - the wider v1 scope.
