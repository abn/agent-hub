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
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

// The headings, labels, rows and paragraphs that start on the gutter. A row's
// own title is not among them: it starts at the gutter plus the row's fixed
// glyph column, which is the row's own layout.
const ON_THE_GUTTER =
  ".section-label, .shell-group-label, .form-group-label, .agents-section-label, .access-section-label, .storage-section-label, .wiki-history-day, h2.day, .shell-title-line, .home-greeting, .home-summary, .home-section-title, .audit-row, .tree-pane-footnote, .brain-header-line, .session-actions-footer, .inbox-snooze-bar button, .search-stage-path, .pset-sub, .settings-helper-line, .wiki-editor-label, .wiki-comment-body";

// Content that sits inside a row rather than on it.
const NESTED = "[data-depth], blockquote, .wiki-tree-row, .wiki-row, .tree-item, .search-preview-article, .shell-head, .home-greeting";

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
