// One ring per focused control, and space between the token field and its
// button. The ring is a shadow (--focus), so a wrapper that draws it and a
// control inside it that draws it too put two concentric rings on screen, which
// no geometry check sees. This walks the screens with a real Tab key and counts
// the rings. It is the port of focus-rings.py onto Playwright Test, so it
// asserts rendered styles rather than source text.
//
// It runs in its own projects (focus-desktop, focus-phone, focus-touch) because
// it needs three widths, one of them a coarse pointer, and reads several screens
// including a feed; sharing a project's hub with the behaviour checks would move
// their state.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

const WIDTHS = {
  "focus-desktop": "1100px",
  "focus-phone": "390px",
  "focus-touch": "390px touch",
};

// Every wrapper that draws the ring on `:focus-within` or `:has(...:focus-visible)`.
// `ring` is the one shadow the subtree may draw it with: `focus` is the --focus
// token itself, `inset` the accent edge a full-bleed row link is given instead.
// The wrappers are listed rather than discovered: which box owns a ring is a
// decision, and a wrapper nobody names is a wrapper nothing holds.
const RINGS = [
  {
    name: "connect token field",
    wrapper: ".connect-field-wrap",
    controls: [".connect-field", ".connect-eye-btn"],
    ring: "focus",
    route: "#/connect",
    spacing: true,
  },
  {
    name: "search field",
    wrapper: ".search-field",
    controls: [".search-field input", ".search-clear"],
    ring: "focus",
    route: "#/search?q={term}",
  },
  {
    // The column's fill paints over a ring drawn on the row, so the ring sits
    // on the column, and nothing inside the owner may cover its edge.
    name: "Now row",
    wrapper: ".now-row",
    controls: [".now-row"],
    ring: "inset",
    route: "#/inbox",
    // Narrower than 940px the row has no fill over it and draws the ring itself.
    owners: { "1100px": "div.now-main" },
    uncovered: true,
    wide: true,
  },
  {
    // The tab track scrolls and clips what leaves it, so the ring is drawn
    // inside the tab and the tab inside the track.
    name: "project tab",
    wrapper: ".project-tools-seg",
    controls: [".project-tools-seg a"],
    ring: "inset",
    route: "#/projects/{project}/feed",
    owner: "a",
    within: ".project-tools-seg",
    widths: ["390px", "390px touch"],
  },
  {
    name: "project register row",
    wrapper: ".project-row",
    controls: [".project-link"],
    ring: "inset",
    route: "#/projects",
  },
];

// Wrappers that read a focus without drawing a ring on the wrapper itself. The
// control inside carries the ring alone, so the count is still one.
const SINGLE_RINGS = [
  {
    // The field and the send control each draw their own ring; the row that
    // holds them draws none.
    name: "reply composer",
    wrapper: ".composer-row",
    controls: [".composer-field", ".composer-send"],
    ring: "focus",
    route: "#/inbox?open={question}",
  },
  {
    name: "connect submit",
    wrapper: "form[data-action='connect']",
    control: ".connect-submit-btn",
    ring: "focus",
    route: "#/connect",
  },
  {
    name: "comment composer",
    wrapper: ".comments-compose .composer-row",
    control: ".comments-compose .composer-field",
    ring: "focus",
    route: "#/artifacts/{artifact}",
    openComments: true,
  },
  {
    name: "comment card",
    wrapper: ".hub-comment-card",
    control: ".hub-comment-card button",
    ring: "focus",
    route: "#/artifacts/{artifact}",
    // The cards live in the aside, hidden below the desktop breakpoint.
    widths: ["1100px"],
  },
];

