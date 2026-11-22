// The artifact's own surface, the project's Artifacts stage: where its actions
// menu opens, and the number of headers and ways out its compose sheet offers.
// The stage is reached both by the artifact's own address and by picking a row
// in the index, so the same geometry is asked of both ways in.
//
// The menu is a position rather than a value, so it is read as geometry: it
// must begin under its trigger and share the trigger's right edge. Anchored to
// a box spanning the index, the stage and the comments column, it opened over
// the comments column several hundred pixels to the right of the button. The
// bounding box is the standard tool for a position and a redesign that keeps
// the menu on its trigger passes.
//
// The compose sheet is opened from the viewer's own control, so both widths are
// asked: the drawer mounts at every width and the sheet is what it becomes.

import { expect } from "@playwright/test";
import { open, test } from "./app.mjs";
import { api, newSession, structured, toolCall } from "./invariants.mjs";

// One artifact opened by its own address, the one a feed link and a share link
// both reach, which lands on the project's stage with it selected.
async function openArtifact(page, hub) {
  await page.goto(`${hub.baseUrl}/#/artifacts/${hub.artifactId}?project=${hub.projectId}`);
  await expect(page.locator("#hub-frame")).toBeVisible();
}

// A document long enough to scroll, with one open thread, so the viewer paints
// its comments bar. Published through the agent surface an agent would use,
// because no seeded artifact is both long and commented. It is left on the
// project's hub, and viewer.spec is the last spec to run against that hub.
async function longCommentedArtifact(hub) {
  const session = await newSession(hub);
  const content = Array.from(
    { length: 40 },
    (_, i) => `## Section ${i}\n\nA paragraph long enough that the document runs well past one viewport.`,
  ).join("\n\n");
  const published = await toolCall(hub, session, "artifact_publish", {
    project_id: hub.projectId,
    title: "Scroll check",
    kind: "markdown",
    content,
  });
  const id = structured(published).artifact_id;
  await api(hub, "POST", `/api/v1/artifacts/${encodeURIComponent(id)}/comments`, {
    body: "A note on the design.",
    anchor_version: 1,
  });
  return id;
}

test.describe("the artifact viewer's actions", () => {
  test("the overflow menu hangs from its trigger, not the viewer's edge", async ({ hub, page }) => {
    await openArtifact(page, hub);
    const trigger = page.getByRole("button", { name: "More" });
    await expect(trigger).toBeVisible();
    await trigger.click();
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();

    const buttonBox = await trigger.boundingBox();
    const menuBox = await menu.boundingBox();
    expect(
      menuBox.y,
      "the menu opened above its trigger",
    ).toBeGreaterThanOrEqual(buttonBox.y);
    expect(
      Math.abs(menuBox.x + menuBox.width - (buttonBox.x + buttonBox.width)),
      "the menu did not share its trigger's right edge",
    ).toBeLessThanOrEqual(16);
  });

  test("the compose sheet offers one header and one way out", async ({ hub, page }) => {
    await openArtifact(page, hub);
    await page.getByRole("button", { name: "Start a thread" }).click();
    const sheet = page.getByRole("dialog");
    await expect(sheet).toBeVisible();
    // The drawer's own "Comments" head belongs to the list. Stacked above the
    // compose head it drew two titles and two close buttons in one sheet.
    await expect(page.getByRole("heading", { name: "Comments" })).toBeHidden();
    await expect(sheet.getByRole("button", { name: /^Close/ })).toHaveCount(1);
  });

  test("a comment on a repeated word lands where it was written", async ({ hub, page }, testInfo) => {
    // The anchor stored only the quote and the frame took its first match, so a
    // comment on the second "quality" highlighted the first one instead. The
    // selection now carries the text around it, and the frame resolves the
    // occurrence whose context matches.
    test.skip(testInfo.project.name !== "desktop", "the callout is a fine-pointer surface");
    const session = await newSession(hub);
    const published = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "Repeated word",
      kind: "markdown",
      content:
        "## First section\n\nThe first card values latency over quality today.\n\n" +
        "## Second section\n\nThe profile objective quality wins on card 86.",
    });
    const id = structured(published).artifact_id;
    await page.goto(`${hub.baseUrl}/#/artifacts/${id}?project=${hub.projectId}`);
    await expect(page.locator("#hub-frame")).toBeVisible();

    const document = page.frameLocator("#hub-frame").frameLocator("#hub-frame");
    await document.locator("body").first().waitFor();

    // Select the word "quality" in the second section together with the spaces
    // a drag tends to catch, so the context boundary has to line up with the
    // trimmed quote rather than sit one space off.
    await document
      .locator("p", { hasText: "profile objective" })
      .evaluate((paragraph) => {
        const walker = document.createTreeWalker(paragraph, NodeFilter.SHOW_TEXT);
        let node = walker.nextNode();
        while (node && node.nodeValue.indexOf("quality") === -1) node = walker.nextNode();
        const start = node.nodeValue.indexOf("quality") - 1;
        const range = document.createRange();
        range.setStart(node, start);
        range.setEnd(node, start + "quality".length + 2);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        document.dispatchEvent(new Event("selectionchange"));
      });

    await document.getByRole("button", { name: "Comment on selection" }).click();
    const sheet = page.locator(".comments-drawer.hub-comment-sheet:not([hidden])");
    await expect(sheet).toBeVisible();
    await sheet.locator("textarea").first().fill("the second one");
    await sheet.getByRole("button", { name: "Post", exact: true }).click();
    await expect(sheet).toHaveCount(0);

    // The painted highlight sits in the second section, not the first.
    const painted = await document
      .locator(".hub-comment-highlight")
      .evaluate((el) => (el.closest("p") ? el.closest("p").textContent : ""));
    expect(painted, "the highlight landed on the wrong occurrence").toContain("profile objective");
  });
});

