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
    name: "reply composer",
    wrapper: ".composer-row",
    controls: [".composer-field", ".composer-send"],
    ring: "focus",
    route: "#/inbox?open={question}",
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
    name: "connect submit",
    wrapper: "form[data-action='connect']",
    control: ".connect-submit-btn",
    ring: "focus",
    route: "#/connect",
  },
  {
    name: "comment composer",
    wrapper: ".hub-composer-field",
    control: ".hub-composer-field textarea",
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
  const outlines = subtree.map((el) => opaqueOutline(el)).filter(Boolean);
  const carriers = subtree.filter((el) => {
    const cs = getComputedStyle(el);
    return cs.outlineStyle !== "none" && cs.outlineWidth !== "0px";
  });
  return {
    focused: document.activeElement === control,
    focusVisible: control.matches(":focus-visible"),
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
    .replace("{artifact}", hub.artifactId);

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
        await leave(page, target);
      }
    }
  }
});