// The measurement, in the page. `--focus` is a surface spacer under an accent
// ring, so a shadow carrying both colours carries the token; nothing else draws
// both. An inset accent shadow with no spacer is the row link's own ring.
function probe({ wrapperSel, controlSel }) {
  const root = getComputedStyle(document.documentElement);
  const rgb = (name) => {
    const m = /^#([0-9a-f]{6})$/i.exec((root.getPropertyValue(name) || "").trim());
    if (!m) return null;
    const n = parseInt(m[1], 16);
    return `rgb(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255})`;
  };
  const surface = rgb("--surface");
  const accent = rgb("--accent");
  const layers = (shadow) => {
    const parts = [];
    let depth = 0;
    let cur = "";
    for (const ch of shadow || "") {
      if (ch === "(") depth++;
      else if (ch === ")") depth--;
      if (ch === "," && depth === 0) {
        parts.push(cur);
        cur = "";
        continue;
      }
      cur += ch;
    }
    parts.push(cur);
    return parts.map((p) => p.trim()).filter(Boolean);
  };
  const colourOf = (layer) => {
    const m = /^\s*((?:rgba?|hsla?)\([^)]*\)|#[0-9a-fA-F]+|[a-zA-Z]+)/.exec(layer);
    return m ? m[1].replace(/\s+/g, "") : layer.trim();
  };
  const shadowColours = (el) => layers(getComputedStyle(el).boxShadow).map(colourOf);
  const insetOf = (el) => getComputedStyle(el).boxShadow.includes("inset");
  const opaqueOutline = (el) => {
    const cs = getComputedStyle(el);
    if (cs.outlineStyle === "none" || cs.outlineColor === "transparent") return null;
    const alpha = /^rgba\([^)]*?,\s*([\d.]+)\)$/.exec(cs.outlineColor.replace(/\s+/g, ""));
    if (alpha && Number(alpha[1]) === 0) return null;
    return `${cs.outlineStyle} ${cs.outlineWidth} ${cs.outlineColor}`;
  };
  const describe = (el) => {
    const cls = typeof el.className === "string" ? el.className.trim() : "";
    return el.tagName.toLowerCase() + (cls ? "." + cls.split(/\s+/).join(".") : "");
  };
  const wrap = document.querySelector(wrapperSel);
  const control = document.querySelector(controlSel);
  if (!wrap || !control) return { error: `not found: ${!wrap ? wrapperSel : controlSel}` };

  const subtree = [wrap, ...wrap.querySelectorAll("*")];
  const focus = subtree.filter((el) => {
    const colours = shadowColours(el);
    return colours.includes(surface) && colours.includes(accent);
  });
  const inset = subtree.filter((el) => {
    const colours = shadowColours(el);
    return !colours.includes(surface) && colours.includes(accent) && insetOf(el);
  });
  const covers = (owner) => {
    const box = owner.getBoundingClientRect();
    return [...owner.querySelectorAll("*")]
      .filter((el) => {
        const bg = getComputedStyle(el).backgroundColor;
        if (bg === "rgba(0, 0, 0, 0)" || bg === "transparent") return false;
        const r = el.getBoundingClientRect();
        return r.left <= box.left + 0.5 && r.top <= box.top + 0.5;
      })
      .map(describe);
  };
  const outside = (owner, boxSel) => {
    const track = owner.closest(boxSel)?.getBoundingClientRect();
    const r = owner.getBoundingClientRect();
    return !track || r.left < track.left - 0.5 || r.right > track.right + 0.5 || r.top < track.top - 0.5 || r.bottom > track.bottom + 0.5;
  };
  const outlines = subtree.map((el) => opaqueOutline(el)).filter(Boolean);
  const carriers = subtree.filter((el) => {
    const cs = getComputedStyle(el);
    return cs.outlineStyle !== "none" && cs.outlineWidth !== "0px";
  });
  return {
    focused: document.activeElement === control,
    focusVisible: control.matches(":focus-visible"),
    covered: inset.flatMap(covers),
    escapes: inset.filter((el) => outside(el, wrapperSel)).map(describe),
    focusRings: focus.map(describe),
    insetRings: inset.map(describe),
    opaqueOutlines: outlines,
    carriers: carriers.map((el) => `${describe(el)} ${getComputedStyle(el).outlineWidth}`),
  };
}

// The Connect form's own rhythm: the gap between the field box and the button
// under it, and the page width beside it.
function connectSpacing() {
  const wrap = document.querySelector(".connect-field-wrap");
  const button = document.querySelector(".connect-submit-btn");
  if (!wrap || !button) return { error: "the Connect form is not in the page" };
  const field = wrap.getBoundingClientRect();
  const submit = button.getBoundingClientRect();
  return {
    gap: Math.round(submit.top - field.bottom),
    pageWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
  };
}

