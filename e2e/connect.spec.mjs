// The control row of the Connect screen says what this browser's token does.

import { expect, test as bare } from "@playwright/test";
import { readHub } from "./hub.mjs";
import { test } from "./app.mjs";

const state = (page) => page.locator(".shell-controls .connect-tools-text");

test.describe("connect state", () => {
  test("reads Connected with a working token", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/#/connect`);
    await expect(state(page)).toHaveText("Connected");
  });

  test("reads Token refused after a 401 redirect", async ({ hub, page }) => {
    await page.route(/\/api\/v1\/home(\?|$)/, (route) =>
      route.fulfill({ status: 401, contentType: "application/json", body: JSON.stringify({ error: "Unauthorized" }) }),
    );
    await page.goto(`${hub.baseUrl}/#/home`);
    await expect(page).toHaveURL(/#\/connect\?next=.*reason=refused/);
    await expect(state(page)).toHaveText("Token refused");
  });

  bare("reads Not connected with no token", async ({ page }, testInfo) => {
    const hub = readHub(testInfo.project.name);
    await page.goto(`${hub.baseUrl}/#/connect`);
    await expect(state(page)).toHaveText("Not connected");
  });
});
