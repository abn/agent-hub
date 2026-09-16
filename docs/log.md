# Documentation update log

This log tracks the evolution of the knowledge base: page additions,
deprecations, and structural refactors. It is deliberately decoupled from
software release notes and the repository changelog.

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
