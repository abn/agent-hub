import { expect } from "@playwright/test";
import { test } from "./app.mjs";

test("capture idea opens a dialog", async ({ page, hub }) => {
  test.skip(test.info().project.name === "phone", "phone coverage is in control-sizing; avoid moving the shared read cursor");
  await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}`);
  const button = page.locator('button[data-action="capture-idea"]:visible');
  await expect(button).toBeVisible();
  await button.click();
  await expect(page.locator("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
});
