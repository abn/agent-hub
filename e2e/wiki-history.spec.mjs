// A wiki page's history: the reader opens it from the page, reads an earlier
// version against the page as it is now, and puts it back in one confirmed
// step. A deleted page is restored the same way from its history.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

const FIRST = "---\ntype: Runbook\ntitle: Rollback\n---\n# Rollback\n\nStop the service.\n";
const SECOND = "---\ntype: Runbook\ntitle: Rollback\n---\n# Rollback\n\nStop the service, then drain it.\n";

async function api(hub, method, path, body) {
  const response = await fetch(`${hub.baseUrl}${path}`, {
    method,
    headers: { Authorization: `Bearer ${hub.token}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  expect(response.ok, `${method} ${path} answered ${response.status}`).toBe(true);
  return response.json();
}

const pageApi = (hub, name) => `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages/${name}`;
const pageRoute = (hub, name) => `#/projects/${encodeURIComponent(hub.projectId)}/wiki?page=${encodeURIComponent(name)}`;

test("an earlier version is read against the page and reverted to", async ({ hub, page }, testInfo) => {
  const name = `history/rollback-${testInfo.project.name}.md`;
  await api(hub, "PUT", pageApi(hub, name), { content: FIRST });
  await api(hub, "PUT", pageApi(hub, name), { content: SECOND });

  // A phone shows the stage alone once a page is open, so the wait is on the
  // reader's own control rather than on the index.
  await page.goto(`${hub.baseUrl}/${pageRoute(hub, name)}`);
  await page.getByRole("link", { name: "History", exact: true }).click();
  await expect(page.getByText("2 versions", { exact: true })).toBeVisible();
  await expect(page.getByRole("link", { name: /edited/ })).toContainText("current");

  await page.getByRole("link", { name: /created/ }).click();
  const changes = page.getByRole("group", { name: "Changes since this version" });
  await expect(changes).toContainText("Removed: Stop the service.");
  await expect(changes).toContainText("Added: Stop the service, then drain it.");
  await expect(page.getByText("Since this version: 1 line added, 1 removed.")).toBeVisible();

  await page.getByRole("button", { name: "Revert to this" }).click();
  const dialog = page.getByRole("dialog", { name: "Revert Rollback?" });
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await dialog.getByRole("button", { name: "Revert page" }).click();

  await expect(page.locator(".wiki-page")).toContainText("Stop the service.");
  await expect(page.locator(".wiki-page")).not.toContainText("drain");
  const history = await api(
    hub,
    "GET",
    `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/versions?path=${encodeURIComponent(name)}`,
  );
  expect(history.total, "the revert is a new version, not a removal").toBe(3);
  expect(history.versions[0].op).toBe("kb.revert");
});

test("a deleted page is restored from its history", async ({ hub, page }, testInfo) => {
  const name = `history/gone-${testInfo.project.name}.md`;
  await api(hub, "PUT", pageApi(hub, name), { content: FIRST });
  await api(hub, "DELETE", pageApi(hub, name));

  await page.goto(`${hub.baseUrl}/${pageRoute(hub, name)}`);
  await expect(page.getByText("That page was deleted.")).toBeVisible();
  await page.getByRole("link", { name: "history", exact: true }).click();
  await expect(page.getByText("2 versions · deleted", { exact: true })).toBeVisible();

  await page.getByRole("link", { name: /created/ }).click();
  await expect(page.getByText("The page was deleted after this version.")).toBeVisible();
  await page.getByRole("button", { name: "Restore this" }).click();
  await page.getByRole("dialog", { name: "Restore Rollback?" }).getByRole("button", { name: "Restore page" }).click();

  await expect(page.locator(".wiki-page")).toContainText("Stop the service.");
});

test("the operator forgets a page's earlier versions and keeps the page", async ({ hub, page }, testInfo) => {
  const name = `history/forget-${testInfo.project.name}.md`;
  await api(hub, "PUT", pageApi(hub, name), { content: FIRST });
  await api(hub, "PUT", pageApi(hub, name), { content: SECOND });

  await page.goto(`${hub.baseUrl}/${pageRoute(hub, name)}`);
  await page.getByRole("link", { name: "History", exact: true }).click();
  await expect(page.getByRole("link", { name: /created/ })).toBeVisible();
  await page.getByRole("button", { name: "Forget history" }).click();
  const dialog = page.getByRole("dialog", { name: "Forget this page's history?" });
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await dialog.getByRole("button", { name: "Forget history" }).click();

  // The rows stay, the earlier one is no longer a door, and nothing is left
  // to forget.
  await expect(page.getByText("not kept")).toBeVisible();
  await expect(page.getByRole("link", { name: /created/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Forget history" })).toHaveCount(0);
  const current = await api(hub, "GET", pageApi(hub, name));
  expect(current.content).toBe(SECOND);
});

test("a page reverted to an older version has nothing left to forget once forgotten", async ({ hub, page }, testInfo) => {
  const name = `history/reverted-${testInfo.project.name}.md`;
  const first = await api(hub, "PUT", pageApi(hub, name), { content: FIRST });
  await api(hub, "PUT", pageApi(hub, name), { content: SECOND });
  await api(hub, "POST", `${pageApi(hub, name)}/revert`, { version: first.version });

  await page.goto(`${hub.baseUrl}/${pageRoute(hub, name)}`);
  await page.getByRole("link", { name: "History", exact: true }).click();
  await page.getByRole("button", { name: "Forget history" }).click();
  await page
    .getByRole("dialog", { name: "Forget this page's history?" })
    .getByRole("button", { name: "Forget history" })
    .click();

  // The first write holds the bytes the page holds again, so it stays a door,
  // and nothing is left to forget.
  await expect(page.getByText("not kept")).toBeVisible();
  await expect(page.getByRole("link", { name: /created/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Forget history" })).toHaveCount(0);
});

test("a deleted page whose history was forgotten does not offer its versions", async ({ hub, page }, testInfo) => {
  const name = `history/purged-${testInfo.project.name}.md`;
  await api(hub, "PUT", pageApi(hub, name), { content: FIRST });
  await api(hub, "DELETE", pageApi(hub, name));
  await api(
    hub,
    "DELETE",
    `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/versions?path=${encodeURIComponent(name)}`,
  );

  await page.goto(`${hub.baseUrl}/${pageRoute(hub, name)}`);
  await expect(page.getByText("That page was deleted and its earlier versions were forgotten.")).toBeVisible();
  await expect(page.getByText("holds its earlier versions")).toHaveCount(0);
  await page.getByRole("link", { name: "history", exact: true }).click();
  await expect(page.getByRole("button", { name: "Forget history" })).toHaveCount(0);
});

test("a page the hub fails to read is not taken for a deleted one", async ({ hub, page }, testInfo) => {
  const name = `history/flaky-${testInfo.project.name}.md`;
  await api(hub, "PUT", pageApi(hub, name), { content: FIRST });
  // The page read alone fails; its history and every other route answer.
  await page.route(
    (url) => url.pathname.endsWith(`/kb/pages/${name}`) && !url.search,
    (route) =>
      route.fulfill({
        status: 500,
        contentType: "application/problem+json",
        body: JSON.stringify({ status: 500, code: "internal", detail: "the store did not answer" }),
      }),
  );

  await page.goto(`${hub.baseUrl}/${pageRoute(hub, name)}`);
  await expect(page.getByText("Could not read this page: the store did not answer")).toBeVisible();
  await expect(page.getByText("That page was deleted.")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Restore this" })).toHaveCount(0);
});
