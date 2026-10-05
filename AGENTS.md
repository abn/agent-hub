# AGENTS.md

Operational contract for humans and agents working in this repository.

## Project

Agent Hub is a local-first, MCP-ready operations layer for a fleet of AI
agents and their human, shipped as one lean binary or container on a homelab
or NAS node. The node is the cloud: agents report in over LAN or tailnet, and
the human watches everything from an installable PWA.

The working specification is held in the gitignored scratch area and is never
committed. The public design lives in the wiki under `docs/` and describes the
shipped behaviour. The trust posture and the access rules are stated once in
[the operating model](docs/architecture/model.md); read it before re-deriving
any access rule.

## Invariants

- **Open by default; boundaries guard against mistakes, not tenants.** One
  operator, their agents, one node. Every agent token reads and writes every
  ordinary project and its own personal space. A confidential project needs a
  grant, and a grant is access or no access, with no read or write levels. The
  admin boundary is privilege, not use: prune and undo, token and grant
  administration, declassifying a project, and deleting a project. Multi-tenant
  isolation and defence against a caller that already holds a token are not
  goals. The [operating model](docs/architecture/model.md) is the one statement
  of this.
- **The local hub is the cloud.** No vendor cloud, and no brain off the node:
  the node holds every brain and agents reach it over LAN or tailnet. The
  homelab or NAS node is the central solution.
- **One engine everywhere.** The hub event store, per-session AgentFS files,
  and artifact storage all run on the Turso Database Rust engine. No libSQL,
  no SQLite C bindings, no two-engine split.
- **Real AgentFS, server-side, per session and per project.** The hub wraps
  AgentFS; it never reimplements it. One AgentFS file per session holds that
  session's KV state, append-only audit log, and POSIX-like filesystem. One
  AgentFS file per project holds the project knowledge base, shared by every
  agent with project write. Agents never touch a file directly: they speak
  MCP, and the wrapper is the single writer per file.
- **Embedded stdio is supported with isolation.** Embedded stdio mode runs
  standalone against the local data directory as the local admin when no
  `HUB_URL` is configured. Two processes must never quietly share one store:
  running embedded stdio against a data directory that a serving hub already
  holds fails at startup, and callers reach the running hub through the proxy
  instead. Embedded mode is for local administrative operations and isolated
  tool harnesses, not concurrent access to an active hub store.
- **Session life is session-bound.** A session brain belongs to the agent that
  started it. It survives same-session compaction and resume of the same named
  session by its owner, can be adopted or forked by another agent, and is
  garbage-collected on prune. The project knowledge base is outside session
  life: prune never touches it. Durable knowledge leaves a session brain only
  by explicit promotion to a feed event, an artifact, or the project knowledge
  base.
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
- **The frame does not move, on a desktop.** Every pane reserves a 52px header
  and a 40px control row, in that order, whether or not it has content for
  them. One gutter per pane, a fixed glyph column in every list, and prose
  left-aligned and capped at 640. Panes change width; the reader's place does
  not. **On a phone the frame is one header plus one tools row**: the header is
  76px at rest and 52px once scrolled, the tools row is a sticky 44px, and the
  48px leading slot and the title's left edge hold in both states. A phone
  reader's place is their scroll position, so the phone reserves nothing. Round
  12's content rules (a list, a summary, a settings row, a storage breakdown)
  hold at both widths; its chrome rules (the collapsing header, the tools row,
  the tab bar, More) are phone-only.
- **One surface per thing.** An object is read in one place at a time: the
  aside on a fine pointer, one sheet on a coarse pointer. No code path mounts
  both, and there is no comment modal on the desktop.
- **An aside exists only where something is read against the stage** (anchored
  comments, a brain `kv` value). Correspondence, a feed or inbox thread, is the
  stage. An index exists only where items are opened one at a time; Settings
  has none.
- **A copy control is a glyph on the row that owns the string**, never a text
  button and never floating over content. Words belong in menus. `--danger` is
  reserved for deletion.
- **`DESIGN.md` is the design contract.** It states the rules, the tokens and
  the build gates the interface is held to, and records where the build
  deviates from the designer's handoff and why. A UI change that departs from
  it changes `DESIGN.md` in the same commit, or it is not done.
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
- **Tests assert behaviour, not source text.** A check holds a rendered value, a
  behaviour, or a parsed property of the stylesheet, never a substring of a
  source file. Use the standard tool for the concern: Playwright Test with role
  locators and auto-waiting for browser behaviour, `toHaveScreenshot` for
  visuals, `@axe-core/playwright` for accessibility, Vitest for client logic,
  Stylelint and `css-tree` for the stylesheet and its tokens, TypeScript and
  `tsc --noEmit` for types, and mutation testing (`cargo-mutants`, Stryker) for
  coverage. A check that cannot be shown failing on a real defect and passing
  after a rename is not a gate: rewrite it with the tool or remove it.
  [ADR 0023](docs/adr/0023-prove-the-interface-with-standard-tools.md) is the
  record, and the toolchain is dev-only: no bundler, and no runtime dependency
  in the shipped binary or container.
- **Tool-specific assets stay out of the repository.** Assistant shims and the
  scratch area under `.agents/brain/` are git-ignored. `.agents/bootstrap.sh`
  regenerates the shims; it is idempotent and safe to re-run. **One exception:**
  reusable project skills live under `.agents/skills/` and are committed, since
  they are part of how the project is maintained, not a tool's local state.
- **The installable skill is a product artifact.** `skills/agent-hub/` is the
  progressive `agent-hub` skill that users and agents install with
  `npx skills add abn/agent-hub`. Its `bootstrap.md` is the canonical setup
  document, and the binary embeds it and serves it at `GET /bootstrap/SKILL.md`.
  This is distinct from the project skills under `.agents/skills/`.
- **The wiki carries product screenshots.** `docs/assets/screens/` holds the
  main feature screens at both widths in both themes, captured with dummy data
  from a seeded scratch hub by `.agents/scripts/wiki_screens.py`; the skill is
  `.agents/skills/capture-wiki-screenshots/`. Re-run and commit the captures
  when a screen changes, in the same commit as the change.
- **Makefile** is the single automation entrypoint. Bare `make` shows the
  targets. `make check` is the gate that hooks and CI both reuse.
- **Commits** follow Conventional Commits, summary first, no trailers (no
  Co-Authored-By, no Signed-off-by). Stage explicit paths, never `git add -A`.
- **Changes** happen in a dedicated worktree with a conventional branch name
  (`feat/`, `fix/`, `docs/`, `chore/`, `refactor/`), rebased on latest `main`.
- **Clean history.** Fix up or amend into the owning commit on active
  branches rather than stacking fix commits.
- **Tests and checks keep their files under the build tree.** A throwaway
  directory goes under `target/tmp` (the shared `TempDir` in `tests/common`,
  `scratch_root()` in the browser harness), never the system temp directory,
  which is often memory and keeps what a killed run leaves behind. A hook
  rejects the system temp directory in `src/`, `tests/` and the scripts.
- **Docs move with the change.** A behaviour change updates the wiki and its
  log. The internal progress log is updated separately and never in the wiki.

## Verification

A change is not done until `make check` passes and the relevant tests are
green, with real captured output. Wiki changes are recorded in `docs/log.md`,
and the bundle is validated by `make docs/check`.

## Contributor guide

See the [contributor guide](docs/contribution/guide.md) and, for maintainers,
the [maintainer guide](docs/contribution/maintainers.md).
