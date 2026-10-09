// A waiting row in an index keeps the whole title line for its subject. The
// waiting state is the action dot at the row's end and the title's weight,
// the title is described by the words, and the "Waiting on you" pill sits on
// the meta line under the subject rather than beside it. The inbox groups its
// waiting rows under those words, so only a snoozed row there carries a pill.
//
// The type is pinned to Cantarell (see pinned-font.mjs), because a character
// count and a screenshot are only comparable between hosts that set the same
// face. The floors are measured under that pin, not guessed, and count the
// subject only, after the "asked: " the feed sets before a question. At the
// 300px default index on a desktop the check's subject shows 18 characters in
// the feed and 26 in the inbox, where the pill beside it in the feed left 1 to
// 5. On a phone it shows 31 and 38, where the feed showed 13 to 18. The floors
// sit a little under the measured counts, so a face that sets a pixel wider
// does not fail the check and the old layout always does.

import { expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { test } from "./app.mjs";
import { api, oneOffQuestion } from "./invariants.mjs";
import { fontsSettled, pinFont, pinnedFontOptions, SCREENSHOT } from "./pinned-font.mjs";

const SUBJECT = "Rotate the staging certificate on the edge host before Friday";
const FLOOR = { "rows-desktop": 16, "rows-phone": 28 };
const LONG_ACTOR = "release-coordinator-for-the-staging-and-production-edge-hosts";
const SCHEMES = ["light", "dark"];

// How many characters of the subject the title line shows before it is cut.
// A character counts when it ends inside the line, short of the ellipsis the
// line draws when the text runs over. A prefix the row sets before the subject,
// such as the feed's "asked: ", is not counted.
const subjectShown = (link) =>
  link.evaluate((el, subject) => {
    const line = el.closest(".title") || el;
    const box = line.getBoundingClientRect();
    const over = line.scrollWidth > line.clientWidth + 0.5;
    const probe = document.createElement("span");
    probe.style.font = getComputedStyle(line).font;
    probe.style.position = "absolute";
    probe.textContent = "\u2026";
    document.body.append(probe);
    const ellipsis = over ? probe.getBoundingClientRect().width : 0;
    probe.remove();
    const end = box.left + line.clientWidth - ellipsis + 0.5;
    const skip = Math.max(0, (el.textContent || "").indexOf(subject));
    const range = document.createRange();
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    let at = 0;
    let shown = 0;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      for (let i = 0; i < node.length; i++, at++) {
        if (at < skip) continue;
        range.setStart(node, i);
        range.setEnd(node, i + 1);
        if (range.getBoundingClientRect().right <= end) shown++;
      }
    }
    return shown;
  }, SUBJECT);

// The pill is the drawn copy of the words: the other is the dot's, in a
// one-pixel box for the title's description.
async function drawnPill(row) {
  const boxes = [];
  for (const words of await row.getByText("Waiting on you", { exact: true }).all()) {
    const box = await words.boundingBox();
    if (box && box.width > 1 && box.height > 1) boxes.push(box);
  }
  return boxes;
}

// The pill is under the title and on the line the time is on.
async function expectPillOnMetaLine(row, link, what) {
  const boxes = await drawnPill(row);
  expect(boxes.length, `${what}: the row draws no waiting pill`).toBe(1);
  const [drawn] = boxes;
  const title = await link.boundingBox();
  const stamp = await row.locator("time").boundingBox();
  expect.soft(drawn.y, `${what}: the pill is on the title line`).toBeGreaterThanOrEqual(title.y + title.height - 0.5);
  const middle = drawn.y + drawn.height / 2;
  expect.soft(
    middle >= stamp.y && middle <= stamp.y + stamp.height,
    `${what}: the pill is not on the meta line with the time`,
  ).toBe(true);
}

async function openIndex(page, hub, route) {
  await page.goto(`${hub.baseUrl}/`);
  // The default width, whatever an earlier check left behind.
  await page.evaluate(() => {
    try {
      localStorage.removeItem("ah-w-index");
    } catch {
      // A browser that refuses storage paints the default anyway.
    }
  });
  await page.goto(`${hub.baseUrl}/${route}`);
  await fontsSettled(page);
  const index = page.locator(".shell-index");
  const named = page.getByRole("link", { name: new RegExp(SUBJECT) });
  const link = index.locator(named);
  await expect(link).toBeVisible();
  return { index, link, row: index.locator(".row").filter({ has: named }) };
}

const routes = (hub) => [
  ["feed", `#/projects/${encodeURIComponent(hub.projectId)}/feed`],
  ["inbox", "#/inbox"],
];

test.use(pinnedFontOptions);

