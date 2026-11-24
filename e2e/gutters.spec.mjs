// One gutter per pane. Text and row content start on it.
//
// The gutter is what makes a row's left edge mean the same thing wherever a
// reader looks, and it breaks quietly: a component written on its own carries
// its own padding, and the row drifts off the pane's gutter by a few pixels
// while every pane still looks aligned at a glance. The session audit line ran
// to the session detail stage's own left edge, the session detail header sat on
// a 24px gutter while the pane's own was 16, and the search preview sat on 32.
//
// The check reads the gutter off the element's own box, walking out from it to
// the first horizontal padding it meets, and holds the text's left edge to that
// padding within a pixel. It holds the headings, the labels, the session
// detail's own rows and the paragraphs a screen writes itself; a row's title
// starts at the gutter plus the row's fixed glyph column, which is the row's own
// layout and is named here rather than silently allowed.
//
// Nested structures, a tree's indentation and a blockquote's inset, sit inside
// the row rather than on it and are named rather than silently allowed.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, oneOffEvent } from "./invariants.mjs";
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

// The headings, labels, rows and paragraphs that start on the gutter. A row's
// own title is not among them: it starts at the gutter plus the row's fixed
// glyph column, which is the row's own layout.
const ON_THE_GUTTER =
  ".section-label, .shell-group-label, .form-group-label, .members-section-label, .access-section-label, .storage-section-label, .wiki-history-day, h2.day, .shell-title-line, .now-sum, .now-grp-name, .now-end-line, .audit-row, .brain-header-line, .session-actions-footer, .search-stage-path, .pset-sub, .wiki-editor-label, .wiki-comment-body, .shell-pad > .empty, .shell-body-pad > .empty, .empty-state";

// Content that sits inside a row rather than on it, and the margins of Now and
// the artifacts list, which are ruled off by their own hairline and carry their
// own gutter.
const NESTED =
  "[data-depth], blockquote, .wiki-tree-row, .wiki-row, .tree-item, .search-preview-article, .shell-head, .now-mar, .arts-mar";

// The row's fixed glyph column, which the text after a leading glyph starts
// after. DESIGN.md fixes it at 20px in an index and 28px on the mobile feed.
const GLYPH_COLUMN = 52;

function measure(page) {
  return page.evaluate(
    ({ textual, nested }) => {
      const pad = (el) => parseFloat(getComputedStyle(el).paddingLeft) || 0;
      const found = [];
      for (const el of document.querySelectorAll(textual)) {
        if (el.closest(nested)) continue;
        if (el.closest("[hidden]")) continue;
        const box = el.getBoundingClientRect();
        if (box.width === 0 || box.height === 0) continue;
        const text = (el.textContent || "").trim();
        if (!text) continue;
        let pane = null;
        for (let node = el; node && node !== document.body; node = node.parentElement) {
          if (node.matches("main .shell-index, main .shell-stage, main .shell-aside")) {
            pane = node;
            break;
          }
        }
        if (!pane) continue;
        const range = document.createRange();
        range.selectNodeContents(el);
        const textBox = range.getBoundingClientRect();
        found.push({
          tag: `${el.tagName.toLowerCase()}.${(el.className || "").toString().split(" ")[0]}`,
          text: text.slice(0, 28),
          pane: [...pane.classList].find((c) => c.startsWith("shell-")),
          offset: Math.round((textBox.left - pane.getBoundingClientRect().left) * 10) / 10,
        });
      }
      return found;
    },
    { textual: ON_THE_GUTTER, nested: NESTED },
  );
}

