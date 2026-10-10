// One section-label style, short time on a history row, and a copy control that
// is a glyph on the row that owns the string.
//
// A section label is 12/600 uppercase sans with 0.06em tracking in the meta
// tone. The mono family is for literal data, so a label that carries a count or
// a path keeps it in a span of its own rather than setting the whole label in
// mono: a row of labels drawn two ways reads as two different things.
//
// A history row shows the short time every other row shows, with the full stamp
// available on request. The raw ISO stamp on a row is a row nobody can read at
// a glance, and the formatter that every other row uses already handles the
// boundaries.
//
// A copy control is never a text chip and never on the inset surface. It is a
// glyph, it is as wide as the target it answers to, and it carries no visible
// label. The session id is shown once in the stage header's meta line with the
// glyph on it, rather than a second time in a chip beside it.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

// The classes that draw a section label.
const LABELS =
  ".form-group-label, .settings-group-title, .agents-section-label, .access-section-label, .storage-section-label, .wiki-section-label, .wiki-history-day, .session-group-header, .brain-label, .settings-group-label, .shell-group-label";

test.describe("one section-label style", () => {
  test("every section label is the one style, and no label is set in the mono family", async ({ hub, page }) => {
    const wiki = await seedWikiPage(hub, "labels/style.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);

    const found = [];
    const mono = [];
    let held = 0;
    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        const labels = await page.$$eval(LABELS, (els) =>
          els
            .filter((el) => el.getClientRects().length)
            .map((el) => {
              const cs = getComputedStyle(el);
              return {
                text: (el.textContent || "").trim().slice(0, 28),
                size: cs.fontSize,
                weight: cs.fontWeight,
                family: cs.fontFamily,
                tracking: cs.letterSpacing,
                transform: cs.textTransform,
                mono: cs.fontFamily.toLowerCase().includes("mono"),
                // The literal data inside a label stays mono in its own span,
                // which is the only mono a label may carry.
                ownMono: el.classList.contains("mono"),
              };
            }),
        );
        for (const label of labels) {
          held += 1;
          if (label.size !== "12px" || label.weight !== "600" || label.transform !== "uppercase") {
            found.push(`${name}: ${label.text} is ${label.size}/${label.weight}/${label.transform}`);
          }
          if (Math.abs(parseFloat(label.tracking) - 0.72) > 0.05) {
            found.push(`${name}: ${label.text} tracks ${label.tracking}`);
          }
          if (label.mono && !label.ownMono) {
            mono.push(`${name}: ${label.text} is set in the mono family`);
          }
        }
      }
    }
    expect(held, "no section label was found on any screen").toBeGreaterThan(0);
    expect(found, "a section label is not the one label style").toEqual([]);
    expect(mono, "a section label is set in the mono family").toEqual([]);
  });
});

