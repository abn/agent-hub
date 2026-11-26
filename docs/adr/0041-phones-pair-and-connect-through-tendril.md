---
type: Decision Record
title: Phones pair and connect through Tendril
description: The hub embeds a Tendril host; Tendril owns pairing, device keys, revocation, routes and the encrypted session, the hub maps a confirmed phone to the operator, and a phone's requests are HTTP/2 over its Tendril session.
tags: [adr, phones, tendril, pairing, transport, identity]
status: stable
---

# 0041. Phones pair and connect through Tendril

## Status

Accepted.

## Context

The operator wants their phone to read, answer, approve and prune the way the
PWA does, from the home LAN, over a tailnet and from anywhere else, without a
certificate on the hub and without the operator running a relay. The REST
surface takes only the admin token, and a phone needs a credential of its own
that the operator can take back.

Each of those needs, a pairing code, a device key, a check against a
man in the middle, revocation, route discovery and an end-to-end encrypted
transport, is a layer the hub would otherwise build. Tendril, a pairing and
transport library, already ships all of them as one piece: a signed
`tendril://pair` URI with up to six route hints, a short authentication string
both sides compare, a roster of paired peers with pending, active and revoked
states, and a hybrid post-quantum Noise session that it routes over LAN,
tailnet or a relay on its own.

## Decision

### Tendril is the layer, used as it ships

The hub embeds a Tendril host and adds no pairing, key, route or revocation
logic of its own. Tendril picks the routes; the hub never selects one. The
host keeps its key and roster under `tendril/` in the data directory.

The host starts on the first "Add a phone", or in the background at serve
time once a phone is paired or waiting, and stops again when no phone is paired
and no code is live. A hub that never pairs a phone opens no port and reaches no
relay. `HUB_PHONES=off` turns phones off, and a build without the `phones`
feature has none. The direct listener is `HUB_PHONES_LISTEN`, the hub's bind
address on port 47319 by default; the container publishes 47319.

### An exception to "no vendor cloud": the relay

Once a phone is added, the hub also offers Tendril's public relay route (the
`n0` relays), so a phone away from home reaches it with no tailnet. This is an
exception to the hub's rule that nothing reaches a vendor cloud, made by the
owner: the relay carries Tendril's end-to-end encrypted session and sees no
content, only that two keys talk; nothing is sent to it before a phone is
added; and `HUB_PHONES_RELAY=off` keeps phones to the LAN and a tailnet. Add a
phone says so in one line. If the relay cannot be reached, the host still
serves phones directly.

### Pairing

Settings > Phones issues a Tendril code, always the long three-group kind so
its strength never rests on where a claim seems to come from (behind port
forwarding every peer looks local), and its host-signed URI, drawn as a QR
code, with the hub's LAN or tailnet address beside the code for typing it by
hand; a loopback address is never shown. A new code withdraws the one before
it, and Close withdraws the code on screen, so only the code the operator is
looking at can be claimed, and the host takes pairing streams only while one
is live. The phone
pairs, both sides show the check string, and the phone stays pending until the
operator confirms the string in the web app, which calls Tendril's confirm with
it. Refusing removes the pending peer from Tendril's roster.

### One principal: the operator on a phone

A table, `devices`, holds one row per paired phone: its Tendril device id,
name, state and when it paired, was confirmed, was unpaired and was last seen.
Tendril's roster decides who may open a session; this row decides who a
request acts as. A request over an active phone's session is the operator, as
the PWA is, with the phone named on the principal. It is not a member and holds
no token.

What the phone may call is an allow-list of the routes the app needs: reading
Now, the inbox, projects, feeds, sessions, artifacts and members, answering,
deciding, approving or refusing a joining machine, capturing ideas, ending a
session, and pruning with its undo. Every other route is refused by the route
the request matched, before a handler runs, with `forbidden` and the reason
`operator_only`, and a route added later is refused until it is added to the
list. A test walks every route the router serves and fails on one with no
decision.

Among the refusals are the operator's controls over credentials and the hub
itself (members, tokens, grants, phones, notify targets, renaming,
declassifying or deleting a project, backups), and share links and viewer
passes, which would keep working after the phone is unpaired. Approving a
joining machine stays open to the phone by the owner's choice: it issues a
member token, and unpairing the phone does not undo it. A phone approves the
machine as it asked and grants only projects it asked for; renaming the member,
sharing, or granting any other project is refused as `operator_only` and stays
with the web app. A machine names the projects it asks for when it enrols
(`projects`), and the request on the inbox carries them, so the phone can offer
each one to grant. An artifact's frame page, which a share link hides from everyone else, is
open to the phone as it is to the operator's viewer, since the phone cannot
mint the viewer's pass.

