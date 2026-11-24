// A session reads once: its row is titled by the name, and the stage names the
// full id one time, with its copy glyph, without repeating what the stage head
// already says.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { newSession, structured, toolCall } from "./invariants.mjs";

const route = (hub, query) =>
  `${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(hub.sessionId)}${query}`;

test.describe("session detail", () => {
  test("the full id appears once in the stage, with its copy glyph", async ({ hub, page }) => {
    await page.goto(route(hub, ""));
    await expect(page.locator("main .session-detail-header")).toBeVisible();
    const text = (await page.locator("main").textContent()) || "";
    const shown = text.split(hub.sessionId).length - 1;
    expect(shown, "the full session id is shown more than once in the stage").toBe(1);
    const copy = page.locator('main [aria-label^="Copy full session id"]');
    await expect(copy).toHaveCount(1);
    await expect(copy.locator("xpath=ancestor::*[contains(@class,'meta')][1]")).toContainText(hub.sessionId);
    const header = page.locator(".session-detail-header");
    // A session is shared: who took part is named once, and nothing says it is owned.
    expect(await header.textContent(), "a session claims an owner").not.toMatch(/\bowner\b/i);
    expect(await header.getByRole("button", { name: "Reassign" }).count(), "a session offers a reassign").toBe(0);
    for (const line of await header.locator(".meta").allTextContents()) {
      expect(line.trim(), "a meta line starts or ends with a separator").not.toMatch(/^·|·$/);
    }
  });

  test("a row is titled by the session name, with the short id in its meta", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/sessions`);
    const row = page.locator(`.session-row[data-id="${hub.sessionId}"]`);
    await expect(row.locator(".title")).toHaveText(hub.fixture.sessionName);
    await expect(row.locator(".meta .session-id")).toBeVisible();
  });

  test("Resume and Brain stay while a kv key is open", async ({ hub, page }) => {
    await page.goto(route(hub, "&file=%2Fkv%2Flast-run"));
    const toggle = page.getByRole("group", { name: "Session view" });
    await expect(toggle.getByRole("link", { name: "Resume" })).toBeVisible();
    await expect(toggle.getByRole("link", { name: "Brain" })).toBeVisible();
  });

  // A session holding exactly one thing of each kind, so the label under test is
  // the singular. Items are keys plus files, so one key and one file are two
  // sessions: the key's session carries "1 key" and "1 item", the file's "1 file".
  async function holding(hub, name, path) {
    const mcp = await newSession(hub);
    const started = await toolCall(hub, mcp, "session_start", { project_id: hub.projectId, session_name: name });
    const id = structured(started).session_id || "";
    expect(id, "the check could not start a session").toBeTruthy();
    await toolCall(hub, mcp, "brain_put", { path, content: "one", store: "session", session: { session_id: id } });
    return id;
  }

  test("a count of one is singular", async ({ hub, page }) => {
    const keyed = await holding(hub, "one-key", "/kv/only");
    await page.goto(route({ ...hub, sessionId: keyed }, ""));
    const stage = page.locator("main");
    await expect(stage.locator(".tree-count", { hasText: /^\s*1 key\s*$/ })).toBeVisible();
    await expect(stage.locator(".brain-label")).toContainText("1 item");
    await expect(stage.locator(".session-detail-header .meta").filter({ hasText: /event/ })).toContainText(/\b1 event\b/);
    const filed = await holding(hub, "one-file", "/fs/only.md");
    await page.goto(route({ ...hub, sessionId: filed }, ""));
    await expect(stage.locator(".tree-count", { hasText: /^\s*1 file\s*$/ })).toBeVisible();
    const plural = /\b1 (events|keys|files|items)\b/;
    expect((await stage.textContent()) || "").not.toMatch(plural);
    await page.goto(route({ ...hub, sessionId: keyed }, ""));
    await expect(stage.locator(".tree-count", { hasText: /^\s*1 key\s*$/ })).toBeVisible();
    expect((await stage.textContent()) || "").not.toMatch(plural);
  });
});
