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
make setup
make check
```

`make setup` installs the git hooks, creates the scratch area, and writes the
local assistant shims. It is idempotent, so re-run it whenever hooks or tooling
change.

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
- `make web/units`: unit tests over the pure client functions, imported from
  `web/` where they live. Node only, no browser.
- `make web/styles`: the stylesheet's design contract, held against a `css-tree`
  parse of `web/*.css` rather than against the source text. It reads the
  token-only colour rule, the 12px type floor, the 44px target, the transition
  bound, reduced motion, token resolution and the focus ring as parsed
  declarations, so a renamed selector passes and a changed value fails. Node
  only, no browser.
- `make web/types`: type-checks the client sources with `tsc --noEmit` over
  `checkJs`. It is not in `make check` yet: it lists the errors the JSDoc phase
  has to clear, and a target that cannot pass is not a gate. As of 2026-10-05
  it reports 273 errors over 31 files, so the JSDoc phase has that many to
  clear before it becomes a gate. Most are one class: a DOM property read off
  an element a `querySelector` returned, which wants a JSDoc type on the
  lookup rather than a cast at every use.
- `make web/a11y`: the headless accessibility audit over the rendered screens,
  at 390px and 1100px in both themes. It walks every screen the router
  registers plus the states a route alone does not show (a dialog, the
  comments drawer, a toast, the public artifact page and its password gate),
  and measures the contrast axe leaves undecided.
- `make web/e2e`: the browser behaviour checks, on Playwright Test. Two
  projects, one per width (1440x900 and 390x844), each against its own seeded
  hub. It holds the list filter, the single-key verbs, the search key and the
  filter chips. Needs Node and a browser; without either the target says so and
  passes, and `HUB_REQUIRE_BROWSER=1` turns that skip into a failure.
- `make check`: the full gate: hooks, linter, formatting, and tests

## The client toolchain

The PWA is served as vanilla ES modules with no bundler, so there is nothing to
compile and nothing to bundle. The Node tree is development tooling only, held
in `package.json` as devDependencies with the lockfile committed and
`node_modules/` ignored. The shipped binary and container never read it.

| Concern | Tool | Where |
|---|---|---|
| Pure client logic | Vitest | `make web/units` |
| Stylesheet rules and tokens | `css-tree` AST walk under Vitest | `make web/styles` |
| Client types | `tsc --noEmit` over `checkJs` | `make web/types` |
| Browser behaviour and layout | Playwright Test | `make web/e2e` |
| Accessibility | `@axe-core/playwright` | not wired yet |

The unit tests import the modules from `web/` directly, so a test and the page
cannot drift apart, and jsdom supplies the document and the media query the
modules read at import time. A rename no longer fails a test, and a branch that
is dropped does.

The browser checks live in `e2e/` and run on Playwright Test. Three things are
worth knowing before writing one:

- **Wait on a condition, not on a number of milliseconds.** `expect(locator)
  .toBeVisible()` and `.toHaveText()` retry until the page says so, so a slow
  paint is waited out and a missing one fails with a message. A fixed
  `wait_for_timeout` is the thing this phase removed; do not add one back.
- **Find what a reader reaches by role or label,** the way
  `getByRole("searchbox", { name: "Filter inbox" })` does, and fall back to a
  class only where the markup has no accessible name to ask for (a row's
  position in a group). A locator that names a class is a screen rule, not a
  behavioural gate.
- **A hub per project, seeded by the Python harness.** `e2e/hub.mjs` starts one
  throwaway hub per project through `e2e/hub-bridge.py`, which reuses
  `hub_harness.seed()`, and writes a descriptor the tests read. So the fixture
  strings a check names are the harness's constants, and a project's tests do not
  move another project's data. `workers: 1` keeps a project's own tests in order,
  which matters because the hub draws Home's unread dots from what is still above
  the reader's cursor.

The specs are `.spec.mjs`, so `npx playwright test` finds them, and the
throwaway data directory and the failure artefacts land under `target/tmp`.

The stylesheet gate reads the same way: it parses `web/*.css` with `css-tree`
and asserts parsed declarations, so a check names the property and the value it
means rather than a substring of a file. Two consequences are worth knowing
before writing one:

- **A selector is a convenience, not the thing matched.** A rule is found by the
  declaration it carries, so renaming a class or moving a rule to the end of the
  file keeps passing. An assertion that has to name a screen's own class is
  telling you it is a screen rule, not a design gate, and belongs in the next
  porting phase rather than here.
- **A waiver is held, not assumed.** Where the design waives a rule (the 9px
  card preview, a scrim over content the hub does not draw), the gate reads the
  condition that earns the waiver rather than trusting the name, so the waiver
  cannot outlive the thing that justified it.

`check-web.py` still holds a regex version of the type floor, the tap target
and the token gate. The two now overlap; the regex versions are the next
phase's deletion, and a new gate should not be added in both places.

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
- **Tests keep their files under the build tree.** A test's throwaway
  directory comes from the shared `TempDir` in `tests/common`, and a check's
  from `scratch_root()` in the browser harness; both sit under `target/tmp`
  and are removed when the test ends. The system temp directory is often
  memory, and a run that is killed leaves its files there for good, so a hook
  rejects it in `src/`, `tests/` and the scripts. `cargo clean` empties what
  a killed run left behind.
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
