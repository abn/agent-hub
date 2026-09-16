# Vendored: agentfs-sdk

A trimmed, patched copy of the upstream AgentFS Rust SDK, used so the hub
links exactly one database engine version.

## Provenance

- Upstream: `github.com/tursodatabase/agentfs`, path `sdk/rust`.
- Version: `0.6.4`.
- License: MIT, as declared in the upstream crate manifest. Upstream ships no
  license file, so the standard MIT text is included here with the upstream
  project attributed as the holder.

## Changes from upstream

1. `turso` is pinned to the single engine version the hub uses
   (`=0.8.0-pre.11`) instead of the `0.4.4` upstream pins. This is the whole
   reason for vendoring: the hub store needs the engine's native full-text
   search, which the pinned-upstream engine version does not have, and linking
   both would put two engine copies in one binary. The 0.4 to 0.8 API is
   source-compatible for everything this crate uses; no source change was
   needed.
2. Development-only material is removed: `benches/`, `[dev-dependencies]`,
   `[[bench]]`, and `[profile.bench]`. The library and its runtime
   dependencies are unchanged.

## Maintenance

When upstream moves, reapply change 1 and reassess the API drift, then refresh
`src/`. The `make lint/engine` gate fails the build if a second engine version
enters the dependency tree.
