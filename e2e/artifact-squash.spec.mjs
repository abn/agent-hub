// An artifact's prior versions are squashed into the one being viewed, from the
// version sheet at both widths, after a confirmation.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { newSession, structured, toolCall } from "./invariants.mjs";

test("the version sheet squashes prior versions after confirmation", async ({ hub, page }) => {
  const session = await newSession(hub);
  const published = await toolCall(hub, session, "artifact_publish", {
    project_id: hub.projectId,
    title: "Squash check",
    kind: "markdown",
    content: "# Squash check\n\nv1\n",
  });
  const id = structured(published).artifact_id;
  await toolCall(hub, session, "artifact_update", {
    artifact_id: id,
    content: "# Squash check\n\nv2\n",
  });
  await toolCall(hub, session, "artifact_update", {
    artifact_id: id,
    content: "# Squash check\n\nv3\n",
  });

  await page.goto(`${hub.baseUrl}/#/artifacts/${id}?project=${hub.projectId}`);
  await expect(page.getByRole("button", { name: "Version 3" })).toBeVisible();

  await page.getByRole("button", { name: "Version 3" }).click();
  const sheet = page.locator("#hub-version-menu");
  await expect(sheet).toBeVisible();
  // A menu under a mouse and a sheet under a touch; the squash is a button
  // in the footer of either.
  const squash = sheet.locator('[data-action="version-squash"]');
  await squash.click();

  const confirm = page.getByRole("dialog", { name: "Squash versions?" });
  await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
  await confirm.getByRole("button", { name: "Squash", exact: true }).click();
  await expect(page.getByText("Versions squashed")).toBeVisible();
  // The stage repaints with the one version left, and the list is read from it.
  await expect(page.getByRole("button", { name: "Version 3" })).toContainText("of 1");

  await page.getByRole("button", { name: "Version 3" }).click();
  await expect(sheet).toBeVisible();
  await expect(sheet.getByText("1 · newest first")).toBeVisible();
  await expect(sheet.locator('.hub-version-row[data-version="3"]')).toBeVisible();
  await expect(sheet.locator('[data-action="version-squash"]')).toHaveCount(0);
});
