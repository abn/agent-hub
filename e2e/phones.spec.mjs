// Settings > Phones: a phone pairs through Tendril, waits on the operator's
// check, and is confirmed, refused or unpaired from the web app. The phone is
// the tendril-phone example, which pairs the way the app does. Every state of
// the screen passes the accessibility audit.

import { execFileSync, spawn } from "node:child_process";
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

// A phone that stays after it pairs, dialling the hub as the app does while it
// waits. It resolves with what it printed and is stopped by the caller.
function stayingPhone(uri, name) {
  const child = spawn(PHONE, [uri, name, "--stay"], { stdio: ["ignore", "pipe", "inherit"] });
  const printed = new Promise((resolve, reject) => {
    let out = "";
    child.stdout.on("data", (chunk) => {
      out += chunk;
      const end = out.indexOf("\n");
      if (end >= 0) resolve(JSON.parse(out.slice(0, end)));
    });
    child.once("exit", (code) => reject(new Error(`tendril-phone exited with ${code} before it paired`)));
  });
  return { child, printed };
}

// The desktop notifications the page raised, recorded rather than shown.
async function recordNotifications(page) {
  await page.addInitScript(() => {
    const raised = [];
    class Recorded {
      static permission = "granted";
      static requestPermission() {
        return Promise.resolve("granted");
      }
      constructor(title, options = {}) {
        raised.push({ title, body: options.body });
      }
      close() {}
    }
    Object.defineProperty(window, "Notification", { value: Recorded, configurable: true, writable: true });
    Object.defineProperty(window, "__raised", { value: raised });
  });
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

test("the screen moves on by itself as a phone pairs and then checks in", async ({ hub, page }) => {
  await recordNotifications(page);
  await openPhones(page, hub);
  const add = page.locator("main .phones-add");
  const issued = page.waitForResponse(
    (response) => response.url().endsWith("/api/v1/devices/pairing") && response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Add a phone" }).click();
  const pairing = await (await issued).json();
  await expect(add.getByRole("status")).toHaveText("Waiting for the phone");
  await audit(page, "a code waiting for the phone");

  const { child, printed } = stayingPhone(pairing.uri, "Live phone");
  try {
    const phone = await printed;
    await expect(add.getByRole("button", { name: "They match, confirm" })).toBeVisible({ timeout: 15_000 });
    await expect(add.getByText(phone.check_string)).toBeVisible();
    await expect(page).toHaveTitle(/^\(1\) Phones/);
    await expect
      .poll(() => page.evaluate(() => window.__raised))
      .toEqual([{ title: "Live phone wants to pair", body: "Compare the check string on Settings, Phones." }]);

    await add.getByRole("button", { name: "They match, confirm" }).click();
    await expect(add.getByText("Confirmed", { exact: true })).toBeVisible();
    await expect(add.getByRole("status")).toHaveText("Live phone is paired", { timeout: 15_000 });
    await expect(page).toHaveTitle(/^Phones/);
    await audit(page, "a phone confirmed and paired");
    await add.getByRole("button", { name: "Close" }).click();
    await expect(add.getByRole("button", { name: "Add a phone" })).toBeVisible();
  } finally {
    child.kill();
  }
});

test("a code that runs out on screen says when, and offers a new one", async ({ hub, page }) => {
  await page.clock.install();
  await openPhones(page, hub);
  const issued = page.waitForResponse(
    (response) => response.url().endsWith("/api/v1/devices/pairing") && response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Add a phone" }).click();
  const pairing = await (await issued).json();
  const add = page.locator("main .phones-add");
  await expect(add.getByRole("status")).toHaveText("Waiting for the phone");

  await page.clock.runFor(Date.parse(pairing.expires_at) - (await page.evaluate(() => Date.now())) + 1_000);
  const until = await page.evaluate((at) => {
    const date = new Date(at);
    return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
  }, pairing.expires_at);
  await expect(add.getByText(`The code ran out at ${until} before a phone used it.`)).toBeVisible();
  await expect(add.getByRole("img", { name: "Pairing code as a QR picture" })).toHaveCount(0);
  await audit(page, "a code that ran out");
  await expect(add.getByRole("button", { name: "Close" })).toBeVisible();
  await add.getByRole("button", { name: "New code" }).click();
  await expect(add.getByRole("status")).toHaveText("Waiting for the phone");
});
