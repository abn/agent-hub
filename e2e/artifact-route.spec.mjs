// An artifact has one surface: its project's Artifacts shell with it selected.
// The address a feed row, a search hit and an older link carry,
// `#/artifacts/<id>`, is replaced by that shell's own address, so the link
// lands on the artifact selected in the stage, Back leaves the artifact rather
// than returning to a hop, and Forward comes back to it.
//
// The artifact the link names is not the newest in its project, so the shell's
// default selection cannot pass for the link's. It has two versions, so a
// version switch has somewhere to go, and one open thread, so the comments
// control has something to open.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, newSession, structured, toolCall } from "./invariants.mjs";
import { SCREENSHOT, fontsSettled, pinFont, pinnedFontOptions } from "./pinned-font.mjs";

const TITLE = "Route check";
const SLUG = "route-check.md";
const FIRST = "The first version of the route check.";
const SECOND = "The second version of the route check.";

// One fixture per hub: every check reads the same artifact, and publishing it
// again per check would move the project's own list under the others.
const linked = new Map();
const decoys = new Map();

async function linkedArtifact(hub) {
  if (linked.has(hub.baseUrl)) return linked.get(hub.baseUrl);
  const session = await newSession(hub);
  const published = await toolCall(hub, session, "artifact_publish", {
    project_id: hub.projectId,
    title: TITLE,
    kind: "markdown",
    content: `# ${TITLE}\n\n${FIRST}\n`,
  });
  const id = structured(published).artifact_id;
  await toolCall(hub, session, "artifact_update", {
    artifact_id: id,
    content: `# ${TITLE}\n\n${SECOND}\n`,
  });
  const decoy = await toolCall(hub, session, "artifact_publish", {
    project_id: hub.projectId,
    title: "Route decoy",
    kind: "markdown",
    content: "# Route decoy\n\nNewer than the route check.\n",
  });
  await api(hub, "POST", `/api/v1/artifacts/${encodeURIComponent(id)}/comments`, {
    body: "A thread on the route check.",
    anchor_version: 2,
  });
  linked.set(hub.baseUrl, id);
  decoys.set(hub.baseUrl, structured(decoy).artifact_id);
  return id;
}

const shellAddress = (hub, id) =>
  new RegExp(
    `#/projects/${hub.projectId}/artifacts\\?artifact=${id}(&|$)`.replace(/[.]/g, "\\."),
  );

const isDesktop = (testInfo) => testInfo.project.name === "route-desktop";

// The document inside the hub's frame route, which is itself framed.
const documentOf = (page) => page.frameLocator("#hub-frame").frameLocator("#hub-frame");

// Arrive from the project's feed, then follow the artifact's own address, the
// way a feed row or a search hit is followed.
async function follow(page, hub, id, query = "") {
  await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/feed`);
  await expect(page.locator('.shell[data-segment="feed"]')).toBeVisible();
  await page.evaluate((hash) => {
    location.hash = hash;
  }, `#/artifacts/${encodeURIComponent(id)}?project=${encodeURIComponent(hub.projectId)}${query}`);
  await expect(page).toHaveURL(shellAddress(hub, id));
  await expect(page.getByRole("heading", { level: 1, name: SLUG })).toBeVisible();
}

