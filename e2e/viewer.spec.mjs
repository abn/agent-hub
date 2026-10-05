// The artifact viewer's own surface: where its actions menu opens, and the
// number of headers and ways out its compose sheet offers.
//
// The menu is a position rather than a value, so it is read as geometry: it
// must begin under its trigger and share the trigger's right edge. Anchored to
// the viewer instead, whose positioned box spans the index, the stage and the
// comments column, it opened over the comments column several hundred pixels to
// the right of the button. The bounding box is the standard tool for a position
// and a redesign that keeps the menu on its trigger passes.
//
// The compose sheet is opened from the viewer's own control, so both widths are
// asked: the drawer mounts at every width and the sheet is what it becomes.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

// One artifact read at its own address, the one a feed link and a share link
// both reach, rather than the project stage.
async function openArtifact(page, hub) {
  await page.goto(`${hub.baseUrl}/#/artifacts/${hub.artifactId}?project=${hub.projectId}`);
  await expect(page.locator("#hub-frame")).toBeVisible();
}

test.describe("the artifact viewer's actions", () => {
  test("the overflow menu hangs from its trigger, not the viewer's edge", async ({ hub, page }) => {
    await openArtifact(page, hub);
    const trigger = page.getByRole("button", { name: "More" });
    await expect(trigger).toBeVisible();
    await trigger.click();
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();

    const buttonBox = await trigger.boundingBox();
    const menuBox = await menu.boundingBox();
    expect(
      menuBox.y,
      "the menu opened above its trigger",
    ).toBeGreaterThanOrEqual(buttonBox.y);
    expect(
      Math.abs(menuBox.x + menuBox.width - (buttonBox.x + buttonBox.width)),
      "the menu did not share its trigger's right edge",
    ).toBeLessThanOrEqual(16);
  });

  test("the compose sheet offers one header and one way out", async ({ hub, page }) => {
    await openArtifact(page, hub);
    await page.getByRole("button", { name: "Start a thread" }).click();
    const sheet = page.getByRole("dialog");
    await expect(sheet).toBeVisible();
    // The drawer's own "Comments" head belongs to the list. Stacked above the
    // compose head it drew two titles and two close buttons in one sheet.
    await expect(page.getByRole("heading", { name: "Comments" })).toBeHidden();
    await expect(sheet.getByRole("button", { name: /^Close/ })).toHaveCount(1);
  });
});