const TAB_LIMIT = 140;

async function tabTo(page, selector) {
  for (let i = 0; i < TAB_LIMIT; i += 1) {
    const there = await page.evaluate(
      (sel) => {
        const el = document.querySelector(sel);
        return !!el && document.activeElement === el;
      },
      selector,
    );
    if (there) return true;
    await page.keyboard.press("Tab");
  }
  return false;
}

const address = (route, hub) =>
  route
    .replace("{term}", hub.fixture.searchTerm)
    .replace("{question}", hub.questionId)
    .replace("{artifact}", hub.artifactId)
    .replace("{project}", encodeURIComponent(hub.projectId));

async function enter(page, target, hub) {
  await page.evaluate((route) => {
    location.hash = route;
  }, address(target.route, hub));
  if (target.openComments) {
    // The drawer is mounted by the viewer route, so wait for the viewer before
    // opening it; otherwise openCommentsDrawer runs before the drawer exists.
    await page
      .locator("#hub-frame")
      .first()
      .waitFor({ state: "visible", timeout: 8000 })
      .catch(() => {});
    await page.evaluate(() => import("./comments.mjs").then((m) => m.openCommentsDrawer()));
  }
  try {
    await page.locator(target.wrapper).first().waitFor({ state: "visible", timeout: 8000 });
    return null;
  } catch {
    return `${target.wrapper} never arrived on ${target.route}`;
  }
}

async function leave(page, target) {
  if (target.openComments) {
    await page.evaluate(() => import("./comments.mjs").then((m) => m.closeCommentsDrawer()));
  }
}

async function holdOneRing(page, target, control, where) {
  const reached = await tabTo(page, control);
  expect(reached, `${where}: the keyboard never reached ${control}`).toBe(true);
  const result = await page.evaluate(probe, { wrapperSel: target.wrapper, controlSel: control });
  expect(result.error, `${where}: ${result.error}`).toBeUndefined();
  expect(
    result.focused && result.focusVisible,
    `${where}: ${control} is focused=${result.focused} focus-visible=${result.focusVisible}, so this is not the keyboard ring`,
  ).toBe(true);
  const drawn = target.ring === "focus" ? result.focusRings : result.insetRings;
  const other = target.ring === "focus" ? result.insetRings : result.focusRings;
  expect(
    drawn.length,
    `${where}: ${drawn.length} elements draw the ring on ${control}, expected one: ${drawn.join(", ") || "none"}`,
  ).toBe(1);
  expect(other, `${where}: ${control} also draws a second ring of the other kind`).toEqual([]);
  const owner = target.owner ?? target.owners?.[where.split(" ")[0]];
  if (owner) {
    expect(drawn[0].startsWith(owner), `${where}: the ring is on ${drawn[0]}, expected ${owner}`).toBe(true);
  }
  if (target.uncovered && owner) {
    expect(result.covered, `${where}: a child's fill covers the ring`).toEqual([]);
  }
  if (target.within) {
    expect(result.escapes, `${where}: the ring leaves ${target.within}`).toEqual([]);
  }
  expect(
    result.opaqueOutlines,
    `${where}: an outline with colour sits on the focused subtree`,
  ).toEqual([]);
  expect(
    result.carriers.length > 0,
    `${where}: the ring on ${control} has no outline under it, so a browser in forced colours drops it`,
  ).toBe(true);
}

