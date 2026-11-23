// A screen that cannot load keeps the frame. The failure is said in the stage,
// in ink, with the hub's own words and a Retry; the rail, the desktop's two
// bands and the phone's header stay where every other screen has them. An
// address that names nothing is said or replaced, never painted as a bare page.

import { expect } from "@playwright/test";
import { open, test } from "./app.mjs";
import { api, newSession, structured, toolCall } from "./invariants.mjs";

const homeCall = /\/api\/v1\/home(\?|$)/;
const isPhone = (testInfo) => testInfo.project.name === "phone";

// A hub that answers with an RFC 9457 problem, the way the real one does.
const refuse = (status, detail) => (route) =>
  route.fulfill({
    status,
    contentType: "application/problem+json",
    body: JSON.stringify({ title: "Failure", detail }),
  });

// The frame a failed screen must still carry, measured on the rendered page.
async function expectFrame(page, testInfo, title) {
  const stage = page.locator(".shell-stage");
  await expect(stage.getByRole("heading", { level: 1, name: title })).toBeVisible();
  const frame = await page.evaluate(() => {
    const box = (el) => (el ? el.getBoundingClientRect() : null);
    const visible = (el) => {
      const rect = box(el);
      return Boolean(rect) && rect.width > 0 && rect.height > 0 && getComputedStyle(el).display !== "none";
    };
    const head = document.querySelector(".shell-stage > .shell-head");
    const controls = document.querySelector(".shell-stage > .shell-controls");
    const rail = document.querySelector(".rail");
    return {
      head: visible(head) ? { top: Math.round(box(head).top), bottom: Math.round(box(head).bottom) } : null,
      controls: visible(controls)
        ? { top: Math.round(box(controls).top), bottom: Math.round(box(controls).bottom) }
        : null,
      rail: visible(rail),
    };
  });
  if (isPhone(testInfo)) {
    // The phone is one header, then one tools row: 76px at rest and a 44px row.
    expect(frame.head, "the phone header is missing").toBeTruthy();
    expect(frame.head.bottom - frame.head.top, "the phone header at rest is not 76px").toBe(76);
    expect(frame.controls, "the phone tools row is missing").toBeTruthy();
    expect(frame.controls.bottom - frame.controls.top, "the phone tools row is not 44px").toBe(44);
  } else {
    expect(frame.rail, "the rail is gone").toBe(true);
    expect(frame.head, "the header does not span 0..52").toEqual({ top: 0, bottom: 52 });
    expect(frame.controls, "the control row does not span 52..92").toEqual({ top: 52, bottom: 92 });
  }
}

// The failure reads in ink, not in the deletion tone: the sentence is the ink
// token, the hub's detail is the second ink token, and neither is --danger.
async function expectQuiet(page) {
  const tones = await page.evaluate(() => {
    const resolve = (token) => {
      const probe = document.createElement("span");
      probe.style.color = `var(${token})`;
      document.body.appendChild(probe);
      const value = getComputedStyle(probe).color;
      probe.remove();
      return value;
    };
    const colour = (selector) => {
      const el = document.querySelector(`.shell-stage .empty-state ${selector}`);
      return el ? getComputedStyle(el).color : null;
    };
    return {
      ink: resolve("--ink"),
      ink2: resolve("--ink-2"),
      danger: resolve("--danger"),
      title: colour(".empty-title"),
      body: colour(".empty-body"),
    };
  });
  expect(tones.title, "the sentence is not in the ink token").toBe(tones.ink);
  expect(tones.title, "an error is drawn in --danger").not.toBe(tones.danger);
  if (tones.body) {
    expect(tones.body, "the detail is not in the second ink token").toBe(tones.ink2);
    expect(tones.body, "the detail is drawn in --danger").not.toBe(tones.danger);
  }
  await expect(page.locator("main .error")).toHaveCount(0);
}

