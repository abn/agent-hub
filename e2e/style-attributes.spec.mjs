// Styles live in the stylesheet. A `style` attribute carries only a value the
// script works out at runtime, never a size, spacing, type or colour, because a
// declaration written into the markup is one the stylesheet gates never read:
// the token rule, the type floor and the control sizes all pass while the
// attribute paints something else over them.
//
// The check reads the rendered DOM rather than the source, so it holds what a
// reader is shown: every screen the router registers and the states and
// dialogs listed below, at the width and pointer of the project that runs it.
// For each element that carries a `style` attribute it reads the declarations
// the browser parsed out of it, longhand by longhand, and refuses any that
// draws: a box size, spacing, type, colour, fill, border, shadow or outline,
// and any custom property that is not a named runtime value.
//
// The walk writes nothing a later check reads. The feed's seen cursor is not
// moved, because the Unread chip in the interaction checks counts what is still
// above it; a write a state would make is answered by the test instead; and
// every dialog is dismissed with its keep answer.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

// The properties a `style` attribute must not declare. Each pattern is matched
// against a parsed longhand, so `padding: 0 16px` is caught as `padding-top`
// and the rest, `border-radius` as its four corners, and `white-space` as the
// collapse and wrap longhands it sets.
const BANNED = [
  /^(min-|max-)?(width|height)$/,
  /^(min-|max-)?(inline|block)-size$/,
  /^padding(-|$)/,
  /^margin(-|$)/,
  /^(row-|column-)?gap$/,
  /^font(-|$)/,
  /^line-height$/,
  /^letter-spacing$/,
  /^color$/,
  /^background(-|$)/,
  /^border(-|$)/,
  /^box-shadow$/,
  /^outline(-|$)/,
  /^fill(-|$)/,
  /^stroke(-|$)/,
  /^caret-color$/,
  /^accent-color$/,
  /^text-(transform|align|decoration|wrap)(-|$)/,
  /^white-space(-|$)/,
];

// Runtime values, each named by the element that carries it. A custom property
// is refused unless it is named here, since one can carry any length a rule
// then reads.
const ALLOWED = [
  // A segment of a stacked bar is as wide as its kind's share of the bytes.
  { selector: ".storage-seg, .storage-share-seg", properties: ["width"] },
  // A Home project row's bar is as wide as its share of the day's events.
  { selector: ".home-bar > span", properties: ["width"] },
  // A rendered document's frame is as tall as the document reports itself.
  { selector: "iframe", properties: ["height"] },
  // A wiki tree row is indented by the depth of its path.
  { selector: ".wiki-row", properties: ["--depth"] },
  // A pane is as wide as the reader dragged it or the layout restored it.
  { selector: ".shell", properties: [/^--w-/] },
  // The comment drawer sits on the keyboard and fits what is still visible.
  { selector: ".comments-drawer", properties: ["--keyboard-inset", "--visible-height"] },
];

async function offenders(page, where) {
  return page.evaluate(
    ({ banned, allowed, where }) => {
      const patterns = banned.map((source) => new RegExp(source));
      const found = [];
      for (const el of document.querySelectorAll("[style]")) {
        const allow = allowed.filter((entry) => el.matches(entry.selector)).flatMap((entry) => entry.properties);
        const allows = (property) =>
          allow.some((entry) => (typeof entry === "string" ? entry === property : new RegExp(entry.source).test(property)));
        for (let i = 0; i < el.style.length; i++) {
          const property = el.style[i];
          const custom = property.startsWith("--");
          if (!custom && !patterns.some((pattern) => pattern.test(property))) continue;
          if (allows(property)) continue;
          const name = `${el.tagName.toLowerCase()}${el.classList.length ? `.${[...el.classList].join(".")}` : ""}`;
          found.push(`${where}: ${name} sets ${property}: ${el.style.getPropertyValue(property)}`);
        }
      }
      return found;
    },
    {
      banned: BANNED.map((pattern) => pattern.source),
      allowed: ALLOWED.map((entry) => ({
        selector: entry.selector,
        properties: entry.properties.map((p) => (p instanceof RegExp ? { source: p.source } : p)),
      })),
      where,
    },
  );
}

