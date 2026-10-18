// A session anchor is the checkpoint a resumed agent reads: the phase, the
// plan, the source events it rests on, and the human's latest score. The resume
// view is the work after it, and the human writes a score from the card.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

const sessionRoute = (hub) =>
  `#/projects/${hub.projectId}/sessions?id=${encodeURIComponent(hub.sessionId)}`;

test.describe("session anchors", () => {
  test("the detail reads the latest anchor and its score", async ({ page, hub }) => {
    await page.goto(`${hub.baseUrl}/${sessionRoute(hub)}`);
    const card = page.locator(".session-anchor-card");
    // A cold hub starts every project at once; the detail is a second paint
    // after the session list, so it gets more than the default wait.
    await expect(card).toBeVisible({ timeout: 15_000 });
    await expect(card.locator(".session-anchor-phase")).toHaveText(hub.fixture.anchorPhase);
    await expect(card.locator(".session-anchor-summary")).toHaveText(hub.fixture.anchorSummary);
    for (const step of hub.fixture.anchorSteps) {
      await expect(card).toContainText(step);
    }
    await expect(card).toContainText("Score 4/5");
    await expect(card).toContainText(hub.fixture.scoreRationale);
    await expect(card.getByRole("link", { name: new RegExp(hub.fixture.finished) })).toBeVisible();
    await expect(card.getByRole("button", { name: "Score this run" })).toBeVisible();
  });

  test("the resume view reads the events after the anchor", async ({ page, hub }) => {
    await page.goto(`${hub.baseUrl}/${sessionRoute(hub)}`);
    await page.getByRole("link", { name: "Resume" }).click();
    await expect(page).toHaveURL(/resume=1/);
    const list = page.locator(".session-resume-list");
    await expect(list).toBeVisible({ timeout: 15_000 });
    await expect(list).toContainText(hub.fixture.anchorAfter);
    await expect(page.locator(".session-anchor-card")).toBeVisible();
    await expect(page.getByRole("link", { name: "Resume" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await expect(page.getByRole("link", { name: "Brain" })).toBeVisible();
  });

  test("a score needs a choice and a rationale", async ({ page, hub }) => {
    await page.goto(`${hub.baseUrl}/${sessionRoute(hub)}`);
    await page.getByRole("button", { name: "Score this run" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await dialog.getByLabel("Rationale").fill("Words are not a score.");
    await dialog.getByRole("button", { name: "Save score" }).click();
    await expect(dialog).toContainText("Choose a score from 1 to 5.");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
  });

  test("the human scores the run and the card reads the new score", async ({ page, hub }) => {
    await page.goto(`${hub.baseUrl}/${sessionRoute(hub)}`);
    await page.getByRole("button", { name: "Score this run" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("radio", { name: "5" }).check();
    await dialog.getByLabel("Rationale").fill("A measured second review.");
    await dialog.getByRole("button", { name: "Save score" }).click();
    await expect(page.locator(".session-anchor-score-value")).toHaveText("Score 5/5");
    await expect(page.locator(".session-anchor-score-rationale")).toHaveText(
      "A measured second review.",
    );
  });

  test("the feed draws anchor and score rows and offers both filters", async ({ page, hub }) => {
    await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/feed`);
    const anchorRow = page.locator(".feed-row", { hasText: hub.fixture.anchorSummary });
    await expect(anchorRow).toBeVisible();
    await expect(anchorRow.locator('.glyph[data-kind="anchor"]')).toBeVisible();
    const scoreRow = page.locator(".feed-row", { hasText: "Scored the run" }).first();
    await expect(scoreRow).toBeVisible();
    await expect(scoreRow.locator('.glyph[data-kind="score"]')).toBeVisible();

    // A phone keeps the index controls behind its filter control; a desktop
    // draws them in the row already.
    const filter = page.getByRole("button", { name: "Filter and group" });
    if (await filter.isVisible()) await filter.click();
    await page.getByRole("button", { name: "Group", exact: true }).click();
    await expect(page.getByRole("menuitemradio", { name: /^Anchors/ })).toBeVisible();
    await expect(page.getByRole("menuitemradio", { name: /^Scores/ })).toBeVisible();
  });
});
