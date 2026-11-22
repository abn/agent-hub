// An answer sent from an opened item while the hub cannot be reached is
// queued, not lost: the reader is told so, no quick answer stays pressed, and
// once the connection returns the queue sends the answer and the item leaves
// the asks waiting on the reader.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, oneOffQuestion } from "./invariants.mjs";

const answerUrl = (id) => `/questions/${id}/answer`;

async function openAsk(hub, page, subject) {
  const id = await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold"] });
  expect(id, "the ask was not posted").toBeTruthy();
  await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`);
  await expect(page.getByRole("heading", { level: 2, name: subject, exact: true })).toBeVisible();
  return id;
}

const stillWaiting = async (hub, id) => {
  const held = (await api(hub, "GET", "/api/v1/inbox?status=action&limit=500")).json();
  return held.items.some((entry) => entry.event_id === id);
};

async function answerOffline(page, context) {
  await context.setOffline(true);
  const group = page.getByRole("group", { name: "Quick answers" });
  const quick = group.getByRole("button", { name: "Ship it" });
  await quick.click();
  await expect(page.getByText("Queued, will send when online.").first()).toBeVisible();
  await expect(group.locator('[aria-pressed="true"]')).toHaveCount(0);
  await expect(quick).toBeEnabled();
  await expect(quick).not.toHaveAttribute("aria-busy", "true");
  await expect(page.locator('form[aria-busy="true"]')).toHaveCount(0);
}

test("an opened item's answer is queued offline and sent when back online", async ({ hub, page, context }) => {
  const id = await openAsk(hub, page, "offline answer ask");
  try {
    await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
    await answerOffline(page, context);

    const sent = page.waitForRequest((request) => request.method() === "POST" && request.url().includes(answerUrl(id)));
    await context.setOffline(false);
    await page.evaluate(() => window.dispatchEvent(new Event("online")));
    await sent;

    await expect.poll(() => stillWaiting(hub, id)).toBe(false);
  } finally {
    await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "cleanup" }).catch(() => {});
  }
});

test("with no active worker the answer still queues and the composer is released", async ({ hub, browser }) => {
  const context = await browser.newContext({ serviceWorkers: "block" });
  const page = await context.newPage();
  await page.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
  const id = await openAsk(hub, page, "offline answer no worker");
  try {
    await answerOffline(page, context);
    await expect(page.getByRole("textbox").first()).toBeEditable();
  } finally {
    await context.setOffline(false);
    await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "cleanup" }).catch(() => {});
    await context.close();
  }
});

test("overlapping replay triggers send a queued answer once", async ({ hub, page, context }) => {
  const id = await openAsk(hub, page, "offline answer replay once");
  try {
    await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
    // Only a POST the hub received counts: the offline attempt never reaches it.
    const posted = [];
    page.on("response", (response) => {
      const request = response.request();
      if (request.method() === "POST" && request.url().includes(answerUrl(id))) posted.push(response.status());
    });
    await answerOffline(page, context);
    expect(posted, "nothing reaches the hub while offline").toEqual([]);

    await context.setOffline(false);
    await page.evaluate(() => {
      window.dispatchEvent(new Event("online"));
      window.dispatchEvent(new Event("online"));
      navigator.serviceWorker.dispatchEvent(
        new MessageEvent("message", { data: { type: "flush-offline-queue" } }),
      );
    });
    await expect.poll(() => stillWaiting(hub, id)).toBe(false);
    await page.waitForTimeout(500);
    expect(posted, "the answer was posted more than once").toHaveLength(1);
  } finally {
    await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "cleanup" }).catch(() => {});
  }
});
