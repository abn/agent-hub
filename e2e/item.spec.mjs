// One item opened at full size: where it sits among the asks, previous and
// next to walk them, answering handing on to the next ask, and on a phone the
// sheet with its one thumb bar. Each check posts its own asks, so the seeded
// ones the other checks count stay where they are.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, oneOffEvent, oneOffQuestion } from "./invariants.mjs";

const item = (hub, id) => `${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`;
const place = (page) => page.getByRole("heading", { level: 1, name: /^\d+ of \d+ (need you|can wait)$/ });
const title = (page, name) => page.getByRole("heading", { level: 2, name, exact: true });

test.describe("an opened item", () => {
  test("previous and next walk the asks, by control and by key", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "a phone's sheet has no previous and next");
    const older = await oneOffQuestion(hub, hub.projectId, "item walk older");
    const newer = await oneOffQuestion(hub, hub.projectId, "item walk newer");
    expect(older && newer, "the walk questions were not posted").toBeTruthy();
    try {
      await page.goto(item(hub, newer));
      await expect(title(page, "item walk newer")).toBeVisible();
      const [, at, of] = (await place(page).textContent()).match(/^(\d+) of (\d+)/).map(Number);

      // The newer ask is the one before the older in Now's order.
      await page.getByRole("link", { name: "Next item" }).click();
      await expect(title(page, "item walk older")).toBeVisible();
      await expect(place(page)).toHaveText(`${at + 1} of ${of} need you`);

      await page.keyboard.press("k");
      await expect(title(page, "item walk newer")).toBeVisible();
      await expect(page).toHaveURL(new RegExp(`open=${newer}$`));
      await page.keyboard.press("j");
      await expect(title(page, "item walk older")).toBeVisible();

      await page.getByRole("link", { name: "Previous item" }).click();
      await expect(title(page, "item walk newer")).toBeVisible();
    } finally {
      for (const id of [older, newer]) {
        await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "walked" }).catch(() => {});
      }
    }
  });

  test("answering an ask opens the next one it named", async ({ hub, page }) => {
    const older = await oneOffQuestion(hub, hub.projectId, "item next older", { options: ["Yes", "No"] });
    const newer = await oneOffQuestion(hub, hub.projectId, "item next newer", { options: ["Yes", "No"] });
    expect(older && newer, "the hand-on questions were not posted").toBeTruthy();
    try {
      await page.goto(item(hub, newer));
      const answer = page.getByRole("region", { name: "Your answer" });
      await expect(answer).toContainText("After you answer, the next item opens: item next older");
      const sent = page.waitForRequest(
        (request) => request.method() === "POST" && request.url().endsWith(`/api/v1/questions/${newer}/answer`),
      );
      await answer.getByRole("group", { name: "Quick answers" }).getByRole("button", { name: "Yes", exact: true }).click();
      expect((await sent).postDataJSON()).toEqual({ body: "Yes" });
      await expect(page).toHaveURL(new RegExp(`open=${older}$`));
      await expect(title(page, "item next older")).toBeVisible();
    } finally {
      for (const id of [older, newer]) {
        await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "handed on" }).catch(() => {});
      }
    }
  });

  test("a choice answers to its number key", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the keys are a desktop's");
    const id = await oneOffQuestion(hub, hub.projectId, "item key choice", { options: ["Keep it", "Drop it"] });
    expect(id, "the key choice question was not posted").toBeTruthy();
    try {
      await page.goto(item(hub, id));
      await expect(title(page, "item key choice")).toBeVisible();
      const sent = page.waitForRequest(
        (request) => request.method() === "POST" && request.url().endsWith(`/api/v1/questions/${id}/answer`),
      );
      await page.keyboard.press("2");
      expect((await sent).postDataJSON()).toEqual({ body: "Drop it" });
      await expect(page).not.toHaveURL(new RegExp(`open=${id}$`));
    } finally {
      await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "keyed" }).catch(() => {});
    }
  });

  test("a phone reads the item as a sheet with one thumb bar", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "the sheet is a phone's");
    const summary = "item sheet approval";
    const id = await oneOffEvent(hub, hub.projectId, "approval", summary);
    expect(id, "the sheet approval was not posted").toBeTruthy();
    try {
      await page.goto(item(hub, id));
      await expect(title(page, summary)).toBeVisible();
      await expect(page.getByRole("link", { name: "Close, back to Now" })).toBeVisible();
      // The sheet covers the tab bar, and the bar takes its place.
      await expect(page.locator(".tabbar")).toBeHidden();
      const bar = page.getByRole("group", { name: "Answer" });
      await expect(bar).toBeVisible();
      const viewport = page.viewportSize();
      const barBox = await bar.boundingBox();
      expect(Math.round(barBox.y + barBox.height), "the thumb bar is not on the bottom edge").toBe(viewport.height);
      for (const name of ["Approve", "Decline"]) {
        const box = await bar.getByRole("button", { name, exact: true }).boundingBox();
        expect(Math.round(box.height), `${name} is not 52px tall`).toBe(52);
      }
      await expect(bar.getByRole("button", { name: "Snooze 1h" })).toBeVisible();
      // The answer block's own verbs are the bar's on a phone, so each is drawn once.
      await expect(page.getByRole("button", { name: "Approve", exact: true })).toHaveCount(1);

      // Scrolled to the end, the last of the item clears the bar.
      await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
      const clear = await page.evaluate(() => {
        const bar = document.querySelector(".item-thumb").getBoundingClientRect();
        const ends = [...document.querySelectorAll(".item-grid > *")].map((el) => el.getBoundingClientRect().bottom);
        return Math.max(...ends) <= bar.top;
      });
      expect(clear, "the end of the item is under the thumb bar").toBe(true);

      // The bar's Approve decides in the dialog and hands on to the next ask.
      await bar.getByRole("button", { name: "Approve", exact: true }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("button", { name: "Approve", exact: true }).click();
      await expect(page).not.toHaveURL(new RegExp(`open=${id}$`));
    } finally {
      await api(hub, "POST", `/api/v1/approvals/${id}/decision`, { decision: "decline" }).catch(() => {});
    }
  });
});
