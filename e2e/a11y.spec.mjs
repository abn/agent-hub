// The accessibility audit, on the standard runner: axe-core over the rendered
// screens, in the same Playwright run as the behaviour checks. This replaces the
// bespoke a11y.py, which drove axe by hand; ADR 0023 names @axe-core/playwright
// for exactly this. It asserts a rendered result, so a redesign that keeps the
// screens accessible passes and a broken one fails.

import { expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { test } from "./app.mjs";

// Each screen the audit walks, and what only that screen paints with the seeded
// data, so a screen that failed to load is not audited as whatever was on the
// page before it.
const screens = (hub) => [
  ["home", "#/home", "main .home .home-summary"],
  ["inbox", "#/inbox", "main .shell-index .inbox-item"],
  ["projects", "#/projects", "main .projects-screen .project-row"],
  ["feed", `#/projects/${hub.projectId}/feed`, "main .shell-index .feed-day .feed-row"],
  ["sessions", `#/projects/${hub.projectId}/sessions`, "main .session-row"],
  ["artifacts", `#/projects/${hub.projectId}/artifacts`, "main .shell-index .artifact-row"],
  ["storage", "#/storage", "main .storage .storage-row"],
  ["settings", "#/settings", "main .row .title, main .form-row-title"],
  ["access", "#/access", "main .access-screen, main .form-row-title"],
  ["connect", "#/connect", "main .connect .connect-field"],
  ["more", "#/more", "main .more-screen .more-row"],
];

test.describe("the accessibility audit", () => {
  test("the main screens have no WCAG A or AA violations", async ({ hub, page }) => {
    for (const [name, route, selector] of screens(hub)) {
      await page.goto(`${hub.baseUrl}/${route}`);
      await expect(
        page.locator(selector).first(),
        `${name}: the screen did not render (${selector})`,
      ).toBeVisible();
      const results = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa"])
        .analyze();
      expect(
        results.violations.map((violation) => `${violation.id}: ${violation.help}`),
        `${name} has accessibility violations`,
      ).toEqual([]);
    }
  });
});
