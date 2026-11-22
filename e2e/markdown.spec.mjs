// Rendered markdown keeps the document rules wherever an agent's text lands: a
// wiki page, an opened item's body and a comment. Each check reads what the
// browser drew, at both widths: a wide table or a long fenced line scrolls
// inside its own block and never takes the page sideways.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, newSession, structured, toolCall } from "./invariants.mjs";

const LONG_LINE = `const value = "${"x".repeat(240)}";`;
const COLUMNS = Array.from({ length: 10 }, (_, i) => `Column ${i + 1}`);
const TABLE = [
  `| ${COLUMNS.join(" | ")} |`,
  `| ${COLUMNS.map(() => "---").join(" | ")} |`,
  `| ${COLUMNS.map((_, i) => `cell ${i + 1}`).join(" | ")} |`,
].join("\n");

const DOCUMENT = [
  "# Runbook",
  "",
  "### Third level heading",
  "",
  "Some `inline code` in a sentence.",
  "",
  "```",
  LONG_LINE,
  "```",
  "",
  TABLE,
  "",
  "> A quoted line from the review.",
  "",
  "- [x] shipped",
  "- [ ] pending",
  "",
  "A list with a nested item.",
  "",
  "- parent",
  "  - child",
  "- sibling",
  "",
].join("\n");

// What the page's own width is held to: no horizontal scroll on the document.
async function expectNoSidewaysPage(page) {
  const widths = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    client: document.documentElement.clientWidth,
  }));
  expect(widths.scroll, "the page scrolls sideways").toBeLessThanOrEqual(widths.client);
}

// The document rules, read from the first rendered body under `root`.
async function expectDocumentRules(page, root) {
  const body = root;

  const pre = body.locator("pre").first();
  await expect(pre).toBeVisible();
  const surface2 = await page.evaluate(() => {
    const probe = document.createElement("div");
    probe.style.background = "var(--surface-2)";
    document.body.appendChild(probe);
    const colour = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return colour;
  });
  expect(await pre.evaluate((el) => getComputedStyle(el).backgroundColor), "fenced block background").toBe(surface2);
  const scrolls = await pre.evaluate((el) => ({ overflowX: getComputedStyle(el).overflowX, over: el.scrollWidth > el.clientWidth }));
  expect(scrolls.overflowX, "the fenced block does not scroll inside itself").toMatch(/auto|scroll/);
  expect(scrolls.over, "the long fenced line does not overflow its block").toBe(true);
  const preBox = await pre.boundingBox();
  const viewport = page.viewportSize().width;
  expect(preBox.x + preBox.width, "the fenced block reaches past the screen").toBeLessThanOrEqual(viewport + 1);

  const table = body.locator("table").first();
  const tableInfo = await table.evaluate((el) => ({
    overflowX: getComputedStyle(el).overflowX,
    cells: [...el.querySelectorAll("tbody td")].map((cell) => cell.getBoundingClientRect().width),
    sticky: getComputedStyle(el.querySelector("tbody td")).position,
    right: el.getBoundingClientRect().right,
  }));
  expect(tableInfo.overflowX, "the table does not scroll inside itself").toMatch(/auto|scroll/);
  expect(tableInfo.sticky, "the first column is not sticky").toBe("sticky");
  for (const width of tableInfo.cells) expect(width, "a cell is squeezed under 120px").toBeGreaterThanOrEqual(119);
  expect(tableInfo.right, "the table reaches past the screen").toBeLessThanOrEqual(viewport + 1);

  // The table really scrolls, and its first column stays where it was.
  const tableScroll = await table.evaluate(async (el) => {
    const first = el.querySelector("tbody td");
    const before = first.getBoundingClientRect().left;
    el.scrollLeft = el.scrollWidth;
    await new Promise((resolve) => requestAnimationFrame(resolve));
    return { over: el.scrollWidth > el.clientWidth, moved: el.scrollLeft > 0, before, after: first.getBoundingClientRect().left };
  });
  expect(tableScroll.over, "the table does not overflow its block").toBe(true);
  expect(tableScroll.moved, "the table did not scroll").toBe(true);
  expect(Math.abs(tableScroll.after - tableScroll.before), "the first column moved with the scroll").toBeLessThanOrEqual(1);

  const code = body.locator("p code").first();
  const chip = await code.evaluate((el) => {
    const style = getComputedStyle(el);
    return { background: style.backgroundColor, radius: parseFloat(style.borderTopLeftRadius), pad: parseFloat(style.paddingLeft) };
  });
  expect(chip.background, "inline code has no chip fill").toBe(surface2);
  expect(chip.radius).toBeGreaterThan(0);
  expect(chip.pad).toBeGreaterThan(0);

  const quote = await body.locator("blockquote").first().evaluate((el) => {
    const style = getComputedStyle(el);
    return { width: parseFloat(style.borderLeftWidth), colour: style.borderLeftColor, ink: style.color };
  });
  expect(quote.width, "the quote rule is not 2 to 3px").toBeGreaterThanOrEqual(2);
  expect(quote.width).toBeLessThanOrEqual(3);
  const ink2 = await page.evaluate(() => {
    const probe = document.createElement("div");
    probe.style.color = "var(--ink-2)";
    document.body.appendChild(probe);
    const colour = getComputedStyle(probe).color;
    probe.remove();
    return colour;
  });
  expect(quote.colour, "the quote rule is not --ink-2").toBe(ink2);

  const boxes = body.locator('input[type="checkbox"]');
  await expect(boxes).toHaveCount(2);
  await expect(boxes.nth(0)).toBeChecked();
  await expect(boxes.nth(1)).not.toBeChecked();
  await expect(boxes.nth(0)).toBeDisabled();
  const bullet = await boxes.first().evaluate((el) => getComputedStyle(el.closest("li")).listStyleType);
  expect(bullet, "a task item keeps its bullet").toBe("none");

  const sizes = await page.evaluate((el) => {
    const px = (node) => parseFloat(getComputedStyle(node).fontSize);
    return { h3: px(el.querySelector("h3")), body: px(el.querySelector("p")) };
  }, await body.elementHandle());
  expect(sizes.h3, "h3 is smaller than body text").toBeGreaterThanOrEqual(sizes.body);

  const gap = await body.evaluate((el) => {
    const nested = el.querySelector("li > ul");
    const list = nested.parentElement.parentElement;
    const parent = nested.parentElement;
    const sibling = parent.nextElementSibling;
    const margin = parseFloat(getComputedStyle(nested).marginBottom) + parseFloat(getComputedStyle(nested).marginTop);
    const between = sibling.getBoundingClientRect().top - nested.getBoundingClientRect().bottom;
    return { margin, between, tag: list.tagName };
  });
  expect(gap.margin, "a nested list carries a margin").toBe(0);
  expect(gap.between, "a blank line follows a nested list").toBeLessThanOrEqual(8);
}