// What a click opens on a screen, held as its own state: the address, what
// only that screen paints, and the controls pressed in order to reach it. A
// state that exists at one layout names it, because the phone reaches some of
// these from a different place or not at all.
function states(hub, wiki, agent) {
  const project = encodeURIComponent(hub.projectId);
  const at = `#/projects/${project}`;
  const page = `${at}/wiki?page=${encodeURIComponent(wiki.name)}`;
  const agentAt = `#/access?agent=${encodeURIComponent(agent)}`;
  const agentReady = "main .access-screen, main .agent-index-row";
  const session = `${at}/sessions?id=${encodeURIComponent(hub.sessionId)}`;
  const file = `${session}&file=${encodeURIComponent(hub.fixture.brainFsTop)}`;
  return [
    { name: "Agent selected", hash: agentAt, ready: agentReady },
    {
      // The seeded agents hold no live token, so Issue reveals one at once. The
      // test answers the issue itself, so no token is written, and the reveal
      // is closed with Done, its only way out.
      name: "Issue a token",
      hash: agentAt,
      ready: agentReady,
      answer: { url: "**/api/v1/agents/*/token", method: "POST", status: 200, body: { token: "not-a-real-token" } },
      press: ['[data-action="agent-issue"]'],
      close: "dialog[open] .dialog-commit",
    },
    { name: "Grant a project", hash: agentAt, ready: agentReady, press: ['[data-action="agent-grant-open"]'], layout: "desktop" },
    { name: "Add an agent", hash: "#/access", ready: agentReady, press: ['[data-action="toggle-add-agent"]'] },
    { name: "New project", hash: "#/projects", ready: "main .projects-screen .project-row", press: ['[data-action="new-project"]'] },
    {
      name: "Delete a project",
      hash: `${at}/feed`,
      ready: "main .feed-row",
      press: [".proj-overflow-btn", '[data-action="delete-project"]'],
    },
    { name: "New wiki page", hash: `${at}/wiki`, ready: "main .wiki-row", press: ['[data-action="wiki-new"]'], layout: "desktop" },
    { name: "A comment thread", hash: page, ready: "main .wiki-comment" },
    {
      name: "Revert to a version",
      hash: `${page}&history=1&version=${encodeURIComponent(wiki.older)}`,
      ready: "main .wiki-diff",
      press: ['[data-action="wiki-revert"]'],
    },
    {
      // The hub's refusal is answered by the test, so the page is not written.
      name: "A save refused for a changed page",
      hash: `${page}&edit=1`,
      ready: "main .wiki-editor",
      answer: { url: "**/kb/pages/**", method: "PUT", status: 409, body: { message: "changed" } },
      press: ['[data-action="wiki-save"]'],
      shows: ".wiki-conflict-btn",
    },
    { name: "Wiki changes", hash: `${at}/wiki?view=changes`, ready: "main .wiki-change", layout: "desktop" },
    { name: "Wiki lint", hash: `${at}/wiki?view=lint`, ready: "main .wiki-finding, main .wiki-empty", layout: "desktop" },
    { name: "A brain file", hash: file, ready: "main .session-doc-content" },
    { name: "Save a brain file to the wiki", hash: file, ready: "main .session-doc-content", press: ['[data-action="wiki-promote"]'] },
    {
      // A key is read in the aside on a desktop and in a sheet under a coarse
      // pointer; a phone-width window with a mouse opens it from the tree.
      name: "A brain key",
      hash: `${session}&file=${encodeURIComponent("/kv/last-run")}`,
      ready: ".session-kv-value:visible",
      layout: "desktop or coarse",
    },
    { name: "End a session", hash: session, ready: "main .end-session", press: ['[data-action="end"]'] },
    { name: "Prune ended sessions", hash: "#/storage", ready: "main .storage", press: ["[data-action='storage-prune-all'], .storage-prune-all-btn"] },
  ];
}

test.describe("styles live in the stylesheet", () => {
  test("no element carries a size, spacing, type or colour in its style attribute", async ({ hub, page }) => {
    // The feed's seen cursor stays where the seed left it.
    await page.route("**/api/v1/projects/*/feed/seen", (route) => route.abort());
    const wiki = await seedWikiPage(hub, "styles/attributes.md");
    const base = `${hub.baseUrl}/api/v1/projects/${encodeURIComponent(hub.projectId)}`;
    const headers = { Authorization: `Bearer ${hub.token}`, "Content-Type": "application/json" };
    const posted = await fetch(`${base}/kb/comments`, {
      method: "POST",
      headers,
      body: JSON.stringify({ path: wiki.name, body: "Check the rollback order." }),
    });
    expect(posted.ok, `the page comment was refused with ${posted.status}`).toBe(true);
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);
    const desktop = page.viewportSize().width >= 1100;
    const coarse = await page.evaluate(() => matchMedia("(pointer: coarse)").matches);
    const reaches = (state) =>
      !state.layout || (state.layout === "desktop" ? desktop : desktop || coarse);

    const found = [];
    let held = 0;
    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        found.push(...(await offenders(page, name)));
        held += 1;
      }
    }

    const response = await fetch(`${hub.baseUrl}/api/v1/agents`, { headers });
    const { agents } = await response.json();
    const tokenless = agents.find((agent) => agent.state !== "pending" && agent.has_live_token === false);
    expect(tokenless, "the seeded hub has no enrolled agent without a token").toBeDefined();
    for (const state of states(hub, wiki, tokenless.id)) {
      if (!reaches(state)) continue;
      const { answer } = state;
      if (answer) {
        await page.route(answer.url, (route) =>
          route.request().method() === answer.method
            ? route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body) })
            : route.fallback(),
        );
      }
      // The bare origin between two addresses is a fresh load, so an address
      // that differs from the last only in its hash still paints from the start.
      await page.goto(`${hub.baseUrl}/`);
      await openScreen(page, hub, state.hash, state.ready);
      for (const opener of state.press || []) {
        const control = page.locator(opener).filter({ visible: true }).first();
        await expect(control, `${state.name}: nothing reaches it with ${opener}`).toBeVisible();
        await control.click();
      }
      if (state.shows) await expect(page.locator(state.shows).first(), `${state.name} did not paint`).toBeVisible();
      if (await page.locator("dialog[open]").count()) await expect(page.locator("dialog[open]").first()).toBeVisible();
      found.push(...(await offenders(page, state.name)));
      held += 1;
      // Escape is the keep answer of every other dialog the walk opens, while
      // nothing has been typed into it; the walk holds that it closed.
      if (state.close) await page.locator(state.close).click();
      else if (await page.locator("dialog[open]").count()) await page.keyboard.press("Escape");
      await expect(page.locator("dialog[open]"), `${state.name}: the dialog did not close`).toHaveCount(0);
      if (answer) await page.unroute(answer.url);
    }

    expect(held).toBeGreaterThan(0);
    expect(found, "a style attribute declares what belongs in the stylesheet").toEqual([]);
  });
});
