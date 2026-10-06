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
      testIgnore: /(a11y|focus-rings|prefix)\.spec\.mjs/,
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
    {
      name: "prefix",
      use: { viewport: { width: 1024, height: 800 } },
      testMatch: /prefix\.spec\.mjs/,
    },
  ],
});
