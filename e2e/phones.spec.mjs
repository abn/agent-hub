// Settings > Phones: a phone pairs through Tendril, waits on the operator's
// check, and is confirmed, refused or unpaired from the web app. The phone is
// the tendril-phone example, which pairs the way the app does. Every state of
// the screen passes the accessibility audit.

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { test } from "./app.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const PHONE = join(ROOT, "target", "debug", "examples", "tendril-phone");

async function openPhones(page, hub) {
  await page.goto(`${hub.baseUrl}/#/settings`);
  await page.getByRole("link", { name: /^Phones/ }).click();
  await expect(page.locator("main .phones .phones-add")).toBeVisible();
}

async function audit(page, what) {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  expect(results.violations.map((v) => `${v.id}: ${v.help}`), what).toEqual([]);
}

// A phone pairs with the code on screen. The page asks for the code, as the
// web app does, so the request is the app's own.
async function pairPhone(page, name) {
  const pairing = await page.evaluate(async () => {
    const token = localStorage.getItem("hub.token");
    const response = await fetch("api/v1/devices/pairing", {
      method: "POST",
      headers: token ? { Authorization: `Bearer ${token}` } : {},
    });
    return response.json();
  });
  const out = execFileSync(PHONE, [pairing.uri, name], { encoding: "utf8", timeout: 20_000 });
  return JSON.parse(out);
}

test.beforeEach(() => {
  test.skip(!existsSync(PHONE), "the tendril-phone example is not built (make build)");
});

test("each state of Add a phone passes the audit", async ({ hub, page }) => {
  await openPhones(page, hub);
  await expect(page.getByRole("button", { name: "Add a phone" })).toBeVisible();
  await audit(page, "no code");

  await page.getByRole("button", { name: "Add a phone" }).click();
  await expect(page.getByRole("img", { name: "Pairing code as a QR picture" })).toBeVisible();
  await expect(page.getByText(/^TND-/)).toBeVisible();
  await audit(page, "a code on screen");
  await page.getByRole("button", { name: "Close" }).click();
  await expect(page.getByRole("button", { name: "Add a phone" })).toBeVisible();

  const phone = await pairPhone(page, "Audit phone");
  await page.reload();
  await expect(page.getByText("Check string", { exact: true })).toBeVisible();
  await expect(page.getByText(phone.check_string)).toBeVisible();
  await audit(page, "a phone waiting");
  await page.getByRole("button", { name: "They differ, refuse" }).click();
  await expect(page.getByText("Audit phone")).toHaveCount(0);
});

test("a phone is confirmed, then unpaired after a question", async ({ hub, page }) => {
  await openPhones(page, hub);
  const phone = await pairPhone(page, "Confirm phone");
  await page.reload();
  await expect(page.getByText(phone.check_string)).toBeVisible();
  await page.getByRole("button", { name: "They match, confirm" }).click();

  const row = page.locator(".phones-row").filter({ hasText: "Confirm phone" });
  await expect(row).toContainText("active");
  await row.getByRole("button", { name: "Unpair…" }).click();
  const dialog = page.getByRole("dialog", { name: "Unpair Confirm phone?" });
  await expect(dialog.getByRole("button", { name: "Keep" })).toBeFocused();
  await dialog.getByRole("button", { name: "Unpair" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(row).toContainText("unpaired");
  await expect(row.getByRole("button", { name: "Unpair…" })).toHaveCount(0);
});

test("a refused phone is gone and pairing it again needs a new code", async ({ hub, page }) => {
  await openPhones(page, hub);
  const phone = await pairPhone(page, "Refused phone");
  await page.reload();
  await expect(page.getByText(phone.check_string)).toBeVisible();
  await page.getByRole("button", { name: "They differ, refuse" }).click();
  await expect(page.getByText("Refused phone")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Add a phone" })).toBeVisible();
});
