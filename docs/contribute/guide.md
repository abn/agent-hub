---
type: Guide
title: Contributor guide
description: Set up the toolchain, make a change, and run every gate
tags: [contributor, contribution]
status: stable
---

# Contributor guide

By the end you will have the hooks installed, the client toolchain ready, and
one change through the full gate. This is the front door to working on the hub:
`AGENTS.md` holds the contract, [Testing](/contribute/testing.md) says what each
gate proves, and [Documentation](/contribute/documentation.md) is the docs
standard.

## Before you start

| Thing | Why |
|---|---|
| `pre-commit` | `make setup` installs the git hooks through it |
| A Rust toolchain, 1.97 or newer | the hub compiles from source |
| `clang` and `cmake` | the pinned engine compiles C |
| Node | the client unit, stylesheet, and type targets |
| A browser | the Playwright checks drive it |
| The `okf` validator | `make docs/check` validates the docs bundle |

[Install and run the hub](/start/install.md) covers the Rust and C toolchains in
full, and [Documentation](/contribute/documentation.md) installs the validator.

## Set up

```sh
make setup
make check
```

`make setup` installs the git hooks, creates the scratch area, and writes the
local assistant shims. It is idempotent, so re-run it whenever the hooks or the
tooling change.

`make check` is the full gate: hooks, linter, formatting, the docs bundle, the
client checks, and the tests. Keep its output. A change is done when the gate
passes and you have the captured output to show for it.

## You should now see

`make setup` ends by printing that the hooks are installed, the scratch area is
ready, and the shims are written. `make check` ends with `check: ok`.

## Development targets

The Makefile is the single automation entrypoint; bare `make` lists every
target. The ones a contributor reaches for most:

Build, lint, and format:

| Target | What it does |
|---|---|
| `make build` | builds the debug binary |
| `make test` | runs the Rust suite, through `cargo-nextest` when it is installed |
| `make test/shard N=1 M=3` | runs one slice of the Rust suite, for CI |
| `make clippy` | runs the Rust linter with warnings as errors |
| `make fmt` | applies the formatting fixes |
| `make fmt/check` | fails if formatting differs from `rustfmt` output |
| `make lint` | runs every declarative hook against all files |
| `make lint/staged` | runs the hooks on the staged files only, for the inner loop |
| `make lint/engine` | fails unless exactly one engine version is linked |
| `make docs/check` | validates the docs bundle against OKF v0.2 |
| `make net/check` | tests the tailnet path in a build without the feature |
| `make serve/check` | compiles the serve-only build the container uses |

The gates:

| Target | What it does |
|---|---|
| `make gate/quick` | the fast inner loop: format, lint, and client units |
| `make gate` | everything the gate runs but the browser checks |
| `make check` | the full gate, browser checks included |

Client and browser:

| Target | What it does |
|---|---|
| `make web/units` | Vitest over the pure client functions and the PWA static checks |
| `make web/styles` | the stylesheet gate, over a `css-tree` parse |
| `make web/types` | type-checks the client sources with `tsc --noEmit` |
| `make web/e2e` | the Playwright behaviour checks and the axe audit |
| `make web/crypto` | the artifact encryption round-trip self-test |
| `make e2e/shard SHARD=1/2` | runs one shard of the browser checks |
| `make e2e/baselines` | rewrites the changed screenshot baselines, on the CI runner |

`make web/types` is not part of `make check` yet; [Testing](/contribute/testing.md)
says what it has to clear before it can be. `make container/config` validates the
compose file and `make container/build` builds the image, both by hand rather
than in the gate. `make clean` removes the build artefacts, and
`make hooks/update` moves the pinned hook revisions.

## The client toolchain

The PWA is served as vanilla ES modules with no bundler, so there is nothing to
compile and nothing to bundle. The Node tree is development tooling only: the
dependencies are devDependencies in `package.json`, the lockfile is committed,
`node_modules/` is ignored, and the shipped binary and container never read it.

| Concern | Tool | Target |
|---|---|---|
| Pure client logic | Vitest | `make web/units` |
| PWA build and assets | Vitest reading the files | `make web/units` |
| Stylesheet rules and tokens | a `css-tree` AST walk under Vitest | `make web/styles` |
| Client types | `tsc --noEmit` over `checkJs` | `make web/types` |
| Browser behaviour and layout | Playwright Test | `make web/e2e` |
| Accessibility | `@axe-core/playwright` under Playwright Test | `make web/e2e` |

[Testing](/contribute/testing.md) has the same matrix with what each concern
proves and how to write a check.

## The workflow

1. **State the scope in one sentence**: the unit of work, the target, and the
   outcome. Anything outside it is out of scope.
2. **Work in a dedicated worktree** with a conventional branch name (`feat/`,
   `fix/`, `docs/`, `chore/`, `refactor/`), rebased on the latest `main`.
3. **Keep the change small.** One logical unit. Fix up or amend into the owning
   commit rather than stacking fix commits on an active branch.
4. **Write the tests and update the wiki.** A behaviour change that leaves the
   docs stale is not finished.
5. **Run `make check`** and keep the output. The gate passing, with the captured
   output to show for it, is what done means.

## Conventions

- **Commits** follow Conventional Commits, summary first, no trailers. Stage
  explicit paths, never `git add -A`. The commit-message hook enforces this.
- **Verification is automated.** `pre-commit` owns file hygiene, commit-message
  shape, and the editorial and privacy patterns, and `make check` runs the same
  hooks CI runs. Do not hand-polish what a tool enforces.
- **Respect the invariants.** One engine, wrap AgentFS rather than reimplement
  it, the wrapper as the single writer per session file, no automatic expiry, no
  chat, and engine-native search. `AGENTS.md` is the authoritative list.
- **Tests keep their files under the build tree.** A throwaway directory goes
  under `target/tmp`, never the system temp directory, which is often memory and
  keeps what a killed run leaves behind.
- **Vendored assets are integrity-checked.** Third-party scripts under
  `web/vendor/` must be registered in `web/vendor/MANIFEST.json` with their
  upstream URL, license, banner-derived version, and SHA-256 hash.
- **Committed files are public-ready.** Committed code, tests, documentation,
  and commit messages never reference internal tracking, task or ticket
  identifiers, wave or lane names, or scratch area paths.
- **Docs are public-ready by default.** `docs/` is an OKF v0.2 bundle: no
  internal names, hostnames, absolute paths, tokens, or task identifiers, and no
  link outside the bundle. [Documentation](/contribute/documentation.md) has the
  page and index rules.
- **The scratch area is local.** Working notes, task breakdowns, and the internal
  progress log live in the gitignored scratch area and are never committed.

## See also

- [Testing](/contribute/testing.md) - the test matrix and the gates
- [Documentation](/contribute/documentation.md) - the docs standard
- [Maintainer guide](/contribute/maintainers.md) - the review gate
- [Install and run the hub](/start/install.md) - the toolchains in full
