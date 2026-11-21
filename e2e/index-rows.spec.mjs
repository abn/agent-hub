// A feed row is Now's row in the project's index: the kind glyph in a fixed
// column, one sentence saying who did what, and the time in mono on the right.
// A row that waits on the reader says so in words under its sentence, in the
// action tone, and its link takes those words as its description; it is the
// only row that takes copper. Seen rows are quiet and unseen rows are ink with
// the unread mark under the time. A day heads its rows with its date in mono.
//
// The waiting row is a question posted on the seeded hub. The read and unread
// checks answer the feed from a fixed page through the router, so the cursor
// is known exactly and nothing is written to the hub the other checks share.
//
// The type is pinned to Cantarell (see pinned-font.mjs) for the screenshot,
// which is only comparable between hosts that set the same face.

import { expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { test } from "./app.mjs";
import { api, oneOffQuestion } from "./invariants.mjs";
import { fontsSettled, pinFont, pinnedFontOptions, SCREENSHOT } from "./pinned-font.mjs";

const SUBJECT = "Rotate the staging certificate on the edge host before Friday";
const LONG_ACTOR = "release-coordinator-for-the-staging-and-production-edge-hosts";
const SCHEMES = ["light", "dark"];

const feedRoute = (hub) => `#/projects/${encodeURIComponent(hub.projectId)}/feed`;

async function openFeed(page, hub) {
  await page.goto(`${hub.baseUrl}/`);
  // The default width, whatever an earlier check left behind.
  await page.evaluate(() => {
    try {
      localStorage.removeItem("ah-w-index");
    } catch {
      // A browser that refuses storage paints the default anyway.
    }
  });
  await page.goto(`${hub.baseUrl}/${feedRoute(hub)}`);
  await fontsSettled(page);
  const index = page.locator(".shell-index");
  await expect(index.locator(".feed-row").first()).toBeVisible();
  return index;
}

async function waitingRow(page, hub) {
  const index = await openFeed(page, hub);
  const named = page.getByRole("link", { name: new RegExp(SUBJECT) });
  const link = index.locator(named);
  await expect(link).toBeVisible();
  return { index, link, row: index.locator(".feed-row").filter({ has: named }) };
}

// A fixed page of the feed: six events, the cursor at the third, and the
// newest two today with the rest yesterday.
const NOW = Date.now();
const ago = (minutes) => new Date(NOW - minutes * 60000).toISOString();
function fixedPage(project, actor = "build-bot") {
  const event = (n, kind, summary, minutes) => ({
    id: `e000${n}`,
    project_id: project,
    kind,
    actor,
    summary,
    payload: null,
    thread_id: null,
    needs_action: false,
    created_at: ago(minutes),
    inbox_status: null,
  });
  const yesterday = (() => {
    const at = new Date(NOW);
    return Math.round((at - new Date(at.getFullYear(), at.getMonth(), at.getDate() - 1, 12)) / 60000);
  })();
  return {
    events: [
      event(6, "finished", "Finished the nightly run", 2),
      event(5, "signal", "Cache hit rate 94% on the arm64 runner.", 3),
      event(4, "session", "Session rel-1 started", yesterday),
      event(3, "signal", "Disk at 40%", yesterday + 1),
      event(2, "finished", "The nightly run scored 4 of 5", yesterday + 2),
      event(1, "session", "Session flaky-hunt ended", yesterday + 3),
    ],
    last_seen: "e0003",
    next_before: null,
  };
}

async function fixedFeed(page, hub, actor) {
  const body = fixedPage(hub.projectId, actor);
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\?/, (route) => route.fulfill({ json: body }));
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\/seen$/, (route) => route.fulfill({ status: 204, body: "" }));
}

const colour = (locator) => locator.evaluate((el) => getComputedStyle(el).color);
const accent = (page) =>
  page.evaluate(() => {
    const probe = document.createElement("span");
    probe.style.color = "var(--accent)";
    document.body.append(probe);
    const said = getComputedStyle(probe).color;
    probe.remove();
    return said;
  });

test.use(pinnedFontOptions);