test.describe("the artifact's address", () => {
  test.use(pinnedFontOptions);

  test("lands on the project's Artifacts shell with the artifact selected", async ({ hub, page }, testInfo) => {
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    await expect(page.locator('.shell[data-segment="artifacts"]')).toBeVisible();
    await expect(documentOf(page).getByText(SECOND)).toBeVisible();
    if (isDesktop(testInfo)) {
      // The index is beside the stage, with the linked artifact marked.
      const row = page.locator(".shell-index").getByRole("link", { name: TITLE, exact: true });
      await expect(row).toHaveAttribute("aria-current", "true");
      await expect(page.locator(".shell-index").getByRole("link", { name: "Route decoy" })).not.toHaveAttribute(
        "aria-current",
        "true",
      );
    } else {
      // A phone shows one zone: the artifact's stage, with a way back to the
      // index it is listed in.
      await expect(page.locator(".shell-index")).toBeHidden();
      await page.getByRole("link", { name: "Back to list" }).click();
      await expect(page).toHaveURL(new RegExp(`#/projects/${hub.projectId}/artifacts$`));
      await expect(page.locator(".shell-index").getByRole("link", { name: TITLE, exact: true })).toBeVisible();
    }
  });

  test("Back leaves the artifact and Forward returns to it", async ({ hub, page }) => {
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`#/projects/${hub.projectId}/feed$`));
    await expect(page.locator('.shell[data-segment="feed"]')).toBeVisible();
    await page.goForward();
    await expect(page).toHaveURL(shellAddress(hub, id));
    await expect(page.getByRole("heading", { level: 1, name: SLUG })).toBeVisible();
  });

  test("a version is switched in the stage and the address keeps it", async ({ hub, page }) => {
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    await page.getByRole("button", { name: "Version 2" }).click();
    const sheet = page.getByRole("dialog", { name: "Versions" });
    await expect(sheet).toBeVisible();
    await sheet.getByRole("button", { name: /^v1\b/ }).click();
    await expect(page).toHaveURL(/[?&]version=1(&|$)/);
    await expect(page).toHaveURL(shellAddress(hub, id));
    await expect(page.locator('.shell[data-segment="artifacts"]')).toBeVisible();
    await expect(page.getByRole("button", { name: "Version 1" })).toBeVisible();
    await expect(documentOf(page).getByText(FIRST)).toBeVisible();
  });

  test("an old link that names a version opens that version in the shell", async ({ hub, page }) => {
    const id = await linkedArtifact(hub);
    await follow(page, hub, id, "&version=1");
    await expect(page).toHaveURL(/[?&]version=1(&|$)/);
    await expect(page.getByRole("button", { name: "Version 1" })).toBeVisible();
    await expect(documentOf(page).getByText(FIRST)).toBeVisible();
  });

  test("a project the link misnames is not trusted, and adds no history", async ({ hub, page }) => {
    const id = await linkedArtifact(hub);
    await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/feed`);
    await expect(page.locator('.shell[data-segment="feed"]')).toBeVisible();
    await page.evaluate((hash) => {
      location.hash = hash;
    }, `#/artifacts/${encodeURIComponent(id)}?project=no-such-project`);
    await expect(page).toHaveURL(shellAddress(hub, id));
    await expect(page.getByRole("heading", { level: 1, name: SLUG })).toBeVisible();
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`#/projects/${hub.projectId}/feed$`));
  });

  test("an artifact that has gone is said in the stage", async ({ hub, page }) => {
    const session = await newSession(hub);
    const published = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "Route gone",
      kind: "markdown",
      content: "# Route gone\n",
    });
    const gone = structured(published).artifact_id;
    await api(hub, "DELETE", `/api/v1/artifacts/${encodeURIComponent(gone)}`);
    await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/artifacts?artifact=${encodeURIComponent(gone)}`);
    await expect(page.locator(".shell-stage").getByText("This artifact is no longer on the hub.")).toBeVisible();
  });

  test("visiting artifacts in turn leaves one comments drawer", async ({ hub, page }) => {
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    const shell = (which) => `#/projects/${hub.projectId}/artifacts?artifact=${encodeURIComponent(which)}`;
    for (const which of [decoys.get(hub.baseUrl), id]) {
      await page.evaluate((hash) => {
        location.hash = hash;
      }, shell(which));
      await expect(page.locator(`.shell-controls [data-action="copy-raw"][data-id="${which}"]`)).toBeVisible();
    }
    await expect(page.locator(".comments-drawer")).toHaveCount(1);
  });

  test("Share opens the share sheet from the stage", async ({ hub, page }) => {
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    // The share endpoint answers 404 for "no link yet", which is not a defect.
    await page.locator(".shell-stage").getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Share" }).click();
    const sheet = page.getByRole("dialog", { name: "Share this artifact" });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole("button", { name: /Make link|Revoke link/ })).toBeVisible();
  });

  test("the stage's bands match their baseline @visual", async ({ hub, page }, testInfo) => {
    // The baseline is pixels, so the text sets in the pinned face rather than
    // whatever sans the host resolves the design's stack to.
    await pinFont(page);
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    await expect(documentOf(page).getByText(SECOND)).toBeVisible();
    await fontsSettled(page);
    const stage = await page.locator(".shell-stage").boundingBox();
    // The header and the row under it, across the stage and the aside on a
    // desktop, and the phone's header and tools row. The meta line carries a
    // relative time, which is masked; every other word sets in the pinned
    // faces, so the path and the version label are held too.
    const clip = isDesktop(testInfo)
      ? { x: stage.x, y: 0, width: page.viewportSize().width - stage.x, height: 92 }
      : { x: 0, y: 0, width: page.viewportSize().width, height: 120 };
    await expect(page).toHaveScreenshot("artifact-bands.png", {
      ...SCREENSHOT,
      clip,
      mask: [page.locator(".shell-stage .shell-head .shell-meta")],
    });
  });
});