test.describe("rendered markdown", () => {
  test("a wiki page carries the document rules", async ({ hub, page }, testInfo) => {
    const name = `markdown/rules-${testInfo.project.name}.md`;
    const project = encodeURIComponent(hub.projectId);
    await api(hub, "PUT", `/api/v1/projects/${project}/kb/pages/${name}`, {
      content: `---\ntype: Runbook\ntitle: Rules\n---\n${DOCUMENT}`,
    });
    await page.goto(`${hub.baseUrl}/#/projects/${project}/wiki?page=${encodeURIComponent(name)}`);
    const article = page.locator(".wiki-article");
    await expect(article.locator("pre")).toBeVisible();
    await expectDocumentRules(page, article);
    await expectNoSidewaysPage(page);
  });

  test("an opened item's body carries the document rules", async ({ hub, page }) => {
    const session = await newSession(hub);
    const subject = "markdown rules item";
    await toolCall(hub, session, "question_post", { project_id: hub.projectId, subject, body: DOCUMENT });
    let id = "";
    for (const status of ["action", "waiting"]) {
      const held = (await api(hub, "GET", `/api/v1/inbox?status=${status}&limit=500`)).json();
      const found = held.items.find((entry) => entry.summary === subject);
      if (found) id = found.event_id;
    }
    expect(id, "the item was not posted").toBeTruthy();
    try {
      await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`);
      const body = page.locator(".inbox-detail-body");
      await expect(body.locator("pre")).toBeVisible();
      await expectDocumentRules(page, body);
      await expectNoSidewaysPage(page);
    } finally {
      await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "done" }).catch(() => {});
    }
  });

  // The vertical gap between the line holding `first` and the line holding
  // `second` inside one element, against the height of a line.
  async function paragraphGap(locator, first, second) {
    return locator.evaluate(
      (el, words) => {
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        const text = walker.nextNode();
        const rects = words.map((word) => {
          const range = document.createRange();
          const at = text.data.indexOf(word);
          range.setStart(text, at);
          range.setEnd(text, at + word.length);
          return range.getBoundingClientRect();
        });
        return { gap: rects[1].top - rects[0].bottom, line: rects[0].height };
      },
      [first, second],
    );
  }

  test("an artifact comment keeps its paragraph break", async ({ hub, page }, testInfo) => {
    const session = await newSession(hub);
    const published = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "Comment paragraphs",
      kind: "markdown",
      content: "## Section\n\nA paragraph.",
    });
    const id = structured(published).artifact_id;
    await api(hub, "POST", `/api/v1/artifacts/${encodeURIComponent(id)}/comments`, {
      body: "First paragraph.\n\nSecond paragraph.",
      anchor_version: 1,
    });
    await page.goto(`${hub.baseUrl}/#/artifacts/${id}?project=${hub.projectId}`);
    // The desktop's stage head holds a toggle that may already be pressed; a
    // phone opens the sheet from the bar.
    const opener = page.getByRole("button", { name: testInfo.project.name === "desktop" ? /^Comments, / : /^Comments on v/ });
    await expect(opener).toBeVisible();
    if ((await opener.getAttribute("aria-pressed")) !== "true") await opener.click();
    // The desktop reads the card in the aside; a phone reads the list row in
    // the sheet. Both are the comment's own text.
    const body = page.locator(".hub-list-body, .hub-thread-body, .hub-card-body").filter({ hasText: "First paragraph.", visible: true });
    await expect(body).toBeVisible();
    const { gap, line } = await paragraphGap(body, "First", "Second");
    expect(gap, "the two paragraphs run together").toBeGreaterThanOrEqual(line * 0.9);
  });

  test("a wiki comment keeps its paragraph break", async ({ hub, page }, testInfo) => {
    const name = `markdown/comment-${testInfo.project.name}.md`;
    const project = encodeURIComponent(hub.projectId);
    await api(hub, "PUT", `/api/v1/projects/${project}/kb/pages/${name}`, { content: "# Note\n\nText.\n" });
    await api(hub, "POST", `/api/v1/projects/${project}/kb/comments`, {
      path: name,
      body: "First paragraph.\n\nSecond paragraph.",
    });
    await page.goto(`${hub.baseUrl}/#/projects/${project}/wiki?page=${encodeURIComponent(name)}`);
    const body = page.locator(".wiki-comment-body", { hasText: "First paragraph." });
    await expect(body).toBeVisible();
    const { gap, line } = await paragraphGap(body, "First", "Second");
    expect(gap, "the two paragraphs run together").toBeGreaterThanOrEqual(line * 0.9);
  });
});