test.describe("the project artifact stage's actions", () => {
  test("the overflow menu hangs from its trigger and carries the stage's actions", async ({
    hub,
    page,
  }, testInfo) => {
    await open(page, hub, `#/projects/${hub.projectId}/artifacts`, "artifacts");
    // On a desktop the stage is already beside the list. A phone shows one zone
    // at a time, so the list holds the screen until a row is picked, and the
    // plain artifact is the row a reader opens.
    if (testInfo.project.name === "phone") {
      await page.locator(`.artifact-row[data-id="${hub.artifactId}"] a`).click();
    }
    const stage = page.locator(".shell-stage");
    const trigger = stage.getByRole("button", { name: "More" });
    await expect(trigger).toBeVisible();
    await trigger.click();
    const menu = stage.getByRole("menu");
    await expect(menu).toBeVisible();
    await expect(menu.getByRole("menuitem", { name: "Copy path" })).toBeVisible();

    const buttonBox = await trigger.boundingBox();
    const menuBox = await menu.boundingBox();
    expect(
      menuBox.y,
      "the menu opened above its trigger",
    ).toBeGreaterThanOrEqual(buttonBox.y);
    expect(
      Math.abs(menuBox.x + menuBox.width - (buttonBox.x + buttonBox.width)),
      "the menu did not share its trigger's right edge",
    ).toBeLessThanOrEqual(16);

    // The item is not drawn dead: Copy path answers with its own toast.
    await menu.getByRole("menuitem", { name: "Copy path" }).click();
    await expect(page.getByText("Path copied")).toBeVisible();
  });

  test("commenting on selected text opens the composer", async ({ hub, page }, testInfo) => {
    // A desktop stage draws the selection callout in the frame. The callout
    // posted a message the shell could not answer on a fine pointer, because the
    // comments drawer the composer renders into was not mounted there, so
    // commenting on selected text did nothing.
    test.skip(testInfo.project.name !== "desktop", "the callout is a fine-pointer surface");
    await open(page, hub, `#/projects/${hub.projectId}/artifacts`, "artifacts");
    const document = page.frameLocator("#hub-frame").frameLocator("#hub-frame");
    await document.locator("body").first().waitFor();
    await document.locator("body").first().selectText();
    const callout = document.getByRole("button", { name: "Comment on selection" });
    await expect(callout).toBeVisible();
    await callout.click();
    const sheet = page.locator(".comments-drawer.hub-comment-sheet:not([hidden])");
    await expect(sheet).toBeVisible();
    await expect(sheet.locator(".hub-sheet-title")).toHaveText("New Comment");
  });

  test("a comment posted from the stage saves without an artifact param", async ({
    hub,
    page,
  }, testInfo) => {
    // The stage shows the first artifact when the URL names none, but the comment
    // state was seeded from the URL param, which is empty there, so the post went
    // to /api/v1/artifacts//comments and the hub answered "artifact not found".
    test.skip(testInfo.project.name !== "desktop", "the callout is a fine-pointer surface");
    await open(page, hub, `#/projects/${hub.projectId}/artifacts`, "artifacts");
    const document = page.frameLocator("#hub-frame").frameLocator("#hub-frame");
    await document.locator("body").first().waitFor();
    await document.locator("body").first().selectText();
    await document.getByRole("button", { name: "Comment on selection" }).click();
    const sheet = page.locator(".comments-drawer.hub-comment-sheet:not([hidden])");
    await expect(sheet).toBeVisible();
    await sheet.locator("textarea").first().fill("a stage comment");
    await sheet.getByRole("button", { name: "Post", exact: true }).click();
    // The sheet closes on success, and the comment lands on the artifact.
    await expect(sheet).toHaveCount(0);
    await expect(page.locator(".hub-comment-card").filter({ hasText: "a stage comment" })).toBeVisible();
  });

  // A reply on the same passage is part of its thread, so the thread counts
  // once and Resolve closes the whole thread. Resolving only the root left the
  // reply behind as a thread of its own.
  test("resolving a thread resolves its replies and counts as one", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the thread cards are the desktop's comments column");
    const session = await newSession(hub);
    const published = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "Thread check",
      kind: "markdown",
      content: "## Plan\n\nRoll the log files every night.",
    });
    const id = structured(published).artifact_id;
    const anchor = { mode: "text", quote: "Roll the log files" };
    for (const body of ["Why nightly?", "Because of disk size."]) {
      await api(hub, "POST", `/api/v1/artifacts/${encodeURIComponent(id)}/comments`, {
        body,
        anchor,
        anchor_version: 1,
      });
    }

    await page.goto(`${hub.baseUrl}/#/artifacts/${id}?project=${hub.projectId}`);
    const card = page.locator(".hub-comment-card").filter({ hasText: "Why nightly?" });
    await expect(card).toContainText("Because of disk size.");
    await expect(page.locator(".hub-comments-head-meta")).toHaveText("1 · v1");

    await card.getByRole("button", { name: "Resolve" }).click();
    await expect(page.locator(".hub-comments-head-meta")).toHaveText("0 · v1");
    const listed = await api(hub, "GET", `/api/v1/artifacts/${encodeURIComponent(id)}/comments`);
    const body = listed.json();
    const comments = Array.isArray(body) ? body : body.comments;
    expect(comments.map((comment) => comment.done)).toEqual([true, true]);
  });
});

