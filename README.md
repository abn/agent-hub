# Agent Hub

A local-first operations layer for AI agents and their humans, shipped as one
lean binary or container on a homelab or NAS node. The node is the cloud.

Agent Hub gives a fleet of agents, spread across machines on a LAN or tailnet,
three things they otherwise lose and one calm place for the human to watch:

- **Session brains.** Every agent session gets a server-side, session-scoped
  store whose life is bound to the session, not to the client context window.
  It survives compaction and resume of the same named session.
- **A global feed.** "What happened in project X" is a first-class, time
  ordered, addressable query that agents read on startup and humans read as a
  feed.
- **A home for the human.** A global inbox for finished work and approval
  requests, artifact hosting with browser-side encryption and password
  sharing, and search, all reachable from an installable PWA.

Agents reach the hub over MCP. Humans reach it over the PWA.

## Status

The repository holds the operational baseline and a minimal binary skeleton.
The hub itself is not implemented yet. The public design lives in the wiki
under [`docs/`](docs/index.md); the working specification is held outside the
committed tree.

## Building

```
./.agents/bootstrap.sh
make check
```

The bootstrap script installs the git hooks and writes the local assistant
shims. It is idempotent and safe to re-run. `make check` is the quality gate
that hooks and CI both reuse.

## Documentation

The wiki lives in [`docs/`](docs/index.md): the
[overview](docs/overview.md), the [design](docs/design/index.md), the
[architecture](docs/architecture/index.md), and the [decision
records](docs/adr/index.md).

## Contributing

Start with the [contributor guide](docs/contribution/guide.md) and
[CONTRIBUTING.md](CONTRIBUTING.md). `AGENTS.md` is the operational contract
for humans and agents alike.

## Licence

MIT. See [LICENSE](LICENSE).
