---
type: Decision Record
title: The token is the identity, and trust is removed
description: A token names who is calling rather than which agent, every call authenticates, ordinary projects are open to any token, and a confidential project is reached by grant and is absent to everyone else.
tags: [adr, identity, tokens, authorization, confidential]
status: proposed
---

# 0021. The token is the identity

## Context

The hub authorizes with a trust ladder. A caller resolves to a `Principal`
carrying `Trust::Trusted` or `Trust::Untrusted`, and `policy::authorize` reads
it: a trusted caller reads every project and writes any project that is not
another agent's personal space; an untrusted caller reaches its own space plus
whatever a grant names, where a grant carries `read` or `write`. The default
for a new agent comes from `HUB_TRUST_DEFAULT`.

Three things went wrong with it in use.

**The ladder confuses two questions.** Trust was asked to answer both "is this
caller allowed here" and "how careful should the hub be with it", and it
answered neither cleanly. A trusted agent reaching everything is not a
statement about that agent; it is the absence of a statement about the
project.

**It made confidential work impossible to express.** The owner wants projects
that most callers cannot see. Under the ladder the only way to arrange that is
to make every other agent untrusted, which then also removes their access to
ordinary work they should have. The confinement machinery for the real answer
already exists and is proven: `Visibility::All | Only(Vec<String>)` confines
search, the feed, the inbox, artifacts, the brain and comments.

**An agent's name was doing work it cannot do.** An agent declares its own
name to the hub. A name a caller chooses cannot be the basis of a permission
decision, because nothing stops the caller choosing another. In practice the
token was already the identity and the name was decoration on top of it.

Two things about the setting matter for what follows. The hub is one
operator's fleet on their own node, so this is confinement and coherence
rather than defence against an attacker who already holds a token. And the
hub is not released, so removing a concept outright is cheaper than carrying
a compatibility path for it.

## Decision

**A token is an identity.** It names who is calling. It may be shared by
several agents deliberately: a fleet that should count as one caller is given
one token, and the hub treats them as one caller because they are.

**A declared agent name sets attribution only.** It decides whose name appears
on an event, a session and an artifact. It never decides what a caller may
reach. Anything that reads a name to make an authorization decision is a bug.

**Every call authenticates.** There is no tokenless tier and no anonymous
read. A non-loopback bind already makes an admin token mandatory at startup;
this extends the same rule to the whole surface. Anonymous access to a single
artifact is a separate thing and stays that way: a share link is a capability
for one artifact, not a way into the hub.

**Ordinary projects are open to any token that authenticates.** Most work on a
personal hub is not secret, and asking the operator to grant every agent every
ordinary project is a tax with no matching risk.

**A confidential project is reached only by a token with a grant.** The flag
lives on the project, which is where the operator already thinks about it.

**To everyone else a confidential project is absent, not refused.** A refusal
tells a caller the project exists, which is the one fact the flag is meant to
withhold. Absence is what `Visibility::Only` already produces across the
confined surfaces, so this is the existing behaviour applied to a new reason.

**Grants are binary.** A grant either reaches a project or it does not. The
`read` and `write` split was never asked for by the product and only exists
because the ladder needed rungs.

**Trust is deleted rather than reinterpreted.** No trust column, no
`HUB_TRUST_DEFAULT`, no `trust` field on agent creation, nothing in the
interface, and nothing about it in the skill the hub serves.

## Consequences

The authorization question becomes: does this token authenticate, and if the
project is confidential, does it hold a grant. Two inputs instead of a ladder
crossed with an ownership rule.

A personal space stays its owner's own. That is an ownership rule, not a trust
one, and it survives unchanged.

**The removal is not done, and the hub is currently inconsistent.** At the
time of writing the interface has no trust: round 4's access screen removed it
from the surface. The API has not followed. `src/principal.rs` still defines
`Trust`, `src/policy.rs` still branches on it, `POST /api/v1/agents` still
accepts a `trust` field, `HUB_TRUST_DEFAULT` still exists, and the skill
served at `/SKILL.md` still teaches agents about `trusted` and `untrusted`
callers and `read` versus `write` grants. An agent onboarding today is taught
a model the product has decided against.

That inconsistency is the reason this record exists. The decision was taken in
conversation and recorded in a hub artifact, which is not somewhere a new
session or a new agent reads, so the surface moved and the API did not. Until
the code catches up, this document is what the target is, and the divergence
above is the work.

Confidential projects have no column yet either. Round 4's access screen draws
the empty state honestly rather than listing ordinary projects as though they
were confidential.

**What this gives up.** Sharing one token across several agents means the hub
cannot tell them apart for authorization, and does not try to: attribution
still separates them by name, and a fleet that wants separate reach takes
separate tokens. Binary grants mean a caller that should read but not write a
confidential project cannot be expressed; if that turns out to be wanted, it
returns as a grant attribute rather than as a revived ladder.

## Status

Proposed. The decision is settled with the owner; the code has not caught up,
and the divergence is listed above. Move this to `stable` when `Trust` is gone
from `src/principal.rs`, `src/policy.rs` and the agent API, when the
confidential flag exists, and when `assets/SKILL.md` describes this model
rather than the ladder.
