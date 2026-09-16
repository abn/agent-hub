# Documentation update log

This log tracks the evolution of the knowledge base: page additions,
deprecations, and structural refactors. It is deliberately decoupled from
software release notes and the repository changelog.

## 2026-09-16, feed surface

* **Creation**: The MCP server exposes the feed: `signal_append` writes an
  event, and `feed_read` pages a project feed with `next_since` and
  `next_before` cursors. The streamable HTTP transport requires a bearer token.
* **Creation**: Added the REST feed route `GET /api/v1/projects/:id/feed`,
  with RFC 9457 problem details.
* **Update**: Recorded the event store design: append-only events with ULID
  ids, cursor paging, idempotency keys, payload limits, and write-through
  indexing into the search corpus.
* **Update**: Corrected the stale "not implemented yet" notes on the overview,
  the architecture index, the agent surface, and the human surface.

## 2026-09-16, implementation and packaging

* **Creation**: Opened the [usage](usage/index.md) section with the
  [quickstart](usage/quickstart.md), covering the binary build, environment
  configuration, the health probes, and running with the container and compose
  file. The hub is an early work in progress and the page says so.
* **Update**: Recorded the container packaging: a multi-stage `Containerfile`,
  a `deploy/compose.yaml`, and a `.dockerignore`.
* **Update**: Corrected the root [index](index.md) and this log, which still
  said the bundle had no usage section and that a runnable binary did not
  exist.

## 2026-09-16, grounding and design handoff

* **Update**: Amended [0003](adr/0003-wrap-agentfs-per-session.md) to record
  that AgentFS is embedded as a crate and vendored, [0004](adr/0004-manual-pruning-in-v1.md)
  that prune is reversible, and [0006](adr/0006-engine-native-search.md) that
  engine-native search is confirmed and centralised in the hub store.
* **Creation**: Added [0010](adr/0010-one-pinned-engine.md) one pinned engine,
  [0011](adr/0011-mcp-primary-a2a-deferred.md) MCP primary with A2A deferred,
  [0012](adr/0012-agent-identity-and-trust.md) agent identity and trust,
  [0013](adr/0013-per-session-serialization.md) per-session serialization,
  [0014](adr/0014-optional-embedded-tailnet.md) optional embedded tailnet, and
  [0015](adr/0015-human-interface-foundation.md) the design foundation.
* **Update**: Reworked [the data model](architecture/data-model.md) with agent
  identity and trust tables, the search corpus, the artifact envelope, the
  closed kind set, and reversible pruning. Updated [components](architecture/components.md),
  [agent surface](architecture/agent-surface.md), and [human surface](architecture/human-surface.md)
  to match.
* **Creation**: Added [the human interface](design/human-interface.md)
  describing the design tokens, screens, alert hierarchy, accessibility gate,
  and copy rules.
* **Update**: Corrected the [overview](overview.md) to place artifact blobs on
  the data volume and note the centralised engine-native search index.

## 2026-09-16

* **Creation**: Opened the bundle with the operational baseline. Added the
  root [index](index.md), this log, the [overview](overview.md), the design
  section ([goals](design/goals.md), [terminology](design/terminology.md)),
  the architecture section ([components](architecture/components.md), [data
  model](architecture/data-model.md), [agent surface](architecture/agent-surface.md),
  [human surface](architecture/human-surface.md)), nine [decision
  records](adr/index.md), and the contribution section
  ([guide](contribution/guide.md), [maintainer guide](contribution/maintainers.md)).
* **Note**: At the baseline the bundle had no usage or reference section,
  because the hub did not run and a page describing how to run it would have
  been fiction. The usage section opened once the first runnable binary
  existed.
* **Note**: Every architecture page describes an intended design, not shipped
  behaviour, and says so. Pages are rewritten against the code as the code
  lands.