test.describe("the artifact's comments on a fine pointer", () => {
  test("are read in the aside, which the comments control toggles", async ({ hub, page }, testInfo) => {
    test.skip(!isDesktop(testInfo), "the aside is the desktop's surface");
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    const aside = page.locator(".shell-aside");
    await expect(aside).toBeVisible();
    await expect(aside.getByText("A thread on the route check.")).toBeVisible();
    const control = page.locator(".shell-stage").getByRole("button", { name: /^Comments, / });
    await control.click();
    await expect(aside).toBeHidden();
    await expect(control).toHaveAttribute("aria-pressed", "false");
    await control.click();
    await expect(aside).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
});

test.describe("the artifact's comments on a fine pointer below the desktop shell", () => {
  // The layout draws no aside below 1100px, so the threads are read in the
  // sheet there whatever the pointer.
  test("are read in the sheet the comments control opens", async ({ hub, page }, testInfo) => {
    if (isDesktop(testInfo)) await page.setViewportSize({ width: 1024, height: 800 });
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    await expect(page.locator(".shell-aside")).toBeHidden();
    await page.locator(".shell-stage").getByRole("button", { name: /^Comments, / }).click();
    const sheet = page.getByRole("dialog");
    await expect(sheet).toHaveCount(1);
    await expect(sheet.getByText("A thread on the route check.")).toBeVisible();
  });
});

test.describe("the artifact's comments on a coarse pointer", () => {
  test.use({ hasTouch: true });

  test("are read in one sheet and never in the aside", async ({ hub, page }, testInfo) => {
    test.skip(isDesktop(testInfo), "the sheet is the phone's surface");
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    await expect(page.locator(".shell-aside")).toBeHidden();
    await page.locator(".shell-stage").getByRole("button", { name: /^Comments, / }).click();
    const sheet = page.getByRole("dialog");
    await expect(sheet).toHaveCount(1);
    await expect(sheet.getByText("A thread on the route check.")).toBeVisible();
    await expect(page.locator(".shell-aside")).toBeHidden();
  });
});

test.describe("the aside's keyboard and its count", () => {
  test("c toggles the aside even before a thread exists", async ({ hub, page }, testInfo) => {
    test.skip(!isDesktop(testInfo), "the aside is the desktop's surface");
    await linkedArtifact(hub);
    const decoy = decoys.get(hub.baseUrl);
    await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/artifacts?artifact=${encodeURIComponent(decoy)}`);
    const aside = page.locator(".shell-aside");
    await expect(aside).toBeVisible();
    await page.locator("main h1").first().focus();
    await page.keyboard.press("c");
    await expect(aside).toBeHidden();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page.keyboard.press("c");
    await expect(aside).toBeVisible();
  });

  // Last in the file, because it adds a thread to the linked artifact.
  test("the aside's count follows a new thread and keeps its version", async ({ hub, page }, testInfo) => {
    test.skip(!isDesktop(testInfo), "the aside is the desktop's surface");
    const id = await linkedArtifact(hub);
    await follow(page, hub, id);
    const meta = page.locator(".shell-aside .hub-comments-head-meta");
    await expect(meta).toHaveText("1 · v2");
    await page.locator(".shell-aside").getByRole("button", { name: "New thread" }).click();
    const sheet = page.getByRole("dialog");
    await sheet.getByRole("textbox").first().fill("A second thread on the route check.");
    await sheet.getByRole("button", { name: "Post" }).click();
    await expect(meta).toHaveText("2 · v2");
  });
});
