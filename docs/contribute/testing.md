---
type: Reference
title: Testing
description: Where each test concern lives and what each gate proves
tags: [contributor, testing]
status: stable
---

# Testing

The hub and the interface are proved with standard tools rather than checks
that pattern-match source text: a check holds a rendered value, a behaviour, or
a parsed property, never a substring of a file. This page says which tool holds
which concern and what each gate proves. The targets themselves are in the
[Contributor guide](/contribute/guide.md), and the design contract the
accessibility gate holds is in [Human interface](/design/human-interface.md).

## The matrix

| Concern | Tool | Target |
|---|---|---|
| Rust behaviour | `cargo test`, through `cargo-nextest` when installed | `make test` |
| Rust lint | `cargo clippy`, warnings as errors | `make clippy` |
| Pure client logic | Vitest | `make web/units` |
| PWA build and assets | Vitest reading the files | `make web/units` |
| Stylesheet rules and tokens | a `css-tree` AST walk under Vitest | `make web/styles` |
| Client types | `tsc --noEmit` over `checkJs` | `make web/types` |
| Browser behaviour and layout | Playwright Test | `make web/e2e` |
| Accessibility | `@axe-core/playwright` under Playwright Test | `make web/e2e` |
| Artifact encryption | a Node self-test | `make web/crypto` |
| Mutation coverage | `cargo-mutants`, Stryker | run directly |

`make check` runs the gate and the browser checks; `make gate` runs everything
but the browser checks; `make gate/quick` is the fast inner loop.

## Units

`make web/units` runs Vitest over the pure client functions, imported from
`web/` where they live, and the PWA's static checks. It is Node only, no
browser: jsdom supplies the document and the media query a module reads at
import time, so a test and the page cannot drift apart. A rename no longer fails
one of these, and a branch that is dropped does.

The run also holds the frontmatter module's corpus, property, and differential
tests. The differential layer answers generated input with the Rust reference
through cargo, and it skips rather than fails when the toolchain is absent.

The build and asset checks read the files and assert parsed or structural
properties: the required asset set, the licence markers, the external-origin and
emoji ban, the shell's accessibility basics, the service-worker registration and
its server-stamped placeholders, the parse of every first-party script, the
served asset table, the glyph set, and the vendor manifest.

## The stylesheet gate

`make web/styles` parses `web/*.css` with `css-tree` and asserts parsed
declarations, so a check names the property and the value it means rather than a
substring of a file. The token-only colour rule, the token contrast pairs, the
one palette, the 12px type floor, the 44px target, the transition bound, reduced
motion, token resolution, and the focus ring are all read this way.

Two consequences follow. A rule is found by the declaration it carries, so
renaming a selector or moving a rule to the end of the file keeps passing. And
where the design waives a rule, the gate reads the condition that earns the
waiver rather than trusting its name, so the waiver cannot outlive what
justified it.

## The type gate

`make web/types` type-checks the client sources with `tsc --noEmit` over
`checkJs`. It is not in `make check` yet: it lists the errors the JSDoc phase
has to clear, and a target that cannot pass is not a gate. As of 2026-10-05 it
reports 273 errors over 31 files. Most are one class, a DOM property read off an
element a `querySelector` returned, which wants a JSDoc type on the lookup
rather than a cast at every use. It joins the gate when that count reaches zero.

## Browser behaviour

`make web/e2e` runs the browser checks on Playwright Test. Two projects, one per
width (1440x900 and 390x844), each against its own seeded hub. They hold the
list filter, the single-key verbs, the search key, and the filter chips. It
needs Node and a browser; without either the target says so and passes, and
`HUB_REQUIRE_BROWSER=1` turns that skip into a failure.

Three rules for writing one:

- **Wait on a condition, not on a number of milliseconds.** `expect(locator)
  .toBeVisible()` and `.toHaveText()` retry until the page says so, so a slow
  paint is waited out and a missing one fails with a message. A fixed
  `wait_for_timeout` is the thing this phase removed; do not add one back.
- **Find what a reader reaches by role or label**, the way
  `getByRole("searchbox", { name: "Filter inbox" })` does, and fall back to a
  class only where the markup has no accessible name to ask for, such as a row's
  position in a group. A locator that names a class is a screen rule, not a
  behavioural gate.
- **A hub per project, seeded by the Python harness.** `e2e/hub.mjs` starts one
  throwaway hub per project through `e2e/hub-bridge.py`, which reuses the seeded
  hub's `seed()`, and writes a descriptor the tests read. So the fixture strings
  a check names are the harness's constants, and a project's tests do not move
  another project's data. `workers: 1` keeps a project's own tests in order,
  which matters because the hub draws Home's unread dots from what is still
  above the reader's cursor.

The specs are `.spec.mjs`, so `npx playwright test` finds them, and the
throwaway data directory and the failure artefacts land under `target/tmp`.

## Accessibility

The audit is `e2e/a11y.spec.mjs`, on `@axe-core/playwright` under Playwright
Test. It walks the main screens and asserts no WCAG A or AA violation. It
replaced the standalone `a11y.py`, so it runs in the same browser pass as the
behaviour checks rather than as a separate step. [Human interface](/design/human-interface.md)
is the design contract behind it.

## Visual baselines

A `toHaveScreenshot` baseline matches only the browser build and the fonts it
was taken with, so a baseline made on your machine fails in CI. CI makes them.
`make e2e/baselines` runs the two e2e shards in turn, as the e2e job does, and
rewrites every baseline that changed.

1. Push your branch, then run the `visual-baselines` workflow on it, from the
   Actions tab or with `gh workflow run visual-baselines --ref <branch>`. It
   refuses the default branch.
2. When a baseline changed, the workflow pushes a
   `test(e2e): refresh visual baselines` commit to the branch. A push made with
   the workflow's token starts no CI. If the branch moved while the run was
   going, it pushes nothing and asks you to run it again.
3. Pull that commit, fold it into the commit that changed the screen, and
   force-push with `git push --force-with-lease`. That push runs CI against the
   new baselines.

## Mutation testing

Coverage of the tests themselves is measured with mutation testing:
`cargo-mutants` for the Rust tree and Stryker for the client. A check that
cannot be shown failing on a real defect and passing after a rename is not a
gate, so a surviving mutant is a missing assertion rather than a style point.
Neither tool has a `make` target; run it directly.

## Host safety

Every test keeps its throwaway files under the build tree, never the system
temporary directory, which is often memory and keeps what a killed run leaves
behind. A Rust test takes its throwaway directory from the shared `TempDir` in
`tests/common`; a browser check takes one from `scratch_root()` in the harness.
Both sit under `target/tmp` and are removed when the test ends. A pre-commit
hook rejects the system temp directory in `src/`, `tests/`, and the scripts, and
`cargo clean` empties what a killed run left behind.

## See also

- [Contributor guide](/contribute/guide.md) - the targets and the workflow
- [Documentation](/contribute/documentation.md) - the docs gate
- [Human interface](/design/human-interface.md) - the interface build gates