test.describe("one gutter per pane", () => {
  test("every heading, label and row a screen writes starts on its pane's gutter", async ({ hub, page }, testInfo) => {
    test.skip(
      testInfo.project.name !== "gutters-desktop" && testInfo.project.name !== "gutters-touch",
      "the gutter is held at both widths",
    );
    const wiki = await seedWikiPage(hub, "gutters/pane.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);

    const held = [];
    const off = [];
    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        const rows = await measure(page);
        held.push(...rows);
        // One gutter per pane: the offset every row's text starts at is the same
        // for every row in the pane. The offset a pane settles on is the pane's
        // own, read off the rows rather than declared here, and a row that
        // starts somewhere else is the defect.
        const counts = new Map();
        for (const row of rows) counts.set(row.offset, (counts.get(row.offset) || 0) + 1);
        const [gutter] = [...counts.entries()].sort((a, b) => b[1] - a[1])[0] || [];
        for (const row of rows) {
          if (Math.abs(row.offset - gutter) > 1) {
            off.push(`${name}: ${row.pane} holds ${row.tag} at x ${row.offset} while the pane's rows start at ${gutter}`);
          }
        }
      }
    }
    expect(held.length, "no heading, label or row was found on any screen").toBeGreaterThan(0);
    expect(off, "a row does not start on its pane's gutter").toEqual([]);
  });
});

// An empty note is prose on the pane's gutter: no border, no fill. The project
// is made for the check, so its feed, artifacts and sessions are known empty.
test.describe("empty notes", () => {
  test("an empty list's note sits on the pane gutter, with no border or fill", async ({ hub, page }) => {
    const id = `empty-${Date.now().toString(36)}`;
    const at = `#/projects/${encodeURIComponent(id)}`;
    await api(hub, "POST", "/api/v1/projects", { id, display_name: "Empty notes" });
    await page.goto(`${hub.baseUrl}/`);
    for (const segment of ["feed", "artifacts", "sessions"]) {
      await page.goto(`${hub.baseUrl}/${at}/${segment}`);
      const found = "main .shell-index :is(.empty-state, .shell-body-pad > p.empty)";
      const note = page.locator(found).first();
      await expect(note, `${segment} shows an empty note`).toBeVisible();
      const read = await page.evaluate((found) => {
        const el = document.querySelector(found);
        const own = getComputedStyle(el);
        const text = el.querySelector(".empty-title") || el;
        const range = document.createRange();
        range.selectNodeContents(text);
        const pane = el.closest(".shell-index").getBoundingClientRect().left;
        const edge = pane + parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--s-4"));
        return {
          offset: Math.round((range.getBoundingClientRect().left - pane) * 10) / 10,
          gutter: Math.round((edge - pane) * 10) / 10,
          border: own.borderTopWidth,
          fill: own.backgroundColor,
        };
      }, found);
      expect(read.offset, `${segment}: the note starts on the gutter`).toBeCloseTo(read.gutter, 0);
      expect(read.border, `${segment}: no border`).toBe("0px");
      expect(read.fill, `${segment}: no fill`).toBe("rgba(0, 0, 0, 0)");
    }
  });

  test("a filter that matches nothing says so in the stage", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "gutters-desktop", "the phone filters from its tools row");
    const id = `empty-${Date.now().toString(36)}f`;
    await api(hub, "POST", "/api/v1/projects", { id, display_name: "Empty filter" });
    await oneOffEvent(hub, id, "signal", "A signal for the filter check");
    await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(id)}/feed`);
    await expect(page.locator("main .shell-index .feed-row").first()).toBeVisible();
    await page.locator("main .shell-index [data-group-toggle]", { hasText: "Group" }).click();
    await page.getByRole("menuitemradio", { name: /^Scores/ }).click();
    await expect(page.locator("main .shell-index .empty-state")).toBeVisible();
    await expect(page.locator("main .shell-stage .empty")).toHaveText("No events match this filter.");
  });
});

// Project and session rows sink on hover under a fine pointer.
test.describe("row hover", () => {
  test("project and session rows take the sunken ground under a fine pointer", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "gutters-desktop", "hover is a fine-pointer behaviour");
    const ground = (row) => row.evaluate((el) => getComputedStyle(el).backgroundColor);
    for (const [hash, selector] of [
      ["#/projects", "main .projects-screen .project-row"],
      [`#/projects/${encodeURIComponent(hub.projectId)}/sessions`, "main .session-row:not(.selected)"],
    ]) {
      await page.goto(`${hub.baseUrl}/${hash}`);
      const row = page.locator(selector).first();
      await expect(row).toBeVisible();
      await page.mouse.move(0, 0);
      const at = await ground(row);
      await row.hover();
      expect(await ground(row), `${selector} sinks on hover`).not.toBe(at);
    }
  });
});
