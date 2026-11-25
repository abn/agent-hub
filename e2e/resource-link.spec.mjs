// A resource's short address, `#/id/<id>`, and the two query parameters its
// routes carry. The hub's resolve answer is mocked at the network, so each
// kind is held without a hub that knows the id. The routes the answers name
// are real ones on the seeded hub.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, newSession, structured, toolCall } from "./invariants.mjs";

const RESOLVE = /\/api\/v1\/resolve\//;

const answer = (body, status = 200) => (route) =>
  route.fulfill({
    status,
    contentType: status === 200 ? "application/json" : "application/problem+json",
    body: JSON.stringify(body),
  });

// Start on Now, then follow the short address, the way a pasted link is.
async function follow(page, hub, id) {
  await page.goto(`${hub.baseUrl}/#/home`);
  await expect(page.locator('.shell[data-segment="now"] .now')).toBeVisible();
  await page.evaluate((hash) => {
    location.hash = hash;
  }, `#/id/${id}`);
}

test.describe("the short address", () => {
  test("replaces itself with each kind's route and leaves no history entry", async ({ hub, page }) => {
    const project = encodeURIComponent(hub.projectId);
    const routes = {
      artifact: `#/artifacts/${encodeURIComponent(hub.artifactId)}`,
      event: `#/projects/${project}/feed?event=${encodeURIComponent(hub.questionId)}`,
      session: `#/projects/${project}/sessions?session=${encodeURIComponent(hub.sessionId)}`,
      comment: `#/artifacts/${encodeURIComponent(hub.artifactId)}?comment=42`,
      "KB comment": `#/projects/${project}/wiki?page=runbook.md&comment=42`,
    };
    for (const [kind, route] of Object.entries(routes)) {
      await page.route(RESOLVE, answer({ kind, id: "short", project_id: hub.projectId, route }));
      await follow(page, hub, "short");
      const place = kind === "artifact" || kind === "comment" ? /#\/projects\/[^/]+\/artifacts\?artifact=/ : null;
      if (place) await expect(page).toHaveURL(place);
      else await expect(page).toHaveURL(new RegExp(route.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      expect(await page.evaluate(() => location.hash), `${kind}: the short address is still shown`).not.toContain(
        "/id/",
      );
      // Back leaves the link: the address was replaced, not stacked.
      await page.goBack();
      await expect(page).toHaveURL(/#\/home$/);
      await page.unroute(RESOLVE);
    }
  });

  test("says Opening while the hub is asked", async ({ hub, page }) => {
    let release;
    const held = new Promise((open) => (release = open));
    await page.route(RESOLVE, async (route) => {
      await held;
      await answer({ kind: "artifact", id: "slow", project_id: hub.projectId, route: "#/home" })(route);
    });
    await follow(page, hub, "slow");
    await expect(page.getByRole("status").getByText("Opening the link.")).toBeVisible();
    release();
    await expect(page).toHaveURL(/#\/home$/);
  });

  test("says nothing is there for an id the hub does not resolve", async ({ hub, page }) => {
    await page.route(RESOLVE, answer({ title: "Not Found", detail: "No such resource" }, 404));
    await follow(page, hub, "missing");
    await expect(page.getByRole("heading", { name: "Nothing is at this address." })).toBeVisible();
    await expect(page).toHaveURL(/#\/id\/missing$/);
    await page.getByRole("link", { name: "Go to Now" }).click();
    await expect(page).toHaveURL(/#\/home$/);
  });

  test("does not follow an answer that is not a route in the app", async ({ hub, page }) => {
    await page.route(RESOLVE, answer({ kind: "artifact", id: "odd", project_id: "p", route: "https://example.test/" }));
    await follow(page, hub, "odd");
    await expect(page.getByRole("heading", { name: "Nothing is at this address." })).toBeVisible();
  });
});

test.describe("the query a resolved route carries", () => {
  test("?session= selects that session in its project", async ({ hub, page }) => {
    await page.goto(
      `${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/sessions?session=${encodeURIComponent(hub.sessionId)}`,
    );
    const row = page.locator(`.session-row.selected[data-id="${hub.sessionId}"]`);
    await expect(row).toBeAttached();
  });

  test("?comment= opens that comment's thread", async ({ hub, page }, testInfo) => {
    // Its own artifact: a thread on the seeded one would change what later specs see there.
    const session = await newSession(hub);
    const published = await toolCall(hub, session, "artifact_publish", {
      project_id: hub.projectId,
      title: `Linked thread, ${testInfo.project.name}`,
      kind: "markdown",
      content: "A short note to hang a thread on.",
    });
    const artifactId = structured(published).artifact_id;
    const body = `A thread a link opens, ${testInfo.project.name}.`;
    const made = await api(hub, "POST", `/api/v1/artifacts/${encodeURIComponent(artifactId)}/comments`, {
      body,
      anchor_version: 1,
    });
    expect(made.status, made.text).toBeLessThan(300);
    const id = made.json().id ?? made.json().comment?.id;
    await page.goto(`${hub.baseUrl}/#/artifacts/${encodeURIComponent(artifactId)}?comment=${encodeURIComponent(id)}`);
    await expect(page).toHaveURL(/comment=/);
    if (testInfo.project.name === "desktop") {
      const card = page.locator(`.shell-aside .hub-comment-card[data-comment-id="${id}"]`);
      await expect(card).toBeVisible();
      await expect(card).toBeFocused();
    } else {
      await expect(page.getByRole("dialog").getByText(body)).toBeVisible();
    }
  });
});
