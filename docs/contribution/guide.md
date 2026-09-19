---
type: Guide
title: Contributor guide
description: Conventions, workflow, and verification for a first change.
tags: [contribution, workflow]
status: draft
---

# Contributor guide

The operational contract lives in `AGENTS.md`. This is the human-readable
how-to for making a change.

## Setup

```
./.agents/bootstrap.sh
make check
```

The bootstrap script installs the git hooks and writes the local assistant
shims. It is idempotent, so re-run it whenever hooks or tooling change.

## Development targets

- `make build`: builds the binary
- `make test`: runs the test suite
- `make clippy`: runs the Rust linter with warnings as errors
- `make fmt`: applies formatting fixes
- `make fmt/check`: fails if formatting differs
- `make docs/check`: validates the docs bundle against OKF v0.2
- `make web/check`: static checks over the PWA assets. The script syntax pass
  needs Node; without it the pass is skipped and says so, and
  `HUB_REQUIRE_BROWSER=1` turns that skip into a failure.
- `make web/a11y`: the headless accessibility audit over the rendered screens,
  at 390px and 1100px in both themes. It walks every screen the router
  registers plus the states a route alone does not show (a dialog, the
  comments drawer, a toast, the public artifact page and its password gate),
  and measures the contrast axe leaves undecided.
- `make web/smoke`: drives the PWA in a browser against a seeded hub
- `make check`: the full gate: hooks, linter, formatting, and tests

## The workflow

1. **State the scope in one sentence** before starting: the unit of work, the
   target, and the outcome. Anything outside it is out of scope.
2. **Work in a dedicated worktree** with a conventional branch name (`feat/`,
   `fix/`, `docs/`, `chore/`, `refactor/`), rebased on the latest `main`.
3. **Keep the change small.** One logical unit. Fix up or amend into the
   owning commit rather than stacking fix commits on an active branch.
4. **Write the tests and update the wiki.** A behaviour change that leaves the
   docs stale is not finished.
5. **Run `make check`** and keep the output. A change is done when the gate
   passes and you have the captured output to show for it.

## Conventions

- **Commits** follow Conventional Commits, summary first, no trailers. Stage
  explicit paths, never `git add -A`. The commit-message hook enforces this.
- **Verification is automated.** `pre-commit` owns file hygiene, commit
  message shape, and the editorial and privacy patterns. `make check` runs
  the same hooks CI runs. Do not hand-polish what a tool enforces.
- **Respect the invariants.** One engine, wrap AgentFS rather than
  reimplementing it, the wrapper as the single writer per session file, no
  automatic expiry, no chat, and engine-native search. `AGENTS.md` is the
  authoritative list.
- **Vendored assets are integrity-checked.** Third-party scripts under
  `web/vendor/` must be registered in `web/vendor/MANIFEST.json` with their
  upstream URL, license, banner-derived version, and SHA-256 hash.
- **Committed files are public-ready.** Committed code, tests, documentation,
  and commit messages must never reference internal tracking, task or ticket
  identifiers, wave or lane names, or scratch area paths. Internal process
  stays in the gitignored scratch area.
- **Docs are public-ready by default.** `docs/` is an OKF v0.2 bundle: no
  internal names, hostnames, absolute paths, tokens, or task identifiers.
  Links are relative or rooted at the bundle; no `file://` and nothing
  pointing outside `docs/`. Wiki changes are recorded in [the log](../log.md).
- **The scratch area is local.** Working notes, task breakdowns, and the
  internal progress log live in the gitignored scratch area and are never
  committed.

## Related

- [Maintainer guide](maintainers.md) - the review gate
- `AGENTS.md` - the authoritative operational contract