test.describe("a waiting row in the feed", () => {
  test.beforeEach(async ({ hub, page }) => {
    await pinFont(page);
    const held = (await api(hub, "GET", "/api/v1/inbox?status=action&limit=500")).json();
    if (!held.items.some((item) => item.summary === SUBJECT)) {
      expect(await oneOffQuestion(hub, hub.projectId, SUBJECT), "the waiting question was not posted").not.toBe("");
    }
  });

  test("reads its whole subject and says it is waiting, in copper under the sentence", async ({ hub, page }) => {
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      const what = `the ${scheme} theme`;
      const { index, link, row } = await waitingRow(page, hub);
      await expect.soft(link, `${what}: the link does not say the row is waiting`).toHaveAccessibleDescription("Waiting on you");
      await expect.soft(link, `${what}: the sentence does not name the agent and the ask`).toContainText(`asks: ${SUBJECT}`);
      const sentence = row.locator(".feed-s");
      const fits = await sentence.evaluate((el) => el.scrollWidth <= el.clientWidth + 0.5);
      expect.soft(fits, `${what}: the sentence is cut rather than wrapped`).toBe(true);
      const words = row.getByText("Waiting on you", { exact: true });
      await expect(words).toBeVisible();
      const below = await words.boundingBox();
      const above = await sentence.boundingBox();
      expect.soft(below.y, `${what}: the words share the sentence's line`).toBeGreaterThanOrEqual(above.y + above.height - 0.5);
      const copper = await accent(page);
      expect.soft(await colour(words), `${what}: the words are not in the action tone`).toBe(copper);
      expect.soft(await colour(row.locator(".feed-k .glyph")), `${what}: the glyph is not in the action tone`).toBe(copper);
      // Copper is for what needs the reader: no other row's glyph takes it.
      const others = index.locator(".feed-row:not(.is-waiting) .feed-k .glyph");
      for (const glyph of await others.all()) {
        expect.soft(await colour(glyph), `${what}: a row that does not wait draws its glyph in copper`).not.toBe(copper);
      }
    }
  });

  test("Answer opens the ask as an item", async ({ hub, page }) => {
    const { row } = await waitingRow(page, hub);
    const id = await row.getAttribute("data-id");
    await row.getByRole("link", { name: "Answer", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`#/inbox\\?open=${id}$`));
    await expect(page.locator(".inbox-detail")).toBeVisible();
  });

  test("passes the accessibility audit in both themes", async ({ hub, page }) => {
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      await waitingRow(page, hub);
      const results = await new AxeBuilder({ page }).include(".shell-index").withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"]).analyze();
      expect(
        results.violations.map((violation) => `${violation.id}: ${violation.help}`),
        `the feed in the ${scheme} theme has accessibility violations`,
      ).toEqual([]);
    }
  });

  test("matches its baselines @visual", async ({ hub, page }) => {
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      const { row } = await waitingRow(page, hub);
      await page.mouse.move(0, 0);
      await expect(row).toHaveScreenshot(`feed-waiting-row-${scheme}.png`, {
        ...SCREENSHOT,
        mask: [row.locator("time")],
      });
    }
  });
});

test.describe("a feed row", () => {
  test.beforeEach(async ({ page }) => {
    await pinFont(page);
  });

  test("is one sentence with the time on the right, unseen in ink and seen quiet", async ({ hub, page }) => {
    await fixedFeed(page, hub);
    const index = await openFeed(page, hub);
    const row = (summary) => index.locator(".feed-row").filter({ hasText: summary });
    await expect(row("Finished the nightly run")).toContainText("build-bot finished the nightly run");
    await expect(row("rel-1")).toContainText("build-bot started rel-1");
    await expect(row("Disk at 40%")).toContainText("build-bot noted: Disk at 40%");
    for (const unseen of ["Finished the nightly run", "Cache hit rate", "rel-1"]) {
      await expect(row(unseen).getByText("Unread", { exact: true }), `${unseen} is above the cursor`).toHaveCount(1);
    }
    for (const seen of ["Disk at 40%", "scored 4 of 5", "flaky-hunt"]) {
      await expect(row(seen).getByText("Unread", { exact: true }), `${seen} is at or below the cursor`).toHaveCount(0);
    }
    const weight = (summary) => row(summary).locator(".feed-s b").evaluate((el) => Number(getComputedStyle(el).fontWeight));
    expect(await weight("rel-1"), "an unseen row's agent is not heavier than a seen one's").toBeGreaterThan(await weight("Disk at 40%"));
    // The time sits right of the sentence, in the mono family, on the row's first line.
    const first = row("Cache hit rate");
    const time = first.locator(".feed-at time");
    await expect(time).toHaveText(/^\d\d:\d\d/);
    const [t, s] = [await time.boundingBox(), await first.locator(".feed-s").boundingBox()];
    expect(t.x, "the time is not right of the sentence").toBeGreaterThanOrEqual(s.x + s.width - 0.5);
    expect(Math.abs(t.y - s.y), "the time is not on the sentence's first line").toBeLessThan(8);
    expect(await time.evaluate((el) => getComputedStyle(el).fontFamily.toLowerCase())).toContain("mono");
  });

  test("a day heads its rows with its date in mono", async ({ hub, page }) => {
    await fixedFeed(page, hub);
    const index = await openFeed(page, hub);
    const today = await page.evaluate(() =>
      new Date().toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" }),
    );
    const head = index.getByRole("heading", { name: /^Today/ });
    await expect(head).toHaveText(`Today ${today}`, { ignoreCase: true });
    await expect(index.getByRole("heading", { name: /^Yesterday/ })).toBeVisible();
    const sub = head.getByText(today, { exact: true });
    expect(await sub.evaluate((el) => getComputedStyle(el).fontFamily.toLowerCase())).toContain("mono");
  });

  test("a long agent name wraps in the sentence and leaves the time in place", async ({ hub, page }) => {
    await fixedFeed(page, hub, LONG_ACTOR);
    const index = await openFeed(page, hub);
    for (const row of await index.locator(".feed-row").all()) {
      const held = await row.evaluate((el) => {
        const time = el.querySelector(".feed-at time").getBoundingClientRect();
        const box = el.getBoundingClientRect();
        const sentence = el.querySelector(".feed-s");
        return {
          inside: time.width > 0 && time.right <= box.right + 0.5,
          whole: sentence.scrollWidth <= sentence.clientWidth + 0.5,
        };
      });
      expect(held.inside, "a long name pushed the time out of the row").toBe(true);
      expect(held.whole, "a long name is cut rather than wrapped").toBe(true);
      await expect(row.locator(".feed-s")).toContainText(LONG_ACTOR);
    }
  });
});
