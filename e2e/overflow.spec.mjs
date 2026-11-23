// No screen goes sideways, however long the words an agent wrote. An answer
// option, a subject, a title or a name is the agent's text, and the agent does
// not know how wide the reader's screen is. Every API response is rewritten on
// its way to the page so those fields carry a long sentence and a long run of
// characters with no space in it, then every screen is opened and the page is
// held to the viewport's width. A screen that fails names the elements that
// reach past the edge.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { newSession, structured, toolCall } from "./invariants.mjs";
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

// What an agent's text is stretched with: a sentence that has to wrap, and a
// token with no break opportunity in it.
const SENTENCE = "and a long tail of plain words that has to wrap onto further lines";
const TOKEN = "an-unbroken-run-of-characters-wider-than-any-phone-column-0123456789abcdef";

// The fields the screens draw as text an agent or a person wrote. Ids, paths
// and names that address something are left alone: the page fetches by them.
const PROSE = new Set(["subject", "summary", "title", "display_name", "description"]);

function stretch(value, key) {
  if (key === "options" && Array.isArray(value)) {
    return value.map((option) => (typeof option === "string" ? `${option} ${SENTENCE} ${TOKEN}` : option));
  }
  if (Array.isArray(value)) return value.map((entry) => stretch(entry, ""));
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([name, entry]) => [name, stretch(entry, name)]));
  }
  if (typeof value === "string" && PROSE.has(key) && value.length > 0) return `${value} ${SENTENCE} ${TOKEN}`;
  // A Markdown body carries the token as inline code too, which wraps with its
  // sentence.
  if (typeof value === "string" && key === "body" && value.length > 0) return `${value} ${SENTENCE} \`${TOKEN}\``;
  return value;
}

// What may scroll sideways on purpose: code and tables keep their lines, and a
// strip of segments or chips scrolls rather than wrapping. One of these excuses
// what is inside it only while it really scrolls and itself fits the screen.
const SCROLLS = [
  "pre", "table", "[role=tablist]", ".seg", ".shell-seg", ".search-scopes", ".feed-chips",
  ".project-tools-seg", ".arts-controls", ".session-file-content", ".hub-artifacts-table-wrap",
].join(", ");

// The visible elements that reach past the right edge of the screen, outside
// what scrolls sideways on purpose. A container that clips or scrolls its
// overflow hides it from the page's own width, so each box is measured.
function overflowing(scrolls) {
  const edge = document.documentElement.clientWidth + 1;
  const found = [];
  for (const el of document.querySelectorAll("body *")) {
    const ownText = [...el.childNodes].some((node) => node.nodeType === Node.TEXT_NODE && node.textContent.trim());
    if ((el.children.length && !ownText) || el.closest("[hidden], dialog:not([open])")) continue;
    const strip = el.parentElement && el.parentElement.closest(scrolls);
    if (strip && /auto|scroll/.test(getComputedStyle(strip).overflowX) && strip.getBoundingClientRect().right <= edge) continue;
    const box = el.getBoundingClientRect();
    if (box.width === 0 || box.height === 0 || box.right <= edge) continue;
    const style = getComputedStyle(el);
    if (style.visibility === "hidden" || style.display === "none") continue;
    const at = el.closest("button, a, [class]") || el;
    const name = at.tagName.toLowerCase() + (typeof at.className === "string" && at.className.trim() ? `.${at.className.trim().split(/\s+/).join(".")}` : "");
    found.push(`${name} ends at ${Math.round(box.right)}`);
  }
  return {
    page: document.documentElement.scrollWidth,
    width: document.documentElement.clientWidth,
    found: [...new Set(found)].slice(0, 6),
  };
}

test("no screen goes sideways with long agent text", async ({ hub, page }) => {
  await page.route("**/api/v1/**", async (route) => {
    // The event stream never ends, so it is passed through untouched.
    if (route.request().url().includes("/api/v1/stream")) return route.continue();
    const response = await route.fetch();
    const type = response.headers()["content-type"] || "";
    if (!type.includes("application/json")) return route.fulfill({ response });
    const body = stretch(await response.json(), "");
    return route.fulfill({ response, json: body });
  });
  const wiki = await seedWikiPage(hub, "overflow-check");
  const routes = screenRoutes(hub, wiki);
  await page.goto(`${hub.baseUrl}/#/home`);
  await expectEveryScreen(page, routes);

  const misfits = [];
  for (const entries of Object.values(routes)) {
    for (const [name, hash, ready] of entries) {
      await openScreen(page, hub, hash, ready);
      const result = await page.evaluate(overflowing, SCROLLS);
      if (hash === "#/home") {
        // The check is only as good as its long text: an ask's answer has to
        // be drawn as a verb carrying it.
        const verbs = page.locator(".now-acts button", { hasText: TOKEN });
        await expect(verbs.first(), "no answer on Now carries the long text").toBeAttached();
      }
      if (result.page > result.width + 1 || result.found.length) {
        misfits.push(`${name}: the page is ${result.page}px in ${result.width}px; ${result.found.join("; ") || "no element named"}`);
      }
    }
  }
  await page.unrouteAll({ behavior: "ignoreErrors" });
  expect(misfits, "a screen goes sideways").toEqual([]);
});

// A brain key is a path an agent chose. At phone width it is cut with an
// ellipsis inside its own row, and the full key stays reachable in `title`.
test("a long brain key is cut inside its row", async ({ hub, page }) => {
  const key = `long-${"k".repeat(140)}`;
  const agent = await newSession(hub);
  const started = await toolCall(hub, agent, "session_start", {
    project_id: hub.projectId,
    session_name: "long-key",
  });
  const id = structured(started).session_id || "";
  expect(id, "the check could not start a session").toBeTruthy();
  await toolCall(hub, agent, "brain_put", {
    path: `/kv/${key}`,
    content: "one",
    store: "session",
    session: { session_id: id },
  });
  await page.goto(
    `${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(id)}`,
  );
  const name = page.locator(".brain-tree-unified .tree-name", { hasText: key });
  await expect(name).toBeVisible();
  await expect(name).toHaveAttribute("title", key);
  const cut = await name.evaluate((el) => {
    const row = el.closest(".tree-item").getBoundingClientRect();
    const box = el.getBoundingClientRect();
    return {
      clipped: el.scrollWidth > el.clientWidth,
      ellipsis: getComputedStyle(el).textOverflow,
      inside: box.right <= row.right + 1,
      page: document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1,
    };
  });
  expect(cut).toEqual({ clipped: true, ellipsis: "ellipsis", inside: true, page: true });
});
