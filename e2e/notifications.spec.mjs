// Notifications: a target is added from Settings, its test send says what the
// target answered, its switch holds across a reload, and delete asks first. The
// URL is written once and never shown again. Each check removes what it added,
// so the seeded hub's Settings reads as before.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

async function openNotifications(page, hub) {
  await page.goto(`${hub.baseUrl}/#/settings`);
  await page.getByRole("link", { name: /^Notifications/ }).click();
  await expect(page.getByRole("heading", { name: "Notifications" })).toBeVisible();
}

test("a target is added, tested, switched off and deleted", async ({ hub, page }, testInfo) => {
  const name = `Check ${testInfo.project.name}`;
  // The hub's own health route answers a POST with 405, so the test send has a
  // real refusal to report.
  const url = `${hub.baseUrl}/healthz?secret=kept-on-the-hub`;
  await openNotifications(page, hub);

  await page.getByRole("button", { name: "Add target" }).click();
  const dialog = page.getByRole("dialog", { name: "Add a notification target" });
  await dialog.getByLabel("Service").selectOption("webhook");
  await dialog.getByLabel("Name", { exact: true }).fill(name);
  await dialog.getByLabel("URL", { exact: true }).fill(url);
  await dialog.getByLabel("What it sends").selectOption("title");
  await dialog.getByRole("button", { name: "Add target" }).click();
  await expect(dialog).toHaveCount(0);

  const toggle = page.getByRole("switch", { name });
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  const row = page.locator(".notify-row").filter({ has: toggle });
  await expect(row).toContainText("Webhook · Kind, project and session · Asks, Deadlines");
  await expect(row).toContainText("Nothing sent since the hub started");

  await page.getByRole("button", { name: `Send test to ${name}` }).click();
  await expect(row).toContainText(/Failed .*The target refused it \(HTTP 405\)/);
  await row.getByText("Details").click();
  await expect(row.locator(".notify-raw code")).toContainText("405");

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await page.reload();
  await expect(page.getByRole("switch", { name })).toHaveAttribute("aria-checked", "false");
  expect(await page.evaluate(() => document.body.innerText), "the URL came back to the page").not.toContain("kept-on-the-hub");

  await page.getByRole("button", { name: `Delete ${name}` }).click();
  await page.getByRole("dialog", { name: `Delete ${name}?` }).getByRole("button", { name: "Delete target" }).click();
  await expect(page.getByRole("switch", { name })).toHaveCount(0);
});

test("a vendor cloud says what leaves the network, and takes no token", async ({ hub, page }) => {
  await openNotifications(page, hub);
  await page.getByRole("button", { name: "Add target" }).click();
  const dialog = page.getByRole("dialog", { name: "Add a notification target" });

  await dialog.getByLabel("Service").selectOption("ntfy");
  await expect(dialog.getByLabel("Token")).toBeVisible();
  await expect(dialog.getByLabel("What it sends")).toHaveValue("none");

  await dialog.getByLabel("Service").selectOption("slack");
  await expect(dialog.getByLabel("Token")).toBeHidden();
  await expect(dialog.getByText(/leave your network/)).toBeHidden();
  await dialog.getByLabel("What it sends").selectOption("full");
  await expect(dialog.getByText("Slack is a vendor's cloud: these details leave your network.")).toBeVisible();

  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);
});