test.describe("a history row shows the short time", () => {
  test("no history row's visible text is a raw ISO stamp", async ({ hub, page }) => {
    const wiki = await seedWikiPage(hub, "labels/history.md");
    const project = encodeURIComponent(hub.projectId);
    await page.goto(`${hub.baseUrl}/#/projects/${project}/wiki?page=${encodeURIComponent(wiki.name)}&history=1`);
    await expect(page.locator("main .wiki-versions")).toBeVisible();
    const stamps = await page.$$eval(".wiki-version, .wiki-change", (rows) =>
      rows.map((row) => ({
        text: (row.textContent || "").trim(),
        iso: row.textContent.match(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/)?.[0] ?? null,
        // The full stamp is available on request: the time element carries it as
        // its title and its accessible name.
        titled: [...row.querySelectorAll("time[datetime]")].map((el) => ({
          title: el.getAttribute("title") || "",
          name: el.getAttribute("aria-label") || "",
        })),
      })),
    );
    expect(stamps.length, "no history row was found").toBeGreaterThan(0);
    const raw = stamps.filter((row) => row.iso).map((row) => `${row.text.slice(0, 24)} still shows ${row.iso}`);
    expect(raw, "a history row shows a raw ISO stamp").toEqual([]);
    const untitled = stamps.filter((row) => row.titled.length === 0);
    expect(untitled.length, "no history row carries the full stamp on request").toBeLessThan(stamps.length);
  });

  test("the time formatter spells the boundaries the way the app does", async () => {
    const { relative } = await import("../web/time.mjs");
    const now = Date.parse("2026-10-10T12:00:00Z");
    const at = (offset) => new Date(now + offset).toISOString();
    // The formatter takes a timestamp and an optional now, so the boundaries are
    // held rather than left to whatever the clock says when the test runs.
    // The boundaries a row cares about: now, minutes, hours, the calendar day
    // and a date from another year. What matters is that none of them is an ISO
    // stamp, so each is held against the shape it takes rather than a string
    // that drifts if the units change.
    const iso = /^\d{4}-\d{2}-\d{2}T/;
    expect(relative(at(0), now)).toBe("now");
    expect(relative(at(-4 * 60_000), now)).toBe("4m");
    expect(relative(at(-3 * 3_600_000), now)).toBe("3h");
    expect(relative(at(-3 * 86_400_000), now)).not.toMatch(/^\d{4}-\d{2}-\d{2}T/);
    expect(iso.test(relative(at(-400 * 86_400_000), now))).toBe(false);
    expect(relative(at(-400 * 86_400_000), now)).toContain("2025");
  });
});

test.describe("a copy control is a glyph on the row that owns the string", () => {
  test("every copy control is a glyph with no visible label and no inset surface", async ({ hub, page }) => {
    const wiki = await seedWikiPage(hub, "labels/copy.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);

    const project = encodeURIComponent(hub.projectId);
    const session = encodeURIComponent(hub.sessionId);
    const found = [];
    const inset = [];
    let held = 0;
    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        const controls = await page.$$eval('button[aria-label^="Copy"], a[aria-label^="Copy"]', (els) =>
          els
            .filter((el) => el.getClientRects().length)
            .map((el) => {
              const r = el.getBoundingClientRect();
              const cs = getComputedStyle(el);
              return {
                name: el.getAttribute("aria-label") || "",
                text: (el.textContent || "").trim(),
                width: Math.round(r.width * 10) / 10,
                svg: el.querySelector("svg") ? 1 : 0,
                background: cs.backgroundColor,
              };
            }),
        );
        for (const control of controls) {
          held += 1;
          if (control.text) found.push(`${name}: ${control.name} carries the visible text ${JSON.stringify(control.text)}`);
          if (!control.svg) found.push(`${name}: ${control.name} draws no glyph`);
          if (control.width > 44.5) found.push(`${name}: ${control.name} is ${control.width}px wide`);
          // --surface-2 is the inset surface, and a copy control never sits on it.
          if (control.background !== "rgba(0, 0, 0, 0)" && control.background !== "transparent") {
            inset.push(`${name}: ${control.name} is drawn on ${control.background}`);
          }
        }
      }
    }
    // The session id appears once in the stage header's meta line, with the copy
    // glyph on the row that owns it, and nowhere else in the stage.
    await openScreen(page, hub, `#/session?project=${project}&id=${session}`, "main .session-detail-header");
    const stage = page.locator(".session-detail-header");
    const copies = page.locator('.session-detail-header [aria-label^="Copy full session id"], .session-copy-id');
    await expect(copies, "the copy control is not on the row that owns the id").toHaveCount(1);
    const body = (await stage.textContent()) || "";
    const id = await page.locator(".session-detail-header .meta .mono").first().textContent();
    expect(id && id.length > 3, "the stage header carries no session id").toBe(true);
    const shown = (body.match(new RegExp(id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g")) || []).length;
    expect(shown, "the session id appears more than once in the stage").toBeLessThanOrEqual(1);

    expect(held, "no copy control was found on any screen").toBeGreaterThan(0);
    expect(found, "a copy control is not a glyph on its own row").toEqual([]);
    expect(inset, "a copy control is drawn on the inset surface").toEqual([]);
  });
});
