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
  const sheet = page.getByRole("dialog", { name: "Versions" });
  await expect(sheet).toBeVisible();
  await sheet.getByRole("button", { name: "Squash", exact: true }).click();

  const confirm = page.getByRole("dialog", { name: "Squash versions?" });
  await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
  await confirm.getByRole("button", { name: "Squash", exact: true }).click();
  await expect(page.getByText("Versions squashed")).toBeVisible();

  await page.getByRole("button", { name: "Version 3" }).click();
  await expect(sheet).toBeVisible();
  await expect(sheet.getByText("1 · newest first")).toBeVisible();
  await expect(sheet.getByRole("button", { name: /^v3\b/ })).toBeVisible();
  await expect(sheet.getByRole("button", { name: "Squash", exact: true })).toHaveCount(0);
});
