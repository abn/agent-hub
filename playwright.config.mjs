// The browser behaviour checks, on the standard runner. This is the migration
// ADR 0023 asks for: a browser check is a Playwright Test that locates what a
// reader reaches by role and label, and waits for it with a web-first assertion
// rather than for a fixed number of milliseconds.
//
// One project per width, because the two widths are different screens: a phone
// shows one zone at a time and keeps a project's filter behind its own control,
// while a desktop shows the index and the stage together. The theme is a setting
// of the run rather than a project axis: every assertion here is about what the
// reader can reach, and both themes paint the same DOM, so doubling the run
// would prove nothing these checks do not already hold.
//
// One worker. A project's tests share its seeded hub and move its state, and
// the Unread chip is asked before anything reads a project feed because the hub
// draws its dots from what is still above the reader's cursor. Serial execution
// is what keeps that order, and a hub per project keeps the two widths from
// moving each other's data. The accessibility audit reads every screen,
// including a feed, so it runs in its own projects against their own hubs: its
// feed read must not advance the cursor the Unread chip counts.

import { defineConfig } from "@playwright/test";

const WIDTHS = {
  desktop: { width: 1440, height: 900 },
  phone: { width: 390, height: 844 },
};

export default defineConfig({
  testDir: "./e2e",
  testMatch: /.*\.spec\.mjs/,
  globalSetup: "./e2e/global-setup.mjs",
  fullyParallel: false,
  workers: 1,
  // Flakiness is answered by the runner, not by a wider timeout: the assertions
  // wait on a condition. A retry is still worth one on a loaded CI machine, and
  // a developer's own run gets none so a repeat is seen rather than hidden.
  retries: process.env.CI ? 1 : 0,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  // Traces and screenshots of a failure are written under the build tree, which
  // `cargo clean` empties, rather than into the checkout.
  outputDir: "target/tmp/playwright",
  reporter: process.env.CI ? [["github"], ["list"]] : [["list"]],
  use: {
    colorScheme: "light",
    trace: "on-first-retry",
  },
  projects: [
    ...Object.entries(WIDTHS).map(([name, viewport]) => ({
      name,
      use: { viewport },
      // Anchored on the file name, so a checkout whose own path carries one of
      // these words does not ignore every spec.
      testIgnore: /\/(a11y|focus-rings|prefix|invariants|control-sizing|index-rows|artifact-route)[^/]*\.spec\.mjs$/,
    })),
    // The behavioural invariants are their own project so their run does not
    // move the behaviour checks' state. They are one phone-width run with the
    // desktop screens driven by the checks that measure them, which is the
    // shape the old script ran in, and their own seeded hub.
    {
      name: "invariants",
      use: { viewport: { width: 390, height: 844 } },
      testMatch: /invariants.*\.spec\.mjs/,
    },
    // The artifact address is followed from a project's feed, and reading a feed
    // moves the hub's read cursor that Home's unread rows are drawn from. Its
    // checks get their own hub per width, so they cannot empty the dots the
    // behaviour checks count.
    ...Object.entries(WIDTHS).map(([name, viewport]) => ({
      name: `route-${name}`,
      use: { viewport },
      testMatch: /artifact-route\.spec\.mjs/,
    })),
    ...Object.entries(WIDTHS).map(([name, viewport]) => ({
      name: `a11y-${name}`,
      use: { viewport },
      testMatch: /a11y\.spec\.mjs/,
    })),
    {
      name: "focus-desktop",
      use: { viewport: { width: 1100, height: 800 } },
      testMatch: /focus-rings\.spec\.mjs/,
    },
    {
      name: "focus-phone",
      use: { viewport: { width: 390, height: 844 } },
      testMatch: /focus-rings\.spec\.mjs/,
    },
    {
      name: "focus-touch",
      use: { viewport: { width: 390, height: 844 }, hasTouch: true },
      testMatch: /focus-rings\.spec\.mjs/,
    },
    // Control sizing follows the pointer, so its two projects are one fine
    // pointer at the desktop width and one coarse pointer at the phone width,
    // each on its own hub because they walk every screen.
    {
      name: "sizing-desktop",
      use: { viewport: WIDTHS.desktop },
      testMatch: /control-sizing\.spec\.mjs/,
    },
    {
      name: "sizing-touch",
      use: { viewport: WIDTHS.phone, hasTouch: true },
      testMatch: /control-sizing\.spec\.mjs/,
    },
    // A waiting row is measured at both widths in both themes and photographed,
    // on hubs of its own so the question it posts moves no other check's queue.
    {
      name: "rows-desktop",
      use: { viewport: WIDTHS.desktop },
      testMatch: /index-rows\.spec\.mjs/,
    },
    {
      name: "rows-phone",
      use: { viewport: WIDTHS.phone, hasTouch: true },
      testMatch: /index-rows\.spec\.mjs/,
    },
    {
      name: "prefix",
      use: { viewport: { width: 1024, height: 800 } },
      testMatch: /prefix\.spec\.mjs/,
    },
  ],
});
