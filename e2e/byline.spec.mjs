// A byline is a subject and receipts: the session in 600, then `agent · member`
// in mono. This holds it for each kind of author on a feed answered from a fixed
// page, so the words are the author record's and nothing is written to the hub
// the other checks share, and holds that at a 360px phone a long subject wraps
// whole rather than being clipped, and nothing is pushed out of the row.
//
// The type is pinned (see pinned-font.mjs) for the screenshot, which is only
// comparable between hosts that set the same face.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { fontsSettled, pinFont, pinnedFontOptions, SCREENSHOT } from "./pinned-font.mjs";

const NOW = Date.now();
const ago = (minutes) => new Date(NOW - minutes * 60000).toISOString();

const participant = (agent, member, session) => ({
  kind: "participant",
  member,
  agent,
  session: session ? { id: `s-${session}`, name: session } : null,
});

// One event per kind of author, newest first, all today.
const ROWS = [
  { n: 7, author: { kind: "legacy", label: "laptop/claude" }, origin: null, summary: "Session old-run ended", kind: "session", words: "laptop/claude", receipts: "before members" },
  { n: 6, author: { kind: "local", agent: "claude-code", session: { id: "s-scratch", name: "scratch" } }, origin: null, summary: "Cache hit rate 94% on the arm64 runner.", kind: "signal", words: "scratch", receipts: "claude-code · local" },
  { n: 5, author: { kind: "human" }, origin: "hub", summary: "Finished the nightly run", kind: "finished", words: "the hub", receipts: "" },
  { n: 4, author: { kind: "human" }, origin: null, summary: "Finished the audit", kind: "finished", words: "you", receipts: "" },
  { n: 3, author: participant("opencode", "desktop", null), origin: null, summary: "Disk at 40%", kind: "signal", words: "opencode", receipts: "desktop" },
  { n: 2, author: participant("claude-code", "laptop", "release-coordinator-for-the-edge-hosts"), origin: null, summary: "Session rel-1 started", kind: "session", words: "release-coordinator-for-the-edge-hosts", receipts: "claude-code · laptop" },
  { n: 1, author: participant("claude-code", "laptop", "feat-x"), origin: null, summary: "The nightly run scored 4 of 5", kind: "finished", words: "feat-x", receipts: "claude-code · laptop" },
];

const page_ = (project) => ({
  events: ROWS.map((row) => ({
    id: `e000${row.n}`,
    project_id: project,
    kind: row.kind,
    actor: "opaque-key",
    author: row.author,
    origin: row.origin,
    summary: row.summary,
    payload: null,
    thread_id: null,
    needs_action: false,
    created_at: ago(row.n),
    inbox_status: null,
  })),
  last_seen: "e0007",
  next_before: null,
});

async function openBylines(page, hub) {
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\?/, (route) => route.fulfill({ json: page_(hub.projectId) }));
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\/seen$/, (route) => route.fulfill({ status: 204, body: "" }));
  // Two members, so receipts name the member rather than leaving it out.
  await page.route(/\/api\/v1\/members$/, (route) =>
    route.fulfill({ json: { members: [{ id: "laptop", state: "active" }, { id: "desktop", state: "active" }] } }),
  );
  await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/feed`);
  await fontsSettled(page);
  const rows = page.locator(".shell-index .feed-row");
  await expect(rows).toHaveCount(ROWS.length);
  return rows;
}

test.use(pinnedFontOptions);

test.describe("a byline at 360px", () => {
  test.beforeEach(async ({ page }) => {
    await pinFont(page);
    await page.setViewportSize({ width: 360, height: 800 });
  });

  test("names the subject and the receipts for each kind of author", async ({ hub, page }) => {
    const rows = await openBylines(page, hub);
    for (const [at, want] of ROWS.entries()) {
      const row = rows.nth(at);
      await expect(row.locator(".by-s"), `row ${at}: the subject`).toHaveText(want.words);
      if (want.receipts) await expect(row.locator(".by-r"), `row ${at}: the receipts`).toHaveText(want.receipts);
      else await expect(row.locator(".by-r"), `row ${at}: receipts where there are none`).toHaveCount(0);
    }
    // A row from before members says so in its receipts, where a reader sees it.
    await expect(rows.nth(0).locator(".by-r")).toHaveText("before members");
    await expect(rows.nth(0).locator(".by-s")).not.toHaveAttribute("title", /.+/);
    // The wire's opaque actor key is never drawn.
    await expect(page.locator(".shell-index")).not.toContainText("opaque-key");
  });

  test("wraps a long subject whole and keeps everything inside the row", async ({ hub, page }) => {
    const rows = await openBylines(page, hub);
    for (const row of await rows.all()) {
      const held = await row.evaluate((el) => {
        const subject = el.querySelector(".by-s");
        const time = el.querySelector(".feed-at time").getBoundingClientRect();
        const box = el.getBoundingClientRect();
        const rects = [...subject.getClientRects()];
        return {
          words: subject.textContent,
          cut: getComputedStyle(subject).textOverflow !== "clip",
          clipped: rects.some((r) => r.left < box.left - 0.5 || r.right > box.right + 0.5),
          hidden: subject.scrollWidth > subject.getBoundingClientRect().width + 1 && getComputedStyle(subject).overflow !== "visible",
          inside: time.width > 0 && time.right <= box.right + 0.5,
          fits: el.scrollWidth <= el.clientWidth + 0.5,
        };
      });
      expect(held.cut, `${held.words}: the subject is cut with an ellipsis`).toBe(false);
      expect(held.clipped, `${held.words}: the subject runs past the row`).toBe(false);
      expect(held.hidden, `${held.words}: the subject is clipped`).toBe(false);
      expect(held.inside, "the time was pushed out of the row").toBe(true);
      expect(held.fits, "the row scrolls sideways").toBe(true);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 0.5), "the page scrolls sideways").toBe(true);
  });

  test("matches its baselines @visual", async ({ hub, page }) => {
    for (const scheme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: scheme });
      const rows = await openBylines(page, hub);
      await page.mouse.move(0, 0);
      await expect(page.locator(".shell-index .feed-day").first()).toHaveScreenshot(`byline-360-${scheme}.png`, {
        ...SCREENSHOT,
        mask: [rows.locator("time")],
      });
    }
  });
});
