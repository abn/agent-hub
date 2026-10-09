---
type: Reference
title: Operating model
description: One operator, their agents, and what the boundaries between them are for.
tags: [architecture, trust, posture, boundaries]
status: draft
---

# Operating model

This page states the philosophy the whole hub is built on, so the boundaries
between the operator and their agents are read from one place instead of being
re-derived. Where another page disagrees with this one, this one is the
intent.

## The shape

A local node runs the hub. One human operator runs the node, and their agents
reach it over the LAN or a tailnet. This is not a multi-tenant service: there is
one tenant, the operator, and the agents are theirs.

## What the boundaries are for

The boundaries guard against mistakes and bad behaviour inside one household,
not against a determined adversary or another tenant. A caller that already holds
a valid token is inside the trust boundary; the token is an identity, and the
server records it as the actor.

The platform is therefore open by default. There are no accounts, no sign-in,
and no requirement for TLS between the operator's own machines. An artifact link
is a capability: whoever holds it reaches the artifact. Security work is spent on
making the operator's own actions unambiguous, not on isolation the deployment
does not have.

## Who may reach a project

- Every agent token reaches **every ordinary project**, to read and to write, and
  its **own personal space**. Ordinary projects need no grant.
- An agent may **not write another agent's personal space**.
- A **confidential project requires a grant**. A grant is **access or no
  access**: there are no read and write levels.
- Global reads are confined to the projects a caller can see, so a search or an
  inbox read never crosses into one it cannot.

## Creating, and locking

- Any agent may **create projects**, including confidential ones. Confidential
  builds a grant gate, not a creation gate.
- An agent may **make a project confidential**. Only the operator, through the
  admin token, may **make it public again**. The asymmetry is the guardrail: an
  agent can tighten, only the human can loosen.
- Project ids are **not secret**. Because creation is open, creating an id that
  already exists says so.

## The admin boundary is privilege, not use

Some operations belong to the operator alone, not because agents cannot be
trusted with projects, but because they are the operator's controls:

- prune and its undo, and forgetting a knowledge base page's history;
- creating and revoking agent tokens;
- managing grants;
- making a confidential project public;
- deleting a project;
- taking an online backup.

Everything else an agent needs is available to it under the rules above.

## The two surfaces

The **REST API is the operator's control surface**, and it accepts the admin
token. The **MCP server is the agent surface**, and it accepts agent tokens. A
route being admin-only is about which surface owns it, not a claim that agents
may not touch projects; agents reach projects over MCP, and they may create and
list the ones they can see over REST as well.

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
