// Now: one stream of what needs the reader, what changed since they looked and
// the quiet rest. These hold what a reader reaches on it: the Inbox's address
// landing on the asks, an ask answered in place with focus moving on, a snooze
// that takes an ask out of the count, the quiet tier's fold, and Mark seen
// posting the read state the feed and the inbox already use.

import { expect } from "@playwright/test";
import { open, test } from "./app.mjs";
import { oneOffQuestion } from "./invariants.mjs";

const chip = (page, name) => page.locator(".shell-controls").getByRole("button", { name });
const badge = (page) => page.locator("#tab-badge");

test.describe("Now", () => {
  test("the Inbox's address opens Now showing what needs the reader", async ({ hub, page }) => {
    await open(page, hub, "#/inbox", "");
    await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();
    await expect(chip(page, /^Needs you \d+$/)).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByRole("heading", { name: /^Needs you · \d+/ })).toBeVisible();
    await expect(page.getByRole("heading", { name: /^Since you looked/ })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: /^Quiet/ })).toHaveCount(0);
    // Every ask is a sentence that names the agent first, with a way to read it.
    const first = page.locator(".now-ask").first();
    await expect(first.locator(".now-s b").first()).not.toBeEmpty();
    await expect(first.getByRole("link", { name: "Read" })).toHaveAttribute("href", /#\/inbox\?open=/);

    await chip(page, "All").click();
    await expect(page).toHaveURL(/#\/home$/);
    await expect(chip(page, "All")).toHaveAttribute("aria-pressed", "true");
    await expect(chip(page, "All")).toBeFocused();
  });

  test("a suggested answer sends at once and the next ask takes the focus", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the keys are a desktop's");
    const subject = "now choice check";
    const id = await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold"] });
    expect(id, "the choice question was not posted").toBeTruthy();
    await open(page, hub, "#/inbox", "");
    const asks = page.locator(".now-ask");
    const row = asks.filter({ hasText: subject });
    await expect(row).toBeVisible();
    const at = await asks.evaluateAll((rows, want) => rows.findIndex((r) => r.textContent.includes(want)), subject);
    const before = await asks.count();
    await row.click({ position: { x: 2, y: 2 } });
    await expect(row).toHaveAttribute("tabindex", "0");
    // Only the selected row fills its primary verb.
    const filled = await page.locator(".now-ask button[data-now]").evaluateAll((buttons) =>
      buttons
        .filter((b) => getComputedStyle(b).backgroundColor !== "rgba(0, 0, 0, 0)")
        .map((b) => b.closest(".now-ask")?.textContent.includes("now choice check")),
    );
    expect(filled, "a row other than the selected one fills its primary").toEqual([true]);

    const sent = page.waitForRequest((request) => request.method() === "POST" && request.url().endsWith(`/api/v1/questions/${id}/answer`));
    await page.keyboard.press("1");
    expect((await sent).postDataJSON()).toEqual({ body: "Ship it" });
    await expect(asks).toHaveCount(before - 1);
    await expect(asks.filter({ hasText: subject })).toHaveCount(0);
    await expect(asks.nth(Math.min(at, before - 2))).toBeFocused();
  });

  test("on a phone an ask's verbs hold one line, Read with them", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "the line under the sentence is a phone's");
    const subject = "now phone verbs";
    await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold until Monday"] });
    await open(page, hub, "#/inbox", "");
    const row = page.locator(".now-ask").filter({ hasText: subject });
    await expect(row.getByRole("link", { name: "Read" })).toBeVisible();
    const placed = await row.evaluate((ask) => {
      const verbs = [...ask.querySelectorAll(".now-acts > button:not([hidden]), .now-acts > a")];
      return {
        tops: [...new Set(verbs.map((verb) => Math.round(verb.getBoundingClientRect().top)))],
        right: Math.max(...verbs.map((verb) => verb.getBoundingClientRect().right)),
        edge: ask.getBoundingClientRect().right,
      };
    });
    expect(placed.tops, "the ask's verbs break across lines").toHaveLength(1);
    expect(placed.right, "the verbs run past the row").toBeLessThanOrEqual(placed.edge + 0.5);
  });

  test("a snoozed ask can wait and leaves the count", async ({ hub, page }) => {
    await open(page, hub, "#/inbox", "");
    const needs = chip(page, /^Needs you \d+$/);
    const count = Number((await needs.textContent()).replace(/\D/g, ""));
    expect(count, "nothing needs the reader, so there is nothing to snooze").toBeGreaterThan(0);
    const first = page.locator(".now-ask:not(.is-later)").first();
    const summary = (await first.locator(".now-s").textContent()).trim().slice(0, 30);
    // The key is the desktop's way in; a phone snoozes from the item's card.
    if (test.info().project.name === "desktop") {
      await first.click({ position: { x: 2, y: 2 } });
      await page.keyboard.press("s");
    } else {
      // Snoozed from the item, the next ask opens; the reader closes it to
      // come back to Now.
      const read = first.getByRole("link", { name: "Read" });
      const opened = (await read.getAttribute("href")).split("open=")[1];
      await read.click();
      await page.getByRole("group", { name: "Answer" }).getByRole("button", { name: "Snooze 1h" }).click();
      await expect(page).not.toHaveURL(new RegExp(`open=${opened}$`));
      await page.getByRole("link", { name: "Close, back to Now" }).click();
      await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();
    }
    try {
      await expect(chip(page, /^Needs you \d+$/)).toHaveText(new RegExp(`^Needs you ${count - 1}$`));
      await expect(badge(page)).toHaveText(String(count - 1));
      const later = page.locator(".now-ask.is-later", { hasText: summary });
      await expect(later).toBeVisible();
      await expect(later).toContainText("Can wait.");
      await expect(later.getByRole("button", { name: "Bring back" })).toBeVisible();
      await later.getByRole("button", { name: "Bring back" }).click();
      await expect(chip(page, /^Needs you \d+$/)).toHaveText(new RegExp(`^Needs you ${count}$`));
    } finally {
      await page.evaluate(() => localStorage.removeItem("hub.snooze"));
    }
  });

  test("the quiet tier folds, and Mark seen posts the read state", async ({ hub, page }) => {
    // The cursors and the unread items are left where the seed put them: the
    // posts are answered here, and what they carried is what is held.
    const posts = [];
    await page.route(/\/api\/v1\/(projects\/[^/]+\/feed\/seen|inbox\/read-all)$/, async (route) => {
      posts.push({ url: new URL(route.request().url()).pathname, body: route.request().postDataJSON() });
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ advanced: true, marked: 0 }) });
    });
    await open(page, hub, "#/home", "");
    const since = page.getByRole("heading", { name: /^Since you looked · \d+/ });
    await expect(since).toBeVisible();
    const groups = await page.locator(".now-grp-name").allTextContents();
    expect(groups.length, "the since tier names no project").toBeGreaterThan(0);
    await page.getByRole("button", { name: "Mark seen" }).click();
    await expect.poll(() => posts.length).toBeGreaterThanOrEqual(groups.length + 1);
    expect(posts.filter((post) => post.url === "/api/v1/inbox/read-all")).toHaveLength(1);
    for (const post of posts.filter((p) => p.url.endsWith("/feed/seen"))) {
      expect(post.body.event_id, `${post.url} names no event`).toBeTruthy();
    }

    // Answered with the real state again, nothing was seen, so the tier is
    // there and the rest of the stream ends in one calm line.
    await expect(page.getByText(/^That is everything/)).toBeVisible();
    const quiet = page.getByRole("button", { name: "Show" });
    if (await quiet.count()) {
      await expect(quiet).toHaveAttribute("aria-expanded", "false");
      await expect(page.locator("#now-quiet")).toBeHidden();
      await quiet.click();
      await expect(page.getByRole("button", { name: "Hide" })).toHaveAttribute("aria-expanded", "true");
      await expect(page.locator("#now-quiet")).toBeVisible();
    }
  });
});
