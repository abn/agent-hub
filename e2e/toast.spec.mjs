// A toast stands clear of the controls a reader presses next, and one about an
// item names it. Boxes are measured, not styles read.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, oneOffEvent, oneOffQuestion } from "./invariants.mjs";

const overlap = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

// An undo toast stands for 30 s, long enough to be measured across a change.
async function raise(page, message, { lasting = false } = {}) {
  await page.evaluate(
    ([text, undo]) => import("/toast.mjs").then((module) => module.toast(text, undo ? () => {} : undefined)),
    [message, lasting],
  );
  await expect(page.locator(".toast-text")).toHaveText(message);
}

async function expectClear(page, controls) {
  const toast = await page.locator(".toast").boundingBox();
  expect(toast, "the toast is drawn").toBeTruthy();
  for (const control of controls) {
    await expect(control).toBeVisible();
    const box = await control.boundingBox();
    expect(overlap(toast, box), "the toast covers a control").toBe(false);
  }
}

test("on a phone's item sheet the toast clears the verb bar and Snooze", async ({ hub, page }, testInfo) => {
  test.skip(testInfo.project.name !== "phone", "the thumb bar is the phone's");
  const subject = "toast clears the thumb bar";
  const id = await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold"] });
  await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`);
  const bar = page.getByRole("group", { name: "Answer", exact: true });
  await expect(bar).toBeVisible();
  await raise(page, "Saved somewhere.");
  await expectClear(page, [bar, bar.getByRole("button", { name: /Snooze/ }), bar.getByRole("button").first()]);
  await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "cleanup" }).catch(() => {});
});

test("on a desktop the toast does not cover the wiki editor's Save", async ({ hub, page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "the editor's actions share the stage with the toast only on a desktop");
  const name = `toast/clear-${testInfo.project.name}.md`;
  await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`, {
    content: "---\ntype: Runbook\ntitle: Toast\n---\n# Toast\n\nBody.\n",
  });
  await page.goto(
    `${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/wiki?page=${encodeURIComponent(name)}&edit=1`,
  );
  const save = page.getByRole("button", { name: "Save", exact: true });
  await expect(save).toBeVisible();
  await raise(page, "Saved somewhere.");
  await expectClear(page, [save]);
});

test("a decision toast names the item, and an unknown item says it is gone", async ({ hub, page }) => {
  await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent("01ZZZZZZZZZZZZZZZZZZZZZZZZ")}`);
  await expect(page.locator(".toast-text")).toHaveText(/no longer waiting/);
  await expect(page).toHaveURL(/#\/home\?show=needs$/);
  await expect(page.getByRole("button", { name: "Needs you", pressed: true })).toBeVisible();

  const subject = "toast names this ask";
  const id = await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold"] });
  await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`);
  await page.getByRole("group", { name: "Quick answers" }).getByRole("button", { name: "Ship it" }).click();
  await expect(page.locator(".toast-text")).toContainText(`“${subject}”`);
});

test("a typed reply sent from Now names the item", async ({ hub, page }) => {
  const subject = "toast names a typed reply";
  await oneOffQuestion(hub, hub.projectId, subject);
  await page.goto(`${hub.baseUrl}/#/home`);
  await page.locator(".now-ask", { hasText: subject }).getByRole("button", { name: "Reply", exact: true }).click();
  await page.getByRole("textbox", { name: "Your answer", exact: true }).last().fill("Go ahead");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.locator(".toast-text")).toContainText(`Answer sent on “${subject}”`);
});

test.describe("the toast follows the chrome under it", () => {
  test.use({ hasTouch: true });

  test("one toast stays clear as the item sheet opens and closes and the window resizes", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "the tab bar and thumb bar are the phone's");
    const id = await oneOffQuestion(hub, hub.projectId, "toast follows the chrome", { options: ["Ship it", "Hold"] });
    await page.goto(`${hub.baseUrl}/#/home`);
    const tabs = page.locator(".tabbar");
    await expect(tabs).toBeVisible();
    await raise(page, "Standing while the screen changes.", { lasting: true });
    const text = page.locator(".toast-text");
    await expectClear(page, [tabs]);

    await page.evaluate((hash) => { location.hash = hash; }, `#/inbox?open=${encodeURIComponent(id)}`);
    const bar = page.getByRole("group", { name: "Answer", exact: true });
    await expect(bar).toBeVisible();
    await expect(text, "the same toast is still up").toHaveText("Standing while the screen changes.");
    await expect.poll(async () => overlap(await page.locator(".toast").boundingBox(), await bar.boundingBox())).toBe(false);
    await expectClear(page, [bar, bar.getByRole("button").first()]);

    await page.setViewportSize({ width: 360, height: 600 });
    await expect(bar).toBeVisible();
    await expect.poll(async () => overlap(await page.locator(".toast").boundingBox(), await bar.boundingBox())).toBe(false);

    await page.evaluate(() => { location.hash = "#/home"; });
    await expect(tabs).toBeVisible();
    await expect(text).toHaveText("Standing while the screen changes.");
    await expect.poll(async () => overlap(await page.locator(".toast").boundingBox(), await tabs.boundingBox())).toBe(false);
    await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "cleanup" }).catch(() => {});
  });

  test("a long subject is clamped in the approval dialog and in its toast", async ({ hub, page }) => {
    const subject =
      "Deploy the nightly build of the renderer to every node in the homelab cluster and restart the edge proxies afterwards";
    const id = await oneOffEvent(hub, hub.projectId, "approval", subject);
    expect(id, "the approval was not posted").toBeTruthy();
    await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`);
    await page.getByRole("button", { name: /^Decline/ }).and(page.locator(":visible")).first().click();
    const dialog = page.getByRole("dialog");
    const title = await dialog.getByRole("heading", { level: 2 }).innerText();
    expect(title).toMatch(/^Decline “.+…”\?$/);
    expect(title.length, "the dialog title is clamped").toBeLessThan(70);
    await dialog.getByRole("button", { name: "Decline" }).click();
    const text = page.locator(".toast-text");
    await expect(text).toContainText("Declined “");
    const said = await text.innerText();
    expect(said).toContain("…”");
    expect(said).not.toContain(subject);
    await expectClear(page, []);
  });
});