test.describe("a screen that fails to load", () => {
  test("a 500 from Now keeps the frame, says why and recovers on Retry", async ({ hub, page }, testInfo) => {
    await page.route(homeCall, refuse(500, "Internal error"));
    await page.goto(`${hub.baseUrl}/#/home`);
    await expectFrame(page, testInfo, "Now");
    const stage = page.locator(".shell-stage");
    await expect(stage.getByText("Internal error")).toBeVisible();
    await expect(stage.getByRole("heading", { level: 2, name: "This screen did not load." })).toBeVisible();
    await expectQuiet(page);

    await page.unroute(homeCall);
    await stage.getByRole("button", { name: "Retry" }).click();
    await expect(page.locator('.shell[data-segment="now"] .now')).toBeVisible();
    await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();
    await expect(page.getByText("This screen did not load.")).toHaveCount(0);
  });

  test("a 500 from a project feed keeps the frame and recovers on Retry", async ({ hub, page }, testInfo) => {
    const feed = new RegExp(`/api/v1/projects/${hub.projectId}/feed(\\?|$)`);
    await page.route(feed, refuse(500, "Feed store unavailable"));
    await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/feed`);
    await expectFrame(page, testInfo, hub.projectId);
    await expect(page.locator(".shell-stage").getByText("Feed store unavailable")).toBeVisible();
    await expectQuiet(page);

    await page.unroute(feed);
    await page.locator(".shell-stage").getByRole("button", { name: "Retry" }).click();
    await expect(page.locator('.shell[data-segment="feed"]')).toBeVisible();
    await expect(page.getByText("This screen did not load.")).toHaveCount(0);
  });

  test("a hub that cannot be reached reads Could not reach the hub", async ({ hub, page }, testInfo) => {
    await open(page, hub, "#/home", "");
    await page.route(homeCall, (route) => route.abort("connectionrefused"));
    await page.evaluate(() => {
      location.hash = "#/search";
      location.hash = "#/home";
    });
    await expectFrame(page, testInfo, "Now");
    const stage = page.locator(".shell-stage");
    await expect(stage.getByText("Could not reach the hub")).toBeVisible();
    await expect(page.getByText("Failed to fetch")).toHaveCount(0);
    await expectQuiet(page);

    await page.unroute(homeCall);
    await stage.getByRole("button", { name: "Retry" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();
    await expect(page.getByText("Could not reach the hub")).toHaveCount(0);
  });

  test("a 401 still routes to Connect", async ({ hub, page }) => {
    await page.route(homeCall, refuse(401, "Unauthorized"));
    await page.goto(`${hub.baseUrl}/#/home`);
    await expect(page).toHaveURL(/#\/connect\?next=/);
    await expect(page.getByText("This screen did not load.")).toHaveCount(0);
  });

  test("the Access screen's card still renders when its list fails", async ({ hub, page }) => {
    await page.route(/\/api\/v1\/members(\?|$)/, refuse(500, "Members unavailable"));
    await page.goto(`${hub.baseUrl}/#/access`);
    const card = page.locator("main .card").filter({ hasText: "Access" });
    await expect(card.getByText("Members unavailable")).toBeVisible();
  });
});

test.describe("the Retry control on a coarse pointer", () => {
  test.use({ hasTouch: true });

  test("answers to 44 by 44", async ({ hub, page }, testInfo) => {
    test.skip(!isPhone(testInfo), "the coarse pointer is the phone project's");
    await page.route(homeCall, refuse(500, "Internal error"));
    await page.goto(`${hub.baseUrl}/#/home`);
    const retry = page.locator(".shell-stage").getByRole("button", { name: "Retry" });
    await expect(retry).toBeVisible();
    const box = await retry.boundingBox();
    expect(box.width, "Retry is narrower than 44px").toBeGreaterThanOrEqual(44);
    expect(box.height, "Retry is shorter than 44px").toBeGreaterThanOrEqual(44);
  });
});

test.describe("an address that names nothing", () => {
  test("an unknown artifact id is said in the Artifacts shell with a way back", async ({ hub, page }, testInfo) => {
    await page.goto(`${hub.baseUrl}/#/artifacts/no-such-artifact`);
    await expectFrame(page, testInfo, "Artifacts");
    const stage = page.locator(".shell-stage");
    await expect(stage.getByText("This artifact is not on this hub.")).toBeVisible();
    await expectQuiet(page);
    await stage.getByRole("link", { name: "Back to Artifacts" }).click();
    await expect(page).toHaveURL(/#\/artifacts$/);
    await expect(page.getByRole("heading", { level: 1, name: "Artifacts" })).toBeVisible();
  });

  test("a failure reading an artifact is the router's, with Retry", async ({ hub, page }, testInfo) => {
    const session = await newSession(hub);
    const made = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "Error state check",
      kind: "markdown",
      content: "# Error state check\n",
    });
    const id = structured(made).artifact_id;
    const versions = new RegExp(`/api/v1/artifacts/${encodeURIComponent(id)}/versions(\\?|$)`);
    await page.route(versions, refuse(500, "Versions unavailable"));
    await page.goto(`${hub.baseUrl}/#/artifacts/${encodeURIComponent(id)}?project=${hub.projectId}`);
    await expectFrame(page, testInfo, "Artifacts");
    await expect(page.locator(".shell-stage").getByText("Versions unavailable")).toBeVisible();
    await page.unroute(versions);
    await page.locator(".shell-stage").getByRole("button", { name: "Retry" }).click();
    await expect(page).toHaveURL(new RegExp(`#/projects/${hub.projectId}/artifacts\\?artifact=`));
    await api(hub, "DELETE", `/api/v1/artifacts/${encodeURIComponent(id)}`);
  });

  test("a failed project list is an error with Retry, not 'not listed'", async ({ hub, page }, testInfo) => {
    const session = await newSession(hub);
    const made = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "List failure check",
      kind: "markdown",
      content: "# List failure check\n",
    });
    const id = structured(made).artifact_id;
    const list = new RegExp(`/api/v1/projects/${hub.projectId}/artifacts(\\?|$)`);
    await page.route(list, refuse(500, "Artifact list unavailable"));
    await page.goto(`${hub.baseUrl}/#/artifacts/${encodeURIComponent(id)}?project=${hub.projectId}`);
    await expectFrame(page, testInfo, "Artifacts");
    const stage = page.locator(".shell-stage");
    await expect(stage.getByText("Artifact list unavailable")).toBeVisible();
    await expect(page.getByText("no project lists it")).toHaveCount(0);
    await expect(page.getByText("This artifact is not on this hub.")).toHaveCount(0);
    await page.unroute(list);
    await stage.getByRole("button", { name: "Retry" }).click();
    await expect(page).toHaveURL(new RegExp(`#/projects/${hub.projectId}/artifacts\\?artifact=`));
    await api(hub, "DELETE", `/api/v1/artifacts/${encodeURIComponent(id)}`);
  });

  test("a late 401 after the reader has moved on does not go to Connect", async ({ hub, page }) => {
    const session = await newSession(hub);
    const made = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: "Late refusal check",
      kind: "markdown",
      content: "# Late refusal check\n",
    });
    const id = structured(made).artifact_id;
    const versions = new RegExp(`/api/v1/artifacts/${encodeURIComponent(id)}/versions(\\?|$)`);
    let release;
    const held = new Promise((resolve) => {
      release = resolve;
    });
    let arrived;
    const requested = new Promise((resolve) => {
      arrived = resolve;
    });
    await page.route(versions, async (route) => {
      arrived();
      await held;
      await refuse(401, "Unauthorized")(route);
    });
    await page.goto(`${hub.baseUrl}/#/more`);
    await expect(page.getByRole("heading", { level: 1, name: "More" })).toBeVisible();
    await page.evaluate((hash) => {
      location.hash = hash;
    }, `#/artifacts/${encodeURIComponent(id)}`);
    await requested;
    await page.evaluate(() => {
      location.hash = "#/settings";
    });
    await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
    release();
    await page.waitForResponse(versions);
    await expect(page).toHaveURL(/#\/settings$/);
    await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
    await page.unroute(versions);
    await api(hub, "DELETE", `/api/v1/artifacts/${encodeURIComponent(id)}`);
  });

  test("an unknown screen is replaced by Now and Now is marked current", async ({ hub, page }, testInfo) => {
    await page.goto(`${hub.baseUrl}/#/more`);
    await expect(page.getByRole("heading", { level: 1, name: "More" })).toBeVisible();
    await page.evaluate(() => {
      location.hash = "#/nowhere";
    });
    await expect(page).toHaveURL(/#\/home$/);
    await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();
    const nav = isPhone(testInfo) ? page.locator(".tabbar a") : page.locator(".rail a.rail-item");
    await expect(nav.and(page.locator('[aria-current="page"]'))).toHaveCount(1);
    await expect(nav.and(page.locator('[aria-current="page"]'))).toContainText("Now");
    // The dead address does not stay in history for Back to return to.
    await page.goBack();
    await expect(page).toHaveURL(/#\/more$/);
  });
});