test.describe("the artifact viewer's comments bar", () => {
  // The bar is chrome, not the document's last paragraph: it holds the bottom
  // of the reader's viewport while the document scrolls under it. Built at the
  // end of the document instead, it floated mid-page and then left the viewport,
  // which is what the wheel is here to catch. The gap is read as geometry in the
  // reader's own viewport, so the fixed tab bar a phone carries is already
  // accounted for and the same expectation holds at both widths.
  test("the comments bar holds the bottom of the viewport while the document scrolls", async ({
    hub,
    page,
  }) => {
    const id = await longCommentedArtifact(hub);
    await page.goto(`${hub.baseUrl}/#/artifacts/${id}?project=${hub.projectId}`);
    await expect(page.locator("#hub-frame")).toBeVisible();
    const bar = page.locator(".hub-comments-strip");
    await expect(bar).toBeVisible();

    // The frame reports its own height, so the document is short until that
    // message lands. Wait for it, or the bar is measured against a page with
    // nothing to scroll.
    await page.waitForFunction(() => {
      const content = document.querySelector(".shell-stage .shell-body");
      const stage = document.querySelector(".shell-stage");
      const tall = Math.max(
        document.documentElement.scrollHeight,
        content ? content.scrollHeight : 0,
        stage ? stage.scrollHeight : 0,
      );
      return tall > window.innerHeight * 2;
    });

    const gap = () =>
      page.evaluate(() => {
        const strip = document.querySelector(".hub-comments-strip");
        return Math.round(window.innerHeight - strip.getBoundingClientRect().bottom);
      });
    // How far the document has scrolled, whichever box is its scrollport: the
    // stage's content on a desktop, the page on a phone.
    const scrollPos = () =>
      page.evaluate(() => {
        const content = document.querySelector(".shell-stage .shell-body");
        const stage = document.querySelector(".shell-stage");
        return Math.max(window.scrollY, content.scrollTop, stage.scrollTop);
      });
    // Zero is flush with the viewport bottom. The ceiling is the phone's 60px
    // tab bar plus the trailing padding the bar settles over at the very end of
    // a page-scrolled document (measured 124 on a phone, 0 on a desktop); it
    // should tighten when that end settle is removed.
    const ceiling = 140;
    const held = async (where) => {
      const distance = await gap();
      expect(distance, `${where}: the bar is under the viewport bottom`).toBeGreaterThanOrEqual(0);
      expect(distance, `${where}: the bar is above the viewport bottom`).toBeLessThanOrEqual(ceiling);
    };

    await held("at rest");

    await page.locator(".shell-stage .shell-body").hover();
    const start = await scrollPos();
    await page.mouse.wheel(0, 1200);
    await page.waitForFunction((from) => {
      const content = document.querySelector(".shell-stage .shell-body");
      const stage = document.querySelector(".shell-stage");
      return Math.max(window.scrollY, content.scrollTop, stage.scrollTop) > from;
    }, start);
    await held("mid-document");

    // And at the end, where a bar built into the document would have settled.
    await page.mouse.wheel(0, 100000);
    await page.waitForFunction(() => {
      const content = document.querySelector(".shell-stage .shell-body");
      const stage = document.querySelector(".shell-stage");
      const end = Math.max(
        document.documentElement.scrollHeight - window.innerHeight,
        content.scrollHeight - content.clientHeight,
        stage.scrollHeight - stage.clientHeight,
      );
      const pos = Math.max(window.scrollY, content.scrollTop, stage.scrollTop);
      return pos >= end - 2;
    });
    await held("at the end");
  });
});