test("each focused control draws exactly one ring", async ({ hub, page }, testInfo) => {
  const width = WIDTHS[testInfo.project.name];
  expect(width, `unknown project ${testInfo.project.name}`).toBeTruthy();

  // A comment, so the comment card holds a control to focus.
  await page.request.post(
    `${hub.baseUrl}/api/v1/artifacts/${hub.artifactId}/comments`,
    {
      headers: { Authorization: `Bearer ${hub.token}` },
      data: { author: "human", body: "a comment to put a control on the card" },
    },
  );

  await page.goto(`${hub.baseUrl}/`, { waitUntil: "load" });

  for (const theme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: theme });
    for (const target of [...RINGS, ...SINGLE_RINGS]) {
      const where = `${width} ${theme} ${target.name}`;
      if (target.widths && !target.widths.includes(width)) continue;
      // A wide stage opens the margin at 940px of the stage, which the 1100px
      // window does not leave it.
      if (target.wide && width === "1100px") await page.setViewportSize({ width: 1440, height: 800 });
      const missing = await enter(page, target, hub);
      expect(missing, `${where}: ${missing}`).toBeNull();
      try {
        for (const control of target.controls ?? [target.control]) {
          await holdOneRing(page, target, control, `${where} ${control}`);
        }
        if (target.spacing) {
          const spacing = await page.evaluate(connectSpacing);
          expect(spacing.error, `${where}: ${spacing.error}`).toBeUndefined();
          expect(
            spacing.gap > 0,
            `${where}: the token field and the Connect button touch (gap ${spacing.gap})`,
          ).toBe(true);
          expect(
            spacing.scrollWidth <= spacing.pageWidth,
            `${where}: the Connect form scrolls sideways`,
          ).toBe(true);
        }
      } finally {
        if (target.wide && width === "1100px") await page.setViewportSize({ width: 1100, height: 800 });
        await leave(page, target);
      }
    }
  }
});

// Walking a project feed with the Tab key: every stop draws one ring, on one
// element. A row that draws its own ring while a control inside it draws one
// too shows two, which a check on a named wrapper does not see.
test("a Tab walk over a project feed shows one ring per stop", async ({ hub, page }, testInfo) => {
  test.skip(testInfo.project.name !== "focus-desktop", "the feed is walked at the desktop width");
  await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/feed`, { waitUntil: "load" });
  await page.locator("main .shell-index .feed-row").first().waitFor();
  for (const theme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: theme });
    await page.evaluate(() => document.activeElement?.blur());
    const stops = [];
    let wraps = 0;
    const first = await page.evaluate(() => {
      document.body.dataset.walkStart = "1";
      return document.activeElement === document.body;
    });
    expect(first).toBe(true);
    for (let i = 0; i < 400; i += 1) {
      await page.keyboard.press("Tab");
      const stop = await page.evaluate(() => {
        const root = getComputedStyle(document.documentElement);
        const hex = /^#([0-9a-f]{6})$/i.exec(root.getPropertyValue("--accent").trim());
        const n = hex ? parseInt(hex[1], 16) : 0;
        const rgb = `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
        const describe = (el) =>
          el.tagName.toLowerCase() + (typeof el.className === "string" && el.className.trim() ? "." + el.className.trim().split(/\s+/).join(".") : "");
        const active = document.activeElement;
        if (!active || active === document.body || active === document.documentElement) return { wrapped: true };
        // A ring is a spread shadow with no offset, or the splitter's accent
        // bar; an offset edge such as the rail's current mark is not one.
        const ringed = new RegExp(`${rgb.replace(/[()]/g, "\\$&")} 0px 0px 0px \\d+px`);
        const rings = [...document.querySelectorAll("body *")]
          .filter((el) => el.getClientRects().length && (ringed.test(getComputedStyle(el).boxShadow) || getComputedStyle(el).backgroundImage.includes(rgb)))
          .map(describe);
        const label = `${describe(active)} ${active.getAttribute("aria-current") || active.getAttribute("aria-selected") || ""}`;
        return { wrapped: false, visible: active.matches(":focus-visible"), active: label, rings };
      });
      // Focus starts wherever the route left it, so the walk runs until it has
      // gone round the page once: it ends at the second wrap to the document.
      if (stop.wrapped) {
        wraps += 1;
        if (wraps === 2) break;
        continue;
      }
      if (stop.visible) stops.push(stop);
    }
    const names = stops.map((stop) => stop.active).join("\n");
    expect(names, `${theme}: the walk never met the current rail item`).toMatch(/rail-item[^\n]* (page|true)/);
    expect(names, `${theme}: the walk never met a selected segment`).toMatch(/^a (true|page)$/m);
    expect(names, `${theme}: the walk never met a feed link`).toMatch(/feed-link/);
    expect(stops.length, `${theme}: the walk reached no control`).toBeGreaterThan(3);
    for (const stop of stops) {
      expect(stop.rings, `${theme}: ${stop.active} shows ${stop.rings.length} rings`).toHaveLength(1);
    }
  }
});
