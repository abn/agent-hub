import { api, expect, oneOffQuestion, test } from "./invariants.mjs";

test("an answered question groups its reply under the root", async ({ hub, page }) => {
  const id = await oneOffQuestion(hub, hub.projectId, "thread grouping check", { options: ["Yes", "No"] });
  expect(id, "the question was not posted").toBeTruthy();
  await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "grouped answer body" });

  await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/feed`);
  await page.waitForSelector(".feed-row", { timeout: 15000 });
  await expect(page.locator(".feed-thread-reply", { hasText: "grouped answer body" })).toHaveCount(1);
  // The answer is not a standalone feed row.
  await expect(page.locator(`.feed-row[data-id="${id}"]`)).toBeVisible();
});
