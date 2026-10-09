// Control sizing follows the pointer. On a fine pointer no control is taller
// than the band that holds it: a control in a pane's 52px header or its 40px
// control row is drawn at 32px, stays inside the band, and still draws its
// glyph. On a coarse pointer every button, chip, link and field answers to a
// 44 by 44 target, by its own box or by the hit area around it, keeps its
// glyph in view, and has no other control over its centre. The target is read
// the way a finger meets it: the element at the centre of the control and 21px
// either side of it in each axis is the control. Hit testing lands on whole
// pixels, so 21 rather than 22 is the furthest point inside a 44px square
// around the centre.
//
// Both walk every screen the router registers, so a new screen is held here
// the moment it is added.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

const BAND_CONTROLS = ["button", "link", "tab", "radio", "checkbox", "switch", "searchbox", "textbox", "combobox"];
// A chip is a button, so buttons, radios and fields cover "every button, chip
// and field". Tabs are held too, because a tab is pressed like a button, and so
// are links, which is how Edit and Cancel are drawn. A link that is a whole row
// is the row's target and is held by the row's own height instead, and a link
// inside a sentence is text rather than a control, as WCAG's inline exception
// has it.
const TARGETS = ["button", "link", "tab", "radio", "checkbox", "switch", "searchbox", "textbox", "combobox"];

// Controls DESIGN.md records as answering to less than 44 on a phone, each with
// the reason. The check fails if one of them starts to pass, so the list only
// shrinks.
const KNOWN_GAPS = [
  // The project switcher scrolls sideways in a 36px track, and a scroll box
  // clips the hit area of what it holds.
  { screens: ["Feed", "Artifacts", "Sessions", "Wiki", "the bare feed address", "the bare sessions address", "the bare artifacts address"], role: "tab" },
  // The viewer has no phone frame: its version line is 16px, between the band
  // and the document.
  { screens: ["Artifact viewer"], role: "button", name: /^Version / },
];

const knownGap = (screen, control) =>
  KNOWN_GAPS.find(
    (gap) =>
      gap.screens.includes(screen) &&
      gap.role === control.role &&
      (!gap.name || gap.name.test(control.name)),
  );