The device routes take writes only from the hub's own web app (the browser's
`Sec-Fetch-Site` or `Origin` says same-origin), so on an open loopback hub a
page on another site cannot pair and confirm a phone of its own through the
operator's browser. Same-origin alone is defeated by DNS rebinding, where a
stranger's name resolves to the hub and the browser sends it as both `Host` and
`Origin`, so every device route also requires a `Host` the hub owns: an IP
literal, a loopback name, or the host of `HUB_PUBLIC_URL`. The list shows a
check string only for the newest pending phone, and only to that app.

A network client cannot become a phone: the phone principal is set only by
the transport that serves a Tendril session, and any `Authorization`, cookie
or agent header a phone sends is removed before routing.

### Transport: HTTP/2 over the session

The payloads of the session's data frames, in order, form one byte stream in
each direction, and that stream carries HTTP/2 with prior knowledge, the phone
as client and the hub as server. The hub serves it with the same router as its
HTTP listener, minus the MCP endpoint.

This is the simplest mapping that reaches the existing REST API and the event
stream unchanged:

- No new protocol: HTTP/2 is the framing, so there is no request envelope,
  stream id or chunking scheme of the hub's own to specify, version and test.
- One session carries the held event stream and any number of requests at
  once, as HTTP/2 streams, so the phone needs one connection per hub.
- Every route, status code and problem body is the one the PWA already gets.

A JSON envelope per request was the alternative. It is easier to write on the
phone, but it is a second protocol beside HTTP that the hub would own, which is
the kind of custom layer this decision removes.

### Unpairing

Unpairing marks the row unpaired first, so a request in flight is refused with
`unauthenticated` and the reason `device_unpaired`, then revokes the peer in
Tendril's roster, then closes every live session of the phone with Tendril's
revoked close code, 4001. A phone unpaired while the host was not running is
revoked in the roster when the host starts.

Every request over a phone's session checks the phone's row first, so a phone
unpaired while its session is open is refused on its next request even before
the session is closed.

### Stopping

The hub closes every phone session with a close frame and gives the running
ones a few seconds to finish before it checkpoints the store. Sending a close
frame waits at most two seconds, so a phone that stopped reading cannot hold
the hub.

### Wake, for later

`HUB_PHONES_WAKE_FILE` names a private file of Tendril's wake configuration
keys. It is read and checked when the host starts and kept for a later wake
provider (iOS, APNs). Nothing sends a wake yet.

### Version

The dependency is pinned to Tendril's main branch at `68c215b` and moves to
the `v0.2.0` tag when it is published. That revision does not build on
Windows, so phones are not available on Windows hubs yet: a Windows build
behaves as one without the `phones` feature until the tag lifts the gate.

## Consequences

- A session checks requests against a copy of Tendril's roster taken when it
  opened. Unpairing closes the session at once, so the copy matters only for
  Tendril's own timers (rekeying), which keep running on it until the session
  ends. This is a known limit.
- A write a phone makes is recorded as the operator's, like any write from the
  web app. The store's author record has no place for the device, so which
  phone made it is in the hub's log only.
- Backups do not carry `tendril/`. Restored on the node that made the backup,
  with `tendril/` left in place, phones keep working. Restored anywhere else,
  the hub has a new Tendril key that no phone pinned: unpair every phone and
  pair it again.

- The hub's build carries Tendril, and with its public relay the Iroh stack.
- A phone's credential is its Tendril key. Losing the phone is handled by
  unpairing it; nothing on the phone is a bearer token that works elsewhere.
- Tendril 0.1 refuses a pending or revoked peer inside the handshake and sends
  it nothing, so a phone that was offline when it was unpaired sees its
  connection end with no close frame. It tells "unpaired" from "unreachable"
  only by its own record and repeated refusals until Tendril sends an
  authenticated refusal.
- The path a phone connected over is Tendril's to report. The hub does not
  show it until Tendril exposes it.

## See also

- [Operating model](/architecture/operating-model.md)
- [ADR 0012](0012-agent-identity-and-trust.md), [ADR 0039](0039-members-agents-and-sessions.md)
- [Phones](/operate/phones.md)
