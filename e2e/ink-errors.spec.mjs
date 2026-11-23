// Errors, failure notes and invalid fields are drawn in ink and say what went
// wrong in words. The deletion tone stays on the controls that delete.

import { expect } from "@playwright/test";
import { open, test } from "./app.mjs";

// True when the element or anything inside it computes the deletion tone as a
// text, border, outline or background colour.
async function drawsDanger(locator) {
  return locator.evaluate((root) => {
    const probe = document.createElement("span");
    probe.style.color = "var(--danger)";
    document.body.appendChild(probe);
    const danger = getComputedStyle(probe).color;
    probe.remove();
    const found = [];
    for (const el of [root, ...root.querySelectorAll("*")]) {
      const style = getComputedStyle(el);
      for (const side of ["Top", "Right", "Bottom", "Left"]) {
        if (parseFloat(style[`border${side}Width`]) > 0 && style[`border${side}Color`] === danger) {
          found.push(`${el.className || el.tagName} border`);
        }
      }
      if (style.color === danger) found.push(`${el.className || el.tagName} color`);
      if (style.backgroundColor === danger) found.push(`${el.className || el.tagName} background`);
    }
    return found;
  });
}

const quiet = async (locator, why) => expect(await drawsDanger(locator), why).toEqual([]);

test.describe("errors are ink, deletion is danger", () => {
  test("a failed screen, a failed search and a failed sync line are not red", async ({ hub, page }) => {
    await page.route(/\/api\/v1\/home(\?|$)/, (route) =>
      route.fulfill({
        status: 500,
        contentType: "application/problem+json",
        body: JSON.stringify({ title: "Failure", detail: "Home store unavailable" }),
      }),
    );
    await page.goto(`${hub.baseUrl}/#/home`);
    const stage = page.locator(".shell-stage");
    await expect(stage.getByText("Home store unavailable")).toBeVisible();
    await quiet(stage, "the failed screen is drawn in --danger");
    await page.unrouteAll({ behavior: "ignoreErrors" });

    await page.route("**/api/v1/search?**", (route) =>
      route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: "Internal error" }) }),
    );
    await page.goto(`${hub.baseUrl}/#/search?q=anything`);
    await expect(page.locator(".search-results")).toHaveText("Search failed. Try again.");
    await quiet(page.locator(".search-results"), "the search failure is drawn in --danger");
    await page.unrouteAll({ behavior: "ignoreErrors" });

    await open(page, hub, "#/home", "");
    await page.evaluate(() => {
      window.__sync.noteSyncFailure({ status: 500 });
      window.__sync.noteSyncFailure({ status: 500 });
    });
    const sync = page.locator("#rail-sync");
    if (await sync.isVisible()) {
      await expect(sync).toHaveAttribute("data-state", "failed");
      await quiet(sync, "the failed sync line is drawn in --danger");
    }
  });

  test.describe("on a touch phone", () => {
    test.use({ hasTouch: true, viewport: { width: 390, height: 844 } });

    test("the sync line under More is ink", async ({ hub, page }) => {
      await open(page, hub, "#/home", "");
      await page.evaluate(() => {
        window.__sync.noteSyncFailure({ status: 500 });
        window.__sync.noteSyncFailure({ status: 500 });
      });
      await page.evaluate(() => {
        location.hash = "#/more";
      });
      await expect(page.getByRole("heading", { level: 1, name: "More" })).toBeVisible();
      const line = page.locator("#more-sync");
      await expect(line).toBeVisible();
      await expect(line).toHaveAttribute("data-state", "failed");
      await quiet(line, "the failed sync line under More is drawn in --danger");
    });
  });

  test("a wrong password says so in ink", async ({ hub, page }) => {
    await open(page, hub, "#/home", "");
    await page.evaluate(
      ([artifact, project]) => {
        location.hash = `#/artifacts/${encodeURIComponent(artifact)}?project=${encodeURIComponent(project)}`;
      },
      [hub.protectedId, hub.projectId],
    );
    const gate = page.frameLocator("main iframe");
    await gate.locator("#hub-password").fill("not the password");
    await gate.locator('#hub-unlock-form button[type="submit"]').click();
    const message = gate.locator("#hub-unlock-error");
    await expect(message).toContainText("Wrong password");
    await quiet(message, "the wrong-password line is drawn in --danger");
  });

  test("an invalid field and a refused save are ink, and Delete project is danger", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/#/projects/${hub.projectId}/settings`);
    await expect(page.locator("main .pset")).toBeVisible();
    const name = page.locator("#pset-name");
    await name.fill("");
    await name.fill(" ");
    await page.getByRole("button", { name: "Save" }).click();
    const field = page.locator(".pset-field").first();
    await expect(page.locator("#pset-name-error")).toHaveText("A project needs a name.");
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await quiet(field, "the invalid field is drawn in --danger");

    await page.route(new RegExp(`/api/v1/projects/${hub.projectId}$`), (route) =>
      route.request().method() === "PATCH"
        ? route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: "Store unavailable" }) })
        : route.continue(),
    );
    await name.fill("Another name");
    await page.getByRole("button", { name: "Save" }).click();
    const problem = page.locator(".pset-problem");
    await expect(problem).toContainText("Nothing changed");
    await quiet(problem, "the refused save is drawn in --danger");

    const remove = page.locator(".pset-delete");
    if (await remove.count()) {
      expect(await drawsDanger(remove), "Delete project lost the deletion tone").not.toEqual([]);
    }
  });

  test("the new project dialog's taken slug is ink", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/#/projects`);
    await page.getByRole("button", { name: "New project" }).first().click();
    const dialog = page.locator(".project-create-dialog");
    await dialog.locator("#project-create-name").fill(hub.projectId);
    await dialog.getByRole("button", { name: "Create project" }).click();
    const taken = dialog.locator(".project-create-taken-text");
    await expect(taken).toBeVisible();
    await expect(taken).toContainText("is taken");
    await quiet(dialog, "the taken-name message is drawn in --danger");
  });
});