test.describe("a waiting row in an index", () => {
  test.beforeEach(async ({ hub, page }) => {
    await pinFont(page);
    const held = (await api(hub, "GET", "/api/v1/inbox?status=action&limit=500")).json();
    if (!held.items.some((item) => item.summary === SUBJECT)) {
      expect(await oneOffQuestion(hub, hub.projectId, SUBJECT), "the waiting question was not posted").not.toBe("");
    }
  });

  test("keeps the title line for the subject and says it is waiting", async ({ hub, page }, testInfo) => {
    const floor = FLOOR[testInfo.project.name];
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const [screen, route] of routes(hub)) {
        const what = `${screen} in the ${scheme} theme`;
        const { link, row } = await openIndex(page, hub, route);
        const shown = await subjectShown(link);
        await expect.soft(link, `${what}: the title does not say the row is waiting`).toHaveAccessibleDescription(
          "Waiting on you",
        );
        expect.soft(shown, `${what}: the title line shows ${shown} characters of the subject`).toBeGreaterThanOrEqual(floor);
        if (screen === "inbox") {
          expect.soft((await drawnPill(row)).length, `${what}: the row repeats its group's words in a pill`).toBe(0);
          continue;
        }
        await expectPillOnMetaLine(row, link, what);
      }
    }
  });

  test("a snoozed waiting row carries the pill on its meta line", async ({ hub, page }) => {
    const held = (await api(hub, "GET", "/api/v1/inbox?status=action&limit=500")).json();
    const item = held.items.find((entry) => entry.summary === SUBJECT);
    expect(item, "the waiting question is not in the inbox").toBeTruthy();
    await page.goto(`${hub.baseUrl}/`);
    await page.evaluate((id) => localStorage.setItem("hub.snooze", JSON.stringify({ [id]: Date.now() + 3_600_000 })), item.event_id);
    try {
      const { link, row } = await openIndex(page, hub, "#/inbox");
      await expect(page.getByRole("button", { name: "Bring back" }).first(), "the row was not snoozed").toBeVisible();
      await expect(link, "the snoozed title does not say the row is waiting").toHaveAccessibleDescription("Waiting on you");
      await expectPillOnMetaLine(row, link, "the snoozed row");
    } finally {
      await page.evaluate(() => localStorage.removeItem("hub.snooze"));
    }
  });

  test("a long agent name gives way to the time on every feed row", async ({ hub, page }) => {
    await page.route("**/api/v1/projects/*/feed*", async (route) => {
      const answer = await route.fetch();
      const body = await answer.json();
      for (const event of body.events || []) event.actor = LONG_ACTOR;
      await route.fulfill({ response: answer, json: body });
    });
    const { index, row } = await openIndex(page, hub, routes(hub)[0][1]);
    const quiet = index
      .locator(".row")
      .filter({ hasText: LONG_ACTOR })
      .filter({ hasNot: page.getByText("Waiting on you", { exact: true }) })
      .first();
    for (const [which, held] of [
      ["the waiting row", row],
      ["a row with nothing waiting", quiet],
    ]) {
      const stamp = held.locator("time");
      await expect(stamp, `${which} shows no time`).toBeVisible();
      const fits = await held.evaluate((el, long) => {
        const time = el.querySelector("time");
        const meta = time.parentElement;
        const name = [...meta.children].find((child) => child.textContent === long);
        const line = meta.getBoundingClientRect();
        const drawn = time.getBoundingClientRect();
        return {
          inside: drawn.width > 0 && drawn.left >= line.left - 0.5 && drawn.right <= line.right + 0.5,
          cut: Boolean(name) && name.scrollWidth > name.clientWidth,
        };
      }, LONG_ACTOR);
      expect(fits.inside, `${which} pushes its time out of the meta line`).toBe(true);
      expect(fits.cut, `${which} does not shorten the agent name`).toBe(true);
    }
  });

  test("passes the accessibility audit in both themes", async ({ hub, page }) => {
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const [screen, route] of routes(hub)) {
        await openIndex(page, hub, route);
        const results = await new AxeBuilder({ page }).include(".shell-index").withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"]).analyze();
        expect(
          results.violations.map((violation) => `${violation.id}: ${violation.help}`),
          `${screen} in the ${scheme} theme has accessibility violations`,
        ).toEqual([]);
      }
    }
  });

  test("matches its baselines @visual", async ({ hub, page }) => {
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const [screen, route] of routes(hub)) {
        const { row } = await openIndex(page, hub, route);
        await page.mouse.move(0, 0);
        await expect(row).toHaveScreenshot(`${screen}-waiting-row-${scheme}.png`, {
          ...SCREENSHOT,
          mask: [row.locator("time")],
        });
      }
    }
  });
});
