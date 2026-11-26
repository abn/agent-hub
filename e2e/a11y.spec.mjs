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
const screens = (hub, member) => [
  ["now", "#/home", "main .now .now-ask"],
  ["now, needs you", "#/inbox", "main .now .now-ask"],
  ["inbox item", `#/inbox?open=${encodeURIComponent(hub.questionId)}`, "main .inbox-detail .item-title"],
  ["projects", "#/projects", "main .projects-screen .project-row"],
  ["feed", `#/projects/${hub.projectId}/feed`, "main .shell-index .feed-day .feed-row"],
  ["sessions", `#/projects/${hub.projectId}/sessions`, "main .session-row"],
  ["artifacts", `#/projects/${hub.projectId}/artifacts`, "main .shell-index .artifact-row"],
  [
    "artifact address",
    `#/artifacts/${hub.artifactId}?project=${hub.projectId}`,
    `main .shell.has-selection[data-segment="artifacts"] .shell-stage #hub-frame`,
  ],
  ["storage", "#/storage", "main .storage .storage-row"],
  ["settings", "#/settings", "main .row .title, main .form-row-title"],
  ["access", "#/access", "main .access-screen, main .form-row-title"],
  ["a member", `#/access?member=${encodeURIComponent(member)}`, "main .member-detail-body, main .access-screen"],
  ["connect", "#/connect", "main .connect .connect-field"],
  ["more", "#/more", "main .more-screen .more-row"],
  ["notifications", "#/notifications", "main .notify-column"],
];

// The first member that has joined, so the screen that reads one is audited
// on what the hub really holds.
async function firstMember(hub) {
  const response = await fetch(`${hub.baseUrl}/api/v1/members`, { headers: { Authorization: `Bearer ${hub.token}` } });
  const { members } = await response.json();
  return (members.find((m) => m.state !== "pending") || members[0] || { id: "none" }).id;
}

// Both themes paint the same DOM with other colours, so a contrast failure is
// found in one and not the other: each screen is read under each.
for (const scheme of ["light", "dark"]) {
  test.describe(`the accessibility audit, ${scheme}`, () => {
    test.use({ colorScheme: scheme });

    test("the main screens have no WCAG A or AA violations", async ({ hub, page }) => {
      const member = await firstMember(hub);
      for (const [name, route, selector] of screens(hub, member)) {
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

    test("an open Who menu has no violations", async ({ hub, page }) => {
      await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/feed`);
      const who = page.locator('.who-menu [data-group-toggle]').first();
      // The menu sits in a desktop's control row; a phone reaches it behind the
      // filter glyph, which the screens above already audit.
      test.skip(!(await who.isVisible().catch(() => false)), "the menu is behind the filter glyph at this width");
      await who.click();
      await expect(page.locator(".who-menu [role='menu']").first()).toBeVisible();
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(
        results.violations.map((violation) => `${violation.id}: ${violation.help}`),
        "the open Who menu has accessibility violations",
      ).toEqual([]);
    });
  });
}
