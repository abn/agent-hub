// Search under the states a reader reaches: a word too long to be one, a query
// the hub cannot run, a wiki page in the preview, and the one scope list at
// both widths.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

const SCOPES = ["All", "Feed", "Artifacts", "Wiki", "Sessions"];
const PAGE = "---\ntype: Runbook\ntitle: Quokka plan\n---\nQuokka drains via [[rollback-steps]] and [[ops|the ops page]].\n";

async function seedPage(hub, name, content) {
  const url = `${hub.baseUrl}/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`;
  const response = await fetch(url, {
    method: "PUT",
    headers: { Authorization: `Bearer ${hub.token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ content }),
  });
  expect(response.ok, `PUT ${name} answered ${response.status}`).toBe(true);
}

const hash = (term, extra = "") => `#/search?q=${encodeURIComponent(term)}${extra}`;

test.describe("search states", () => {
  test("a 140-character word returns hits or the no-results state, never an error", async ({ hub, page }) => {
    const answered = page.waitForResponse((response) => response.url().includes("/api/v1/search?"));
    await page.goto(`${hub.baseUrl}/${hash("a".repeat(140))}`);
    expect((await answered).status()).toBe(200);
    await expect(page.locator(".search-results .empty-state, .search-results .search-row").first()).toBeVisible();
    await expect(page.locator(".search-results .error")).toHaveCount(0);
    await expect(page.locator(".search-results")).not.toContainText(/FTS|parse error|Range query/);
  });

  test("a query that fails shows the no-results state with a plain line", async ({ hub, page }) => {
    await page.route("**/api/v1/search?**", (route) =>
      route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: "Internal error: FTS parse error" }) }),
    );
    await page.goto(`${hub.baseUrl}/${hash("anything")}`);
    await expect(page.locator(".search-results")).toContainText("Nothing matches");
    await expect(page.locator(".search-count")).toHaveText("No results");
    await expect(page.locator(".search-results .error")).toHaveCount(0);
    await expect(page.locator("body")).not.toContainText(/FTS|parse error/);
  });

  test("the scope list is the same at both widths", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/#/search`);
    const chips = page.locator(".search-scopes button[data-scope]");
    await expect(chips).toHaveCount(SCOPES.length);
    // Before a query a chip is its label alone, on a phone too.
    await expect(chips).toHaveText(SCOPES);
    await page.goto(`${hub.baseUrl}/${hash(hub.fixture.searchTerm)}`);
    await expect(page.locator(".search-row").first()).toBeVisible();
    const names = await chips.evaluateAll((nodes) => nodes.map((node) => node.firstChild.textContent.trim()));
    expect(names).toEqual(SCOPES);
  });

  test("the Wiki scope asks the hub for wiki pages", async ({ hub, page }) => {
    await seedPage(hub, "search-states/quokka.md", PAGE);
    await page.goto(`${hub.baseUrl}/${hash("quokka", "&type=kb")}`);
    await expect(page.locator(".search-row").first()).toBeVisible();
    await expect(page.locator(".search-scopes button[aria-pressed='true']")).toContainText("Wiki");
  });

  test("a wiki hit previews as text, without frontmatter or link brackets", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the preview is the desktop's");
    await seedPage(hub, "search-states/quokka.md", PAGE);
    await page.goto(`${hub.baseUrl}/${hash("quokka")}`);
    const body = page.locator(".search-preview-body");
    await expect(body).toContainText("Quokka drains via");
    await expect(body).toContainText("rollback-steps and the ops page");
    await expect(body).not.toContainText("---");
    await expect(body).not.toContainText("type: Runbook");
    await expect(body).not.toContainText("[[");
  });

  test("the empty preview says what to do in its body, not its header", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the preview is the desktop's");
    await page.goto(`${hub.baseUrl}/${hash("zzzznomatch")}`);
    await expect(page.locator(".search-preview-empty")).toHaveText("Select a result to preview.");
    await expect(page.locator(".search-stage-head")).not.toContainText("Select a result");
  });

  test("phone chips carry counts only after a query", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "counts are the phone's");
    await page.goto(`${hub.baseUrl}/#/search`);
    await expect(page.locator(".search-chip-count")).toHaveCount(0);
    await page.goto(`${hub.baseUrl}/${hash(hub.fixture.searchTerm)}`);
    await expect(page.locator(".search-chip-count").first()).toBeVisible();
  });
});
