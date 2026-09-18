# AGENTS.md

Operational contract for humans and agents working in this repository.

## Project

Agent Hub is a local-first, MCP-ready operations layer for a fleet of AI
agents and their human, shipped as one lean binary or container on a homelab
or NAS node. The node is the cloud: agents report in over LAN or tailnet, and
the human watches everything from an installable PWA.

The working specification is held in the gitignored scratch area and is never
committed. The public design lives in the wiki under `docs/` and describes the
shipped behaviour.

## Invariants

- **The local hub is the cloud.** No vendor cloud, and no brain off the node:
  the node holds every brain and agents reach it over LAN or tailnet. The
  homelab or NAS node is the central solution.
- **One engine everywhere.** The hub event store, per-session AgentFS files,
  and artifact storage all run on the Turso Database Rust engine. No libSQL,
  no SQLite C bindings, no two-engine split.
- **Real AgentFS, server-side, per session.** The hub wraps AgentFS; it never
  reimplements it. One AgentFS file per session holds KV state, an
  append-only audit log, and a POSIX-like filesystem. Agents never touch the
  file directly: they speak MCP, and the wrapper is the single writer per
  session file.
- **Session life is session-bound.** A brain survives same-session compaction
  and resume of the same named session, and is garbage-collected on prune.
  Durable knowledge leaves the brain only by explicit promotion to a feed
  event, an artifact, or a memory write.
- **The human is the garbage collector in v1.** No automatic expiry ships.
  Retention is a per-layer concept, and the schema carries the timestamps and
  hints the later layers need.
- **The human surface is an installable PWA.** Responsive and mobile-first,
  equally good on desktop. Interaction is read, answer, approve, and prune,
  under async mailbox semantics. No real-time chat with agents in v1.
- **Search is engine-native.** Full-text search over the same engine backs
  both the MCP tools and the PWA. No separate search service.
- **Lean by construction.** Single binary, single engine, no microservices,
  no CRDTs, no custom distributed consensus.
- **Artifact privacy.** Protected artifacts are encrypted in the browser
  before upload; the server stores ciphertext and never sees plaintext.
  Nothing leaves the machine, in a shared artifact or a bug report, without
  the redaction-by-default rule from the global standards.
- **No sudo** in this repository unless a human explicitly requests
  elevation.
- **No AI slop.** No em-dashes, no marketing fluff, no filler prose, no
  comments that restate the code. Code and docs read like a human wrote them.
- **No emoji.** Anywhere: interface, copy, mocks, or docs. Icons are inline
  SVG line glyphs or typographic marks.
- **Accessibility is a build gate.** WCAG AA in both themes, a 12px UI text
  floor, 44px tap targets, a visible focus ring, a full keyboard path, and no
  meaning carried by colour alone. Reduced motion is honoured.
- **No internal process leaks.** Committed files and assets never reference
  internal process or tracking identifiers, task or ticket numbers, agent-work
  references, milestone identifiers, or scratch paths. Internal and agent-work
  tracking belongs in the scratch area only. A hook rejects the obvious
  identifier patterns; review covers the rest.
- **Always-public-ready docs.** `docs/` is an OKF v0.2 bundle. No internal
  names, codenames, hostnames, absolute paths, tokens, or task identifiers.
- **Tightly scoped changes.** Every change is the smallest clean change that
  satisfies one stated scope: unit of work, target, outcome. No opportunistic
  refactoring or feature creep.

## Automation and conventions

- **Prefer automation over manual conformance.** `pre-commit` and `make check`
  own style, conventions, and quality gates. Do not hand-polish what a tool
  can enforce.
- **Tool-specific assets stay out of the repository.** Assistant shims and the
  scratch area under `.agents/brain/` are git-ignored. `.agents/bootstrap.sh`
  regenerates the shims; it is idempotent and safe to re-run.
- **Makefile** is the single automation entrypoint. Bare `make` shows the
  targets. `make check` is the gate that hooks and CI both reuse.
- **Commits** follow Conventional Commits, summary first, no trailers (no
  Co-Authored-By, no Signed-off-by). Stage explicit paths, never `git add -A`.
- **Changes** happen in a dedicated worktree with a conventional branch name
  (`feat/`, `fix/`, `docs/`, `chore/`, `refactor/`), rebased on latest `main`.
- **Clean history.** Fix up or amend into the owning commit on active
  branches rather than stacking fix commits.
- **Docs move with the change.** A behaviour change updates the wiki and its
  log. The internal progress log is updated separately and never in the wiki.

## Verification

A change is not done until `make check` passes and the relevant tests are
green, with real captured output. Wiki changes are recorded in `docs/log.md`,
and the bundle is validated by `make docs/check`.

## Contributor guide

See the [contributor guide](docs/contribution/guide.md) and, for maintainers,
the [maintainer guide](docs/contribution/maintainers.md).