test.describe("control sizing", () => {
  test("no band control is taller than 32 or leaves its band on a fine pointer", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "sizing-desktop", "the desktop frame is a fine-pointer rule");
    const wiki = await seedWikiPage(hub, "sizing/fine.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    expect(await page.evaluate(() => matchMedia("(pointer: fine)").matches), "the project has no fine pointer").toBe(true);
    await expectEveryScreen(page, routes);

    const found = [];
    let held = 0;
    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        // A band is a pane's 52px header at the top of the window, or the 40px
        // control row under it.
        const bands = await page.evaluate(() =>
          [...document.querySelectorAll("main *")]
            .map((el) => el.getBoundingClientRect())
            .filter(
              (r) =>
                r.width > 0 &&
                ((Math.abs(r.top) < 0.5 && Math.abs(r.height - 52) < 0.5) ||
                  (Math.abs(r.top - 52) < 0.5 && Math.abs(r.height - 40) < 0.5)),
            )
            .map((r) => ({ left: r.left, right: r.right, top: r.top, bottom: r.bottom })),
        );
        expect(bands.length, `${name}: no 52px header or 40px control row was found`).toBeGreaterThan(0);
        for (const role of BAND_CONTROLS) {
          const boxes = await page.getByRole(role).evaluateAll((els) =>
            els
              .map((el) => {
                const r = el.getBoundingClientRect();
                const label = el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || "";
                const glyph = el.querySelector("svg");
                return {
                  name: label.trim().replace(/\s+/g, " ").slice(0, 40),
                  left: r.left,
                  top: r.top,
                  width: r.width,
                  height: r.height,
                  glyph: glyph ? glyph.getBoundingClientRect().width : null,
                };
              })
              .filter((box) => box.width > 0 && box.height > 0),
          );
          for (const box of boxes) {
            const x = box.left + box.width / 2;
            const y = box.top + box.height / 2;
            const band = bands.find((b) => x >= b.left && x <= b.right && y >= b.top && y <= b.bottom);
            if (!band) continue;
            held += 1;
            const spills = box.top < band.top - 0.5 || box.top + box.height > band.bottom + 0.5;
            if (box.height > 32.5 || spills) {
              found.push(`${name}: ${role} "${box.name}" is ${Math.round(box.height)}px${spills ? " and leaves its band" : ""}`);
            }
            // A smaller box must not squeeze out what it draws.
            if (box.glyph === 0) found.push(`${name}: ${role} "${box.name}" draws its glyph 0px wide`);
          }
        }
      }
    }
    expect(held, "no control was found in any band").toBeGreaterThan(0);
    expect(found, "a band control is taller than 32 or leaves its band").toEqual([]);
  });

  test("every button, chip, link and field answers to a 44 by 44 target on a coarse pointer", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "sizing-touch", "the target is a coarse-pointer rule");
    const wiki = await seedWikiPage(hub, "sizing/coarse.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches), "the project has no coarse pointer").toBe(true);
    await expectEveryScreen(page, routes);

    const found = [];
    const stillKnown = new Set();
    let held = 0;
    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        for (const role of TARGETS) {
          const targets = await page.getByRole(role).evaluateAll((els) =>
            els.flatMap((el) => {
              if (!el.getClientRects().length) return [];
              if (el.matches(".row, .row a, [role=treeitem], p a")) return [];
              el.scrollIntoView({ block: "center", inline: "center" });
              const r = el.getBoundingClientRect();
              if (r.width === 0 || r.height === 0) return [];
              const owns = (x, y) => {
                const hit = document.elementFromPoint(x, y);
                if (!hit) return false;
                if (hit === el || el.contains(hit)) return true;
                return [...(el.labels || [])].some((label) => label.contains(hit));
              };
              const describe = (hit) =>
                hit ? `${hit.tagName.toLowerCase()}.${String(hit.className?.baseVal ?? hit.className ?? "").split(" ")[0]}` : "nothing";
              const label = el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || "";
              const entry = (misses) => [
                {
                  name: label.trim().replace(/\s+/g, " ").slice(0, 40),
                  size: `${Math.round(r.width)}x${Math.round(r.height)}`,
                  misses,
                },
              ];
              const x = r.left + r.width / 2;
              const y = r.top + r.height / 2;
              if (!owns(x, y)) {
                // Another control's box or hit area over this one's centre is a
                // miss. Chrome over it (a sticky band, a sheet's backdrop) is
                // not this control's target to answer for.
                const hit = document.elementFromPoint(x, y);
                const other = hit?.closest("button, a, input, select, textarea, label, [role]");
                return other && !other.matches("[role=dialog], [role=tabpanel], [role=navigation], [role=main], [role=region], [role=group]")
                  ? entry([`its centre meets ${describe(other)}`])
                  : [];
              }
              // A glyph the control or its field draws stays on top: a field
              // painted over its own magnifier hides it. The glyph takes no
              // pointer events, so it is made hittable for the one probe.
              const glyphs = [el, ...(el.labels || [])].flatMap((host) => [...host.querySelectorAll("svg")]);
              const covered = glyphs.flatMap((glyph) => {
                const g = glyph.getBoundingClientRect();
                if (g.width === 0 || g.height === 0) return [];
                const was = glyph.style.pointerEvents;
                glyph.style.pointerEvents = "auto";
                const hit = document.elementFromPoint(g.left + g.width / 2, g.top + g.height / 2);
                glyph.style.pointerEvents = was;
                return hit && (glyph === hit || glyph.contains(hit) || hit.contains(glyph))
                  ? []
                  : [`its glyph is under ${describe(hit)}`];
              });
              const reach = 21;
              const misses = [
                [-reach, 0],
                [reach, 0],
                [0, -reach],
                [0, reach],
              ]
                .filter(([dx, dy]) => !owns(x + dx, y + dy))
                .map(([dx, dy]) => {
                  const side = dx < 0 ? "left" : dx > 0 ? "right" : dy < 0 ? "above" : "below";
                  return `${side} meets ${describe(document.elementFromPoint(x + dx, y + dy))}`;
                });
              return entry([...misses, ...covered]);
            }),
          );
          for (const target of targets) {
            held += 1;
            const control = { role, name: target.name };
            const gap = knownGap(name, control);
            if (gap) {
              if (target.misses.length) stillKnown.add(gap);
              continue;
            }
            if (target.misses.length) {
              found.push(`${name}: ${role} "${target.name}" (${target.size}): ${target.misses.join(", ")}`);
            }
          }
        }
      }
    }
    expect(held, "no control was found on any screen").toBeGreaterThan(0);
    expect(found, "a control answers to less than 44 by 44 under a coarse pointer").toEqual([]);
    expect(
      KNOWN_GAPS.filter((gap) => !stillKnown.has(gap)).map((gap) => gap.screens[0]),
      "a recorded gap now passes: take it off the list and out of DESIGN.md",
    ).toEqual([]);
  });
});
