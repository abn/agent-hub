// The wiki says what is true: the empty state names a command that exists, the
// reader's verbs leave the title alone, the home has one New page, and the
// editor says what it holds and refuses a blank save.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

async function api(hub, method, path, body) {
  const response = await fetch(`${hub.baseUrl}${path}`, {
    method,
    headers: { Authorization: `Bearer ${hub.token}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  expect(response.ok, `${method} ${path} answered ${response.status}`).toBe(true);
  return response.json();
}

const wikiRoute = (project, query = "") => `#/projects/${encodeURIComponent(project)}/wiki${query}`;
const TITLE = "Rollback and drain runbook";
const PAGE = `---\ntype: Runbook\ntitle: ${TITLE}\n---\n# Long\n\nBody.\n`;

test("the empty wiki copies the real kb put command", async ({ hub, page, context }, testInfo) => {
  const id = `wiki-empty-${testInfo.project.name}`;
  await api(hub, "POST", "/api/v1/projects", { id, display_name: `Wiki empty ${testInfo.project.name}` });
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto(`${hub.baseUrl}/${wikiRoute(id)}`);
  const command = `agent-hub kb put index.md --file index.md --project ${id}`;
  await expect(page.locator(".wiki-instruction-command")).toHaveText(command);
  await page.getByRole("button", { name: "Copy agent instruction" }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(command);
});

test("the reader's verbs sit in the control row and the title is whole", async ({ hub, page }, testInfo) => {
  const name = `reader/long-${testInfo.project.name}.md`;
  await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`, { content: PAGE });
  await page.goto(`${hub.baseUrl}/${wikiRoute(hub.projectId, `?page=${encodeURIComponent(name)}`)}`);
  const stage = page.locator(".shell-stage");
  const title = stage.locator("h1.shell-title-line");
  await expect(title).toHaveText(TITLE);
  await expect(stage.locator(".shell-head").getByRole("button", { name: /Review/ })).toHaveCount(0);
  await expect(stage.locator(".shell-head").getByRole("link", { name: "Edit" })).toHaveCount(0);
  const controls = stage.locator(".shell-controls");
  await expect(controls.getByRole("button", { name: /Review/ })).toBeVisible();
  await expect(controls.getByRole("link", { name: "Edit", exact: true })).toBeVisible();
  const truncated = await title.evaluate((el) => el.scrollWidth > el.clientWidth + 1);
  if (testInfo.project.name === "phone") expect(truncated, "the phone title is cut to a fragment").toBe(false);
});

test("the wiki home has one New page and rows with a count and a chevron", async ({ hub, page }, testInfo) => {
  const name = `home/page-${testInfo.project.name}.md`;
  await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`, { content: PAGE });
  await page.goto(`${hub.baseUrl}/${wikiRoute(hub.projectId)}`);
  await expect(page.locator(".shell-index .wiki-row").first()).toBeVisible();
  await expect(page.locator('.shell-stage [data-action="wiki-new"]')).toHaveCount(0);
  if (testInfo.project.name === "desktop") {
    await expect(page.locator('.shell [data-action="wiki-new"]:visible')).toHaveCount(1);
  }
  if (testInfo.project.name === "desktop") {
    const rows = page.locator(".wiki-home-link");
    await expect(rows).toHaveCount(3);
    for (const row of await rows.all()) {
      await expect(row.locator(".wiki-home-count")).toHaveText(/^\d+$/);
      await expect(row.locator("svg")).toBeVisible();
    }
  }
});

test("a blank editor holds Save and says why; a typed one is marked unsaved", async ({ hub, page }) => {
  const writes = [];
  page.on("request", (request) => {
    if (request.method() === "PUT") writes.push(request.url());
  });
  await page.goto(`${hub.baseUrl}/${wikiRoute(hub.projectId, "?new=1")}`);
  const save = page.getByRole("button", { name: "Create page" });
  const text = page.locator("#wiki-content");
  await expect(text).toHaveAttribute("placeholder", /.+/);
  await expect(save).toHaveAttribute("aria-disabled", "true");
  await expect(page.locator("#wiki-save-reason")).not.toBeEmpty();
  await expect(page.locator(".wiki-dirty")).toBeHidden();

  await page.locator("#wiki-new-path").fill("blank-guard.md");
  await save.click({ force: true });
  await save.focus();
  await page.keyboard.press("Enter");
  await text.focus();
  await page.keyboard.press("Control+Enter");
  await expect(page.locator(".wiki-editor")).toBeVisible();
  expect(page.url()).toContain("new=1");
  expect(writes, "a blank page was written").toEqual([]);

  await text.fill("# Not blank\n");
  await expect(save).toHaveAttribute("aria-disabled", "false");
  await expect(page.locator("#wiki-save-reason")).toBeEmpty();
  await expect(page.locator(".wiki-dirty")).toBeVisible();

  await text.fill("   \n");
  await expect(save).toHaveAttribute("aria-disabled", "true");
});

test("the dirty mark clears after Save, Cancel and leaving the editor", async ({ hub, page }, testInfo) => {
  const name = `editor/dirty-${testInfo.project.name}.md`;
  const edit = `${hub.baseUrl}/${wikiRoute(hub.projectId, `?page=${encodeURIComponent(name)}&edit=1`)}`;
  await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`, { content: PAGE });
  const text = page.locator("#wiki-content");
  const mark = page.locator(".wiki-dirty");
  const type = async () => {
    await text.press("End");
    await text.pressSequentially("x");
    await expect(mark).toBeVisible();
  };

  await page.goto(edit);
  await type();
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.locator(".wiki-page")).toBeVisible();
  await page.goto(edit);
  await expect(mark).toBeHidden();

  await type();
  await page.getByRole("link", { name: "Cancel" }).click();
  await expect(page.locator(".wiki-page")).toBeVisible();
  await page.goto(edit);
  await expect(mark).toBeHidden();

  await type();
  await page.evaluate((id) => {
    location.hash = `#/projects/${encodeURIComponent(id)}/wiki`;
  }, hub.projectId);
  await expect(text).toHaveCount(0);
  await page.goBack();
  await expect(text).toBeVisible();
  await expect(mark).toBeHidden();
});

test("Reload theirs takes the hub's copy as a clean baseline", async ({ hub, page }, testInfo) => {
  const name = `editor/reload-${testInfo.project.name}.md`;
  const url = `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`;
  await api(hub, "PUT", url, { content: PAGE });
  await page.goto(`${hub.baseUrl}/${wikiRoute(hub.projectId, `?page=${encodeURIComponent(name)}&edit=1`)}`);
  const text = page.locator("#wiki-content");
  await text.press("End");
  await text.pressSequentially("mine");
  const theirs = `${PAGE}\nTheirs.\n`;
  await api(hub, "PUT", url, { content: theirs });
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page.getByRole("button", { name: "Reload theirs" }).click();
  await expect(text).toHaveValue(theirs);
  await expect(page.locator(".wiki-dirty")).toBeHidden();
  await expect(page.getByRole("button", { name: "Save", exact: true })).toHaveAttribute("aria-disabled", "false");
  await text.fill("");
  await expect(page.getByRole("button", { name: "Save", exact: true })).toHaveAttribute("aria-disabled", "true");
  await text.fill(theirs);
  await expect(page.locator(".wiki-dirty")).toBeHidden();
});

test("Recent changes stays on a wiki with no live pages, and restores the only page", async ({ hub, page }, testInfo) => {
  const id = `wiki-gone-${testInfo.project.name}`;
  await api(hub, "POST", "/api/v1/projects", { id, display_name: `Wiki gone ${testInfo.project.name}` });
  const url = `/api/v1/projects/${encodeURIComponent(id)}/kb/pages/only.md`;
  await api(hub, "PUT", url, { content: PAGE });
  await api(hub, "DELETE", url);
  await page.goto(`${hub.baseUrl}/${wikiRoute(id)}`);
  if (testInfo.project.name === "desktop") {
    await expect(page.getByText("No pages yet.")).toBeVisible();
  } else {
    await expect(page.getByText("No wiki yet")).toBeVisible();
  }
  await expect(page.locator(".wiki-home-link:visible")).toHaveCount(1);
  await page.getByRole("link", { name: /Recent changes/ }).click();
  await page.locator(".wiki-change").first().click();
  await page.getByRole("link", { name: /created/ }).click();
  await page.getByRole("button", { name: "Restore this" }).click();
  await page.getByRole("dialog", { name: `Restore ${TITLE}?` }).getByRole("button", { name: "Restore page" }).click();
  await expect(page.locator(".wiki-page")).toContainText("Body.");
});

test("a phone has New page in the overflow menu only", async ({ hub, page }, testInfo) => {
  test.skip(testInfo.project.name !== "phone", "the filter row's New page is the desktop's");
  await page.goto(`${hub.baseUrl}/${wikiRoute(hub.projectId)}`);
  const tools = page.getByRole("toolbar", { name: "Project tools" });
  await tools.getByRole("button", { name: "Filter and group" }).click();
  await expect(page.locator(".mobile-filter-open")).toBeVisible();
  await expect(page.locator(".project-wiki-new")).toBeHidden();
  await tools.getByRole("button", { name: "Project actions" }).click();
  await expect(page.getByRole("menuitem", { name: "New page" })).toBeVisible();
  await expect(page.getByRole("button", { name: "New page" }).and(page.locator(":visible"))).toHaveCount(0);
});

test("the editor is titled by the page, not its file", async ({ hub, page }, testInfo) => {
  const name = `editor/titled-${testInfo.project.name}.md`;
  await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`, {
    content: "---\ntype: Runbook\ntitle: Deploy guide\n---\n# Deploy\n",
  });
  await page.goto(`${hub.baseUrl}/${wikiRoute(hub.projectId, `?page=${encodeURIComponent(name)}&edit=1`)}`);
  await expect(page.locator(".shell-stage h1.shell-title-line")).toHaveText("Deploy guide");
  await expect(page.locator(".wiki-dirty")).toBeHidden();
  await page.locator("#wiki-content").press("End");
  await page.locator("#wiki-content").pressSequentially("x");
  await expect(page.locator(".wiki-dirty")).toBeVisible();
});