test.describe("the artifact frame's layout", () => {
  const rows = Array.from({ length: 3 }, (_, i) => `| row ${i} | ${"cell ".repeat(6)} | ${"wide ".repeat(8)} | more ${i} | last ${i} |`);
  const content = [
    `# Layout`,
    "",
    "x".repeat(140),
    "",
    "| Name of the record that this first column heading describes at length | Description | Another long column | Fourth | Fifth |",
    "| --- | --- | --- | --- | --- |",
    ...rows,
    "",
    "- outer",
    "  - inner one",
    "  - inner two",
    "",
    "- [ ] open task",
    "- [x] done task",
  ].join("\n");

  for (const [name, size] of [
    ["phone", { width: 390, height: 800 }],
    ["desktop", { width: 1440, height: 900 }],
  ]) {
    test(`wraps, scrolls and pins inside the frame at ${name} width`, async ({ hub, page }) => {
      await page.setViewportSize(size);
      const session = await newSession(hub);
      const published = await toolCall(hub, session, "artifact_publish", {
        project_id: hub.projectId,
        title: `Layout ${name}`,
        kind: "markdown",
        content,
      });
      const id = structured(published).artifact_id;
      await page.goto(`${hub.baseUrl}/#/artifacts/${id}?project=${hub.projectId}`);
      await expect(page.locator("#hub-frame")).toBeVisible();
      const document = page.frameLocator("#hub-frame").frameLocator("#hub-frame");
      await expect(document.locator("table")).toBeVisible();

      const root = document.locator("html");
      const [scrollWidth, clientWidth] = await root.evaluate((el) => [el.scrollWidth, el.clientWidth]);
      expect(scrollWidth, "the frame scrolls sideways").toBeLessThanOrEqual(clientWidth);

      // The first column is pinned but capped, so a long first heading wraps
      // inside it and leaves room for the columns it scrolls over.
      const table = document.locator("table");
      const [firstWidth, tableWidth] = await table.evaluate((el) => [
        el.querySelector("th:first-child").getBoundingClientRect().width,
        el.getBoundingClientRect().width,
      ]);
      expect(firstWidth, "the pinned first column covers the table").toBeLessThanOrEqual(tableWidth * 0.5);
      const overflow = await table.evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow, "the wide table does not scroll").toBeGreaterThan(0);
      const geometry = () =>
        table.evaluate((el) => {
          const box = el.getBoundingClientRect();
          const first = el.querySelector("th:first-child").getBoundingClientRect();
          const last = el.querySelector("th:last-child").getBoundingClientRect();
          return { first: first.left - box.left, lastRight: last.right - box.right };
        });
      const before = await geometry();
      expect(before.lastRight, "the last column starts visible").toBeGreaterThan(0);
      await table.evaluate((el) => {
        el.scrollLeft = el.scrollWidth;
      });
      const after = await geometry();
      expect(after.lastRight, "the last column does not scroll into view").toBeLessThanOrEqual(1);
      expect(Math.abs(after.first - before.first), "the first column does not stay pinned").toBeLessThan(2);

      expect(await document.locator("th:first-child").first().evaluate((el) => getComputedStyle(el).position)).toBe("sticky");
      expect(await document.locator("th:nth-child(2)").evaluate((el) => getComputedStyle(el).whiteSpace)).toBe("nowrap");
      expect(await document.locator("td").first().evaluate((el) => parseFloat(getComputedStyle(el).minWidth))).toBeGreaterThan(0);

      const gap = await document.locator("li > ul").first().evaluate((el) => getComputedStyle(el).marginBottom);
      expect(gap, "a nested list keeps the paragraph gap").toBe("0px");
      const marker = await document
        .locator("li:has(input[type=checkbox])")
        .first()
        .evaluate((el) => getComputedStyle(el).listStyleType);
      expect(marker, "a task item shows a bullet beside its box").toBe("none");
    });
  }

  for (const [name, size] of [
    ["phone", { width: 390, height: 800 }],
    ["desktop", { width: 1440, height: 900 }],
  ]) {
    test(`the password field's focus ring is not clipped at ${name} width`, async ({ hub, page }) => {
      await page.setViewportSize(size);
      await page.goto(`${hub.baseUrl}/#/artifacts/${encodeURIComponent(hub.protectedId)}?project=${encodeURIComponent(hub.projectId)}`);
      const gate = page.frameLocator("#hub-frame");
      const field = gate.locator("#hub-password");
      await expect(field).toBeVisible({ timeout: 10_000 });
      await field.focus();
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.press("Tab");
      await expect(field).toBeFocused();
      const slack = await field.evaluate((el) => {
        const ring = 4;
        const box = el.getBoundingClientRect();
        let left = 0;
        let right = el.ownerDocument.documentElement.clientWidth;
        for (let node = el.parentElement; node; node = node.parentElement) {
          if (getComputedStyle(node).overflowX === "visible") continue;
          const r = node.getBoundingClientRect();
          left = Math.max(left, r.left);
          right = Math.min(right, r.right);
        }
        return { left: box.left - ring - left, right: right - (box.right + ring) };
      });
      expect(slack.left, "the ring is clipped on the left").toBeGreaterThanOrEqual(0);
      expect(slack.right, "the ring is clipped on the right").toBeGreaterThanOrEqual(0);
    });
  }

  test("an unlocked artifact hides the lock tile and the heading", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/#/artifacts/${encodeURIComponent(hub.protectedId)}?project=${encodeURIComponent(hub.projectId)}`);
    const gate = page.frameLocator("#hub-frame");
    await expect(gate.getByRole("heading", { name: "Encrypted artifact" })).toBeVisible({ timeout: 10_000 });
    await gate.locator("#hub-password").fill(hub.fixture.protectedPassword);
    await gate.locator('#hub-unlock-form button[type="submit"]').click();
    await expect(gate.frameLocator("#hub-frame").getByText(hub.fixture.protectedBody)).toBeVisible({ timeout: 15_000 });
    await expect(gate.getByRole("heading", { name: "Encrypted artifact" })).toBeHidden();
    await expect(gate.locator(".hub-lock-tile")).toBeHidden();
  });
});
