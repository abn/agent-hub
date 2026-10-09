// Every screen the router registers, with an address that opens it and what
// only that screen paints once its data is in. A check that has to hold every
// screen walks this table, and `expectEveryScreen` compares it with the
// router's own registered list, so a screen added to the router fails such a
// check until it is given an address here.

import { expect } from "@playwright/test";

const PAGE_FIRST = "---\ntype: Runbook\ntitle: Rollback\n---\n# Rollback\n\nStop the service.\n";
const PAGE_SECOND = "---\ntype: Runbook\ntitle: Rollback\n---\n# Rollback\n\nStop the service, then drain it.\n";

async function call(hub, method, path, body) {
  const response = await fetch(`${hub.baseUrl}${path}`, {
    method,
    headers: { Authorization: `Bearer ${hub.token}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  expect(response.ok, `${method} ${path} answered ${response.status}`).toBe(true);
  return response.json();
}

// A wiki page with two versions, so the reader, its history and a diff all
// have something to draw. The name is the caller's, so two checks on one hub
// do not write the same page.
export async function seedWikiPage(hub, name) {
  const base = `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb`;
  await call(hub, "PUT", `${base}/pages/${name}`, { content: PAGE_FIRST });
  await call(hub, "PUT", `${base}/pages/${name}`, { content: PAGE_SECOND });
  const history = await call(hub, "GET", `${base}/versions?path=${encodeURIComponent(name)}`);
  return { name, older: history.versions[history.versions.length - 1].version };
}

// The addresses, by the screen name the router registers. A screen that only
// forwards (the bare feed, sessions and artifacts addresses, and the old
// session address) is opened at its own address and held where it lands.
export function screenRoutes(hub, wiki) {
  const project = encodeURIComponent(hub.projectId);
  const at = `#/projects/${project}`;
  const page = `${at}/wiki?page=${encodeURIComponent(wiki.name)}`;
  return {
    home: [["Home", "#/home", "main .home"]],
    inbox: [["Inbox", "#/inbox", "main .shell-index .inbox-item"]],
    projects: [
      ["Projects", "#/projects", "main .projects-screen .project-row"],
      ["Feed", `${at}/feed`, "main .shell-index .feed-row"],
      ["Artifacts", `${at}/artifacts`, "main .shell-index .artifact-row"],
      ["Sessions", `${at}/sessions`, "main .session-row"],
      ["Wiki", `${at}/wiki`, "main .wiki-row"],
      ["Wiki page", page, "main .wiki-page"],
      ["Wiki history", `${page}&history=1`, "main .wiki-versions"],
      ["Wiki version", `${page}&history=1&version=${encodeURIComponent(wiki.older)}`, "main .wiki-diff"],
      ["Wiki editor", `${page}&edit=1`, "main .wiki-editor"],
      ["Project settings", `${at}/settings`, "main .pset"],
    ],
    feed: [["the bare feed address", "#/feed", "main .shell-index .feed-row"]],
    sessions: [["the bare sessions address", "#/sessions", "main .session-row"]],
    artifacts: [
      ["the bare artifacts address", "#/artifacts", "main .shell-index .artifact-row"],
      ["Artifact viewer", `#/artifacts/${encodeURIComponent(hub.artifactId)}`, "main iframe"],
    ],
    session: [
      ["Session detail", `#/session?project=${project}&id=${encodeURIComponent(hub.sessionId)}`, "main .brain-tree-unified, main .session-detail-header"],
    ],
    search: [["Search", `#/search?q=${encodeURIComponent(hub.fixture.searchTerm)}`, "main .search-row"]],
    storage: [["Storage", "#/storage", "main .storage .storage-row"]],
    settings: [["Settings", "#/settings", "main .row .title, main .form-row-title"]],
    more: [["More", "#/more", "main .more-screen .more-row"]],
    access: [["Agents and tokens", "#/access", "main .agent-index-row, main .agent-row, main .access-row"]],
    connect: [["Connect", "#/connect?next=%2Fstorage", "main .connect .connect-field"]],
  };
}

// The table and the router name the same screens.
export async function expectEveryScreen(page, routes) {
  const registered = await page.evaluate(() => window.__router.screens());
  expect(Object.keys(routes).sort(), "the routes and the router's screens differ").toEqual(registered.sort());
}

// Open one address and wait for what only that screen paints.
export async function openScreen(page, hub, hash, ready) {
  await page.goto(`${hub.baseUrl}/${hash}`);
  await expect(page.locator(ready).first(), `${hash} did not paint (${ready})`).toBeVisible();
}
