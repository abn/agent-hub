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
    // Every ask is a sentence that names the agent first, with a way to open it.
    const first = page.locator(".now-ask").first();
    await expect(first.locator(".now-s b").first()).not.toBeEmpty();
    await expect(first.getByRole("link", { name: "Open", exact: true })).toHaveAttribute("href", /#\/inbox\?open=/);

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

  test("on a phone the answers hold one line and Open sits right-aligned on the line below", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "the lines under the sentence are a phone's");
    const subject = "now phone verbs";
    await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold until Monday"] });
    await open(page, hub, "#/inbox", "");
    const row = page.locator(".now-ask").filter({ hasText: subject });
    await expect(row.getByRole("link", { name: "Open", exact: true })).toBeVisible();
    await expect(row.getByRole("button", { name: "Ship it" })).toBeVisible();
    const placed = await row.evaluate((ask) => {
      const box = (el) => el.getBoundingClientRect();
      const verbs = [...ask.querySelectorAll(".now-acts > button:not([hidden])")];
      const foot = [...ask.querySelectorAll(".now-foot > *")];
      return {
        tops: [...new Set(verbs.map((verb) => Math.round(box(verb).top)))],
        right: Math.max(...verbs.map((verb) => box(verb).right)),
        edge: box(ask).right,
        verbsBottom: Math.max(...verbs.map((verb) => box(verb).bottom)),
        footTops: foot.map((el) => box(el).top),
        footRight: Math.max(...foot.map((el) => box(el).right)),
        foot: foot.map((el) => el.firstChild.textContent.trim()).sort(),
      };
    });
    expect(placed.tops, "the ask's answers break across lines").toHaveLength(1);
    expect(placed.right, "the answers run past the row").toBeLessThanOrEqual(placed.edge + 0.5);
    expect(placed.foot, "Open and Reply are the bottom line").toEqual(["Open", "Reply"]);
    expect(Math.min(...placed.footTops), "Open is not below the answers").toBeGreaterThanOrEqual(placed.verbsBottom);
    expect(placed.edge - placed.footRight, "Open is not right-aligned").toBeLessThanOrEqual(2);
  });

  test("Open and Reply are smaller secondaries on a bottom line, right-aligned", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "measured on the desktop's pointer");
    const subject = "now bottom line";
    await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold"] });
    await open(page, hub, "#/inbox", "");
    const row = page.locator(".now-ask").filter({ hasText: subject });
    await row.click({ position: { x: 2, y: 2 } });
    const placed = await row.evaluate((ask) => {
      const box = (el) => el.getBoundingClientRect();
      const answer = box(ask.querySelector('.now-acts button[data-choice]'));
      const foot = [...ask.querySelectorAll(".now-foot > *")].map((el) => ({ label: el.firstChild.textContent.trim(), top: box(el).top, right: box(el).right, height: box(el).height }));
      const main = box(ask.querySelector(".now-body"));
      return { answer: { bottom: answer.bottom, height: answer.height }, foot, bodyRight: main.right };
    });
    expect(placed.foot.map((f) => f.label).sort()).toEqual(["Open", "Reply"]);
    for (const f of placed.foot) {
      expect(f.top, `${f.label} is not below the answers`).toBeGreaterThanOrEqual(placed.answer.bottom);
      expect(f.height, `${f.label} is not the compact size`).toBe(28);
    }
    expect(placed.answer.height, "the answers are not larger than the secondaries").toBeGreaterThan(28);
    expect(placed.bodyRight - Math.max(...placed.foot.map((f) => f.right)), "the bottom line is not right-aligned").toBeLessThanOrEqual(2);
  });

  test("under a touch pointer the lines are far enough apart that no 44px target overlaps", async ({ hub, browser }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "a touch pointer is a phone's");
    const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true });
    await context.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
    const page = await context.newPage();
    try {
      await oneOffQuestion(hub, hub.projectId, "now touch lines", { options: ["Ship it", "Hold"] });
      await open(page, hub, "#/inbox", "");
      const row = page.locator(".now-ask").filter({ hasText: "now touch lines" });
      await expect(row.getByRole("link", { name: "Open", exact: true })).toBeVisible();
      const gap = await row.evaluate((ask) => {
        const grown = (el) => {
          const b = el.getBoundingClientRect();
          const h = Math.max(b.height, 44);
          const mid = (b.top + b.bottom) / 2;
          return { top: mid - h / 2, bottom: mid + h / 2 };
        };
        const answers = [...ask.querySelectorAll(".now-acts > button:not([hidden])")].map(grown);
        const open = grown(ask.querySelector(".now-foot a"));
        return open.top - Math.max(...answers.map((a) => a.bottom));
      });
      expect(gap, "an answer's and Open's 44px targets overlap").toBeGreaterThanOrEqual(0);
    } finally {
      await context.close();
    }
  });

  test("an ask's sentence opens the item and an answer does not", async ({ hub, page }) => {
    const subject = "now answer stays";
    const other = "now sentence opens";
    const id = await oneOffQuestion(hub, hub.projectId, subject, { options: ["Ship it", "Hold"] });
    const second = await oneOffQuestion(hub, hub.projectId, other, { options: ["Ship it", "Hold"] });
    expect(id && second, "the questions were not posted").toBeTruthy();
    await open(page, hub, "#/inbox", "");
    const row = page.locator(".now-ask").filter({ hasText: subject });
    await expect(row).toBeVisible();
    // Choosing an answer is not opening: the address stays on Now.
    const sent = page.waitForResponse((r) => r.request().method() === "POST" && r.url().endsWith(`/api/v1/questions/${id}/answer`));
    await row.click({ position: { x: 2, y: 2 } });
    await row.getByRole("button", { name: "Ship it" }).click();
    await sent;
    await expect(page).not.toHaveURL(/open=/);
    await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();

    await page.locator(".now-ask").filter({ hasText: other }).getByRole("link", { name: other }).click();
    await expect(page).toHaveURL(new RegExp(`open=${second}`));
    await expect(page.locator("#inbox-detail-title")).toHaveText(other);
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
      const read = first.getByRole("link", { name: "Open", exact: true });
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
