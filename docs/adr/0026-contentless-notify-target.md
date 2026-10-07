---
type: Decision
title: A contentless nudge to an operator-configured target
description: Why the hub may POST one fixed sentence to a URL the operator runs when something starts waiting on the human, what that refines in the push deferral, and why Web Push stays deferred.
tags: [adr, notifications, privacy, operations]
status: stable
---

# 0026. A contentless nudge to an operator-configured target

## Context

[ADR 0016](0016-push-notifications-deferred.md) deferred background push
because Web Push puts a browser-chosen vendor service in the transport path and
needs a secure context. The cost it accepted is that an installed app that is
fully closed raises nothing: the operator learns of a waiting question when the
app next opens.

Many operators already run a notification service of their own on the same LAN
or tailnet as the hub: a self-hosted ntfy, a Gotify behind a small bridge, or a
home automation webhook. The node is the cloud, so a target the operator chose
and runs is part of their own estate, not a third party. What 0016 rejected was
a transport the hub cannot choose; this one the operator chooses outright.

## Decision

The operator may configure one outbound URL, `notify_url` (`HUB_NOTIFY_URL`),
http or https only. With it set, a write that leaves something waiting on the
human (a new question, a new approval, or an enrolment request) also wakes a
background sender, which POSTs a fixed plain-text body to the URL:

```
Something is waiting for you in Agent Hub.
```

- **Contentless.** The body is that one sentence. It names no project, agent,
  title, id or count, and no header adds any. The hub's own address is not sent
  either: an operator who wants a tap to open the app sets that on the target,
  such as ntfy's `click` query parameter, where it is their choice.
- **Opt-in and off by default.** With no URL the sender is not started and
  nothing changes. An optional `notify_token` is sent as
  `Authorization: Bearer`, and is masked wherever the configuration is printed.
  The URL may not carry credentials, and only its origin is ever printed or
  logged, since a topic path or a query can act as a secret on some targets.
- **Coalesced.** At most one send per `notify_interval_secs` (default 60). A
  trigger inside the quiet period produces one trailing send when it ends, so a
  burst of questions is two nudges, not one each.
- **Fire and forget.** The sender is its own task with a ten-second timeout and
  no retries beyond the next coalesced trigger. It never blocks or fails the
  write that woke it. Each send is counted as `delivered` or `failed` in
  `agenthub_notify_sends_total`, and a failure is logged at warn with the
  target's origin and the cause, never the token.

The sender is a small client over hyper, hyper-util and hyper-rustls, which the
serve-only container already links through the engine's own client at the
versions the lock file pins. Taking them directly adds no crate to the image,
keeps rustls with the aws-lc-rs provider the build already carries, and links
no OpenSSL. `reqwest` stays behind the `client` feature: making it a regular
dependency would have pulled it and its platform verifier into a container that
only serves.

Web Push stays deferred, on the terms 0016 states.

## Consequences

- An operator who runs a notification service gets a background nudge with the
  app fully closed, and no vendor sits in the path. One who does not configure
  a target sees exactly the 0016 behaviour.
- The target learns only that something is waiting and when. The operator
  opens the app to see what, over the same authenticated surface as before, so
  the mailbox stays pull-based.
- The trigger is the write path that already nudges the freshness stream, so
  nothing polls and a write that does not wait on the human, such as a finished
  signal or an answer, sends nothing.
- One target, one body. A second target, per-project routing, or a templated
  body would put content on the wire and is out of scope for this record.
- 0016 is refined rather than superseded: its decision against a vendor push
  service still holds.
