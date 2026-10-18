---
type: Guide
title: Monitoring
description: Watch health, metrics, integrity, and the limits that grow with use
tags: [operator, operations, config]
status: stable
---

# Monitoring

By the end you will know which probe to point at the hub, which numbers to
scrape, and where the store's own limits are. The backup that pairs with
integrity checking is in [Back up and restore](/operate/backup-and-restore.md).

## Before you start

| Thing | Why |
|---|---|
| An admin token | `/metrics` is admin-gated like the rest of the control surface |
| A supervisor or orchestrator (optional) | decides when the node is out of rotation |
| A time-series system (optional) | scrapes `/metrics` before the readiness margin trips |

## Liveness and readiness

Two probes answer different questions:

- `GET /healthz` is liveness only: the process is up.
- `GET /readyz` is readiness, and it answers `200` only when the store is
  usable. The schema version is read, is supported, and matches what the process
  opened; the data directory and `hub.db` still have the identity they had at
  open, so a removed or replaced store is caught; a content read confirms it is
  still a hub store; and the volume has free space above a safety margin.

Any leg failing is a `503` problem, so a supervisor can take the node out of
rotation instead of holding it there while every call fails.

`agent-hub health [--url URL]` asks `/readyz` and exits 0 when ready, non-zero
otherwise. It is what the container healthcheck runs, because the runtime image
is distroless and has no shell or `curl`.

## Metrics

`GET /metrics` reports the hub's own counters in Prometheus text:

- HTTP requests, by method and status class;
- failed responses, by hub error code;
- events appended, by kind: `agenthub_events_total`, whose `kind` label carries
  `anchor` and `score` beside the other kinds;
- MCP tool calls, by tool;
- sends to the notify target, by result
  (`agenthub_notify_sends_total`, `delivered` or `failed`).

It also carries storage gauges, read fresh for each scrape: free bytes on the
data volume, the `hub.db` size, and the write-ahead log size. A time-series
system can alert before the readiness margin trips rather than only when the
node is nearly out of room. The route is admin-gated like the rest of the
control surface, so the scraper carries the admin token in
`Authorization: Bearer`. There is no separate scrape credential.

Set `integrity_sample_secs` (`HUB_INTEGRITY_SAMPLE_SECS`, default `0`, disabled)
to a number of seconds to have the hub sample the store's own integrity in the
background, on the sweeper's cadence. Each sample runs `PRAGMA quick_check` on
its own connection, never inside a request, bounded by a wall-clock cap and
skipped while the previous one has not finished, so a large or damaged store
cannot pin the loop. It raises two series: `agenthub_integrity_ok`, a gauge that
is `1` for a passing sample and `0` for a failing one (absent until the first
sample runs), and `agenthub_integrity_failures_total`, a counter of samples that
timed out, errored, or reported a problem. A failed sample is what tells you a
zeroed page has appeared between offline `check` runs.

## Offline integrity

Two read-only commands walk the store with the full `PRAGMA integrity_check`:

```sh
agent-hub check --data-dir /path/to/data
agent-hub doctor --data-dir /path/to/data
```

`check` verifies any backup manifest present, runs the integrity check on the
store and every brain and knowledge file, and cross-checks artifact rows against
the blobs on disk. `doctor` prints one report: the data directory and its
identity, the schema version and the most this binary supports, free space, the
write-ahead log size, the id high-water mark, and any artifact blob the store
names but the tree is missing. Both refuse while a hub holds the store, and exit
non-zero on any problem.

## Practical limits

The node is sized for one operator and their agents, not a fleet of tenants, so
a handful of things grow with use rather than being capped. Knowing where they
are is how you decide when to prune.

- **Sessions.** No count cap. A session lives until the human prunes it. One
  session brain file warns past 256 MiB and is refused at 1 GiB, and a listing
  returns at most 200 sessions.
- **Events.** A project's feed holds `events_per_project`
  (`HUB_EVENTS_PER_PROJECT`, default one million). A write past it is refused
  and names the cap; promote durable work to the knowledge base or prune the
  feed to make room.
- **Knowledge base.** One page is capped at 1 MiB and one path at 512 bytes. A
  project's knowledge base file warns past 256 MiB and is refused at 1 GiB, so
  its page count follows that file's size rather than a count of its own.
  History and last-write scan the write log newest first, which is the brain
  engine's own call table; the hub wraps it and does not index it, so those
  reads cost more as the log grows. A history request carries at most 200 rows
  and reports the real total.
- **Artifacts.** One blob is capped at 50 MiB. A blob moves on the Tokio
  blocking pool, so a large transfer does not hold an async worker and the hub
  keeps answering other requests while it lands. Many large transfers at once
  are felt in memory (roughly 50 MiB each in flight) and disk bandwidth rather
  than in the workers; past the blocking pool's ceiling they queue.
- **Search.** A filtered search reads at most 5000 rows before it stops, and
  returns at most 100 hits a page. A scope that matches more than the fetch cap
  is reported as truncated rather than scanned whole.

## The feed ceiling

A project's feed is bounded by `events_per_project`. A write past it is refused
with the cap named, so a runaway agent cannot fill the node; pruning a session
removes its lifecycle records and makes room. The ceiling bounds
every agent-surface writer: signals, questions, answers, artifact publish and
update, and the knowledge base's lifecycle signal. Session lifecycle records
and the hub's audit trail are exempt, so a full feed can never refuse
`session_start`; a knowledge base write itself still succeeds, because its feed
signal is best-effort and is dropped when the feed is full.

## Shutdown

On `SIGTERM` or `SIGINT` the hub stops accepting, lets in-flight requests finish
for a bounded window, then checkpoints the store and exits 0. `docker stop` and
a service restart therefore drain rather than cut. This is why the service stop
timeout in [Deploy as a service](/operate/deploy-as-a-service.md) sits above the
ten-second drain.

## You should now see

```sh
curl -sS -H "Authorization: Bearer $HUB_ADMIN_TOKEN" \
  http://127.0.0.1:8080/metrics | head
```

Prometheus text with the counters and the three storage gauges.

## See also

- [Back up and restore](/operate/backup-and-restore.md) - the offline `check`
- [Notifications](/operate/notifications.md) - what a notify failure is counted as
- [Configuration](/reference/configuration.md) - `events_per_project`, `integrity_sample_secs`
