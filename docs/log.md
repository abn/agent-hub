# Documentation update log

This log tracks the evolution of the knowledge base: page additions,
deprecations, and structural refactors. It is deliberately decoupled from
software release notes and the repository changelog.

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
* **Note**: The bundle deliberately has no usage or reference section yet.
  The hub does not run, so a page describing how to run it would be fiction.
  Both sections open when the first runnable binary does.
* **Note**: Every architecture page describes an intended design, not shipped
  behaviour, and says so. Pages are rewritten against the code as the code
  lands.
