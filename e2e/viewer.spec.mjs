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
