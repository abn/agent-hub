---
type: Decision
title: Prove the interface with standard tools, not bespoke scripts
description: Why the UI is held by Playwright Test, Vitest, Stylelint, TypeScript and mutation testing rather than by pattern-matching scripts over source text.
tags: [adr, testing, interface, tooling]
status: stable
---

# 0023. Prove the interface with standard tools, not bespoke scripts

## Context

The human surface is proven by scripts the project wrote itself. They grew
large and they test the wrong thing:

- `tests/web.rs` is about 2990 lines with more than 350 `.contains()`
  assertions on the CSS and JS **source text**. `APP_CSS.contains("height:
  34px")` and `APP_CSS.contains(".settings-theme-seg")` are a string search, not
  a check of a rendered value. A rename fails the test with no defect; a broken
  build passes it while the substring survives. The test cannot tell a value
  from a token, or a rule that applies from one that is overridden.
- `.agents/scripts/invariants.py` is about 3599 lines of Python driving
  Playwright by hand, with hundreds of `page.evaluate`, `getBoundingClientRect`
  and `wait_for_timeout` calls. It re-implements a browser test runner: its own
  fixed waits (the source of whole-run timeouts under load), its own geometry
  math, and its own screenshot byte comparisons.
- `check-web.py`, `focus-rings.py`, `interaction.py`, `a11y.py` and
  `prefix-smoke.py` repeat the same shape for one concern each.

Meanwhile Node 22 and npm are present and already used for `axe-core`, and the
standard tools for exactly these checks are unused. A UI change is expected to
ship a mutation-tested check, and the checks that were needed were the ones
those tools exist to provide: a browser runner with auto-waiting, a stylesheet
parser, a type checker, a visual differ and a mutation runner.

A check that pattern-matches source text is automation of the wrong kind. It is
brittle against refactoring, blind to the rendered result, and cheap to satisfy
without fixing anything.

## Decision

The interface is proven with standard, dev-only tooling. Each check asserts a
rendered value, a behaviour, or a parsed property of the stylesheet, never a
substring of a source file. New gates replace and delete the scripts they
supersede, so the test surface shrinks as it modernises.

| Concern | Standard tool |
|---|---|
| CSS and design tokens (type floor, 44px target, token-only colours, no transition) | Stylelint plus a `css-tree` AST walk, asserting the property and value |
| Browser behaviour and layout (focus rings, gutters, reachability) | `@playwright/test` with role and label locators and web-first auto-waiting assertions |
| Visual correctness | `expect(page).toHaveScreenshot()` baselines with a pixel threshold |
| Accessibility | `@axe-core/playwright` in the end-to-end run |
| Pure client logic | Vitest unit tests over the functions directly |
| Client types and contracts | TypeScript (`checkJs` and JSDoc, or a TypeScript source) with `tsc --noEmit`, and JSON Schema validation of the MCP tool contracts |
| Mutation coverage | `cargo-mutants` for Rust and Stryker for the JS, rather than reverting a hunk by hand |

This is a testing choice only. The shipped artifact does not change: the tooling
is a `package.json` of devDependencies, there is no bundler, and the PWA stays
vanilla ES modules. [ADR 0008](0008-lean-single-binary.md) governs the binary and
the image, which this decision leaves alone. The Rust gates (clippy, rustfmt,
the test suite, the engine check) stay as they are.

## Consequences

- The project gains a Node toolchain for development and CI. It is already
  present for `axe-core`; this makes it the standard one rather than one script's
  private dependency. The shipped binary and container do not read it.
- Checks become resilient to refactoring and honest about the rendered result: a
  renamed class no longer fails a test, and a broken rule no longer passes one.
  What a reader sees is asserted with a role locator or a screenshot baseline,
  not inferred from source text.
- Flakiness is addressed by the runner rather than by widening timeouts:
  Playwright's web-first assertions wait on the condition, so a fixed
  `wait_for_timeout` and the timeouts it caused are removed with the scripts.
- The migration is phased, and each phase deletes what it replaces: browser
  checks to Playwright Test, stylesheet rules to Stylelint and `css-tree`, pure
  logic to Vitest, types to `tsc`, mutation to the runners. The Python scripts
  and the `.contains()` blocks are removed as their replacements land, not left
  in place beside them.
- The first `tsc` run and the first Stylelint pass will surface real defects the
  pattern matchers could not see, which is the point and a one-time cost.
- The test stack now lives in the tool's own idiom, so a contributor already
  knows it, and a new check is written the way the tool documents rather than
  reinvented per concern.
