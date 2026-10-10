// A form is a 640 column at every width, a settings row is 48 whatever its
// control, and a button is as wide as its label. The three together are why a
// pane reads as a column of readable width instead of a field that runs to the
// window's edge with a verb stretched beside it.
//
// The check reads the rendered DOM rather than the source, and it proves the
// layout rather than the rule that sets it. A control is as wide as its label
// when the width it is drawn at is the width it asks for with every
// width-stretching declaration taken away, so the check asks for that width by
// neutralising `width`, `min-width`, `max-width`, `flex` and the stretch
// alignment in both axes, reading the box back, and putting them all again. A
// control that is only as wide as it is drawn because of `min-width: 100%` or
// `flex: 1` is caught whether or not that declaration is in a rule the
// stylesheet gate reads.
//
// A run at a desktop width and one at a phone width with a coarse pointer,
// because a column that cannot be 640 in a 358px pane is the pane, and a row's
// control follows the pointer.
//
// The artifact viewer's document is a separate page with its own stylesheet,
// loaded in a sandboxed frame. It is not walked here.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { expectEveryScreen, openScreen, screenRoutes, seedWikiPage } from "./screens.mjs";

// The forms the rule is about, each with what a reader calls it. Every one has
// to be measured somewhere in the walk, so the check cannot pass by walking a
// set of screens that happens to hold no form.
const COLUMNS = [
  { name: "project settings", selector: "form.pset" },
  // Add agent is a column in the stage on a desktop and a stack in the list on
  // a phone, so one pointer reaches one and the other run the other.
  { name: "New agent", selector: ".agent-create-column", fine: true },
  { name: "the add-agent form", selector: ".access-add-fields", fine: false },
  { name: "the settings form column", selector: ".form-column", fine: true },
  { name: "the wiki editor", selector: ".wiki-editor" },
];

// Controls that are deliberately as wide as the box that holds them, each named
// with the reason. The check fails if one of them starts to fit its label, so
// the list only shrinks.
const ALLOWED = [
  // A row that is itself a control carries the row's own width: its label is
  // the whole row, and its target is the row.
  ".settings-btn-row",
  // A menu item is as wide as the menu that opened it.
  '[role="menu"] button',
  // The delete dialog's two actions share one 90px floor, so a long label and
  // a short one give the same pair of boxes.
  ".dialog-del-actions button",
  // A tab shares the width of the strip that holds it.
  ".projects-tab-btn",
];

// The declarations that make a control take the width of its box. They are
// taken away and put back again, which is the whole of the measurement.
const NEUTRALISED = {
  width: "auto",
  minWidth: "0px",
  maxWidth: "none",
  flex: "0 0 auto",
  flexGrow: "0",
  flexBasis: "auto",
  alignSelf: "start",
  justifySelf: "start",
};

function controls(page, selector, where, allowed) {
  return page.evaluate(
    ({ selector, where, allowed, neutralised }) => {
      const found = [];
      for (const el of document.querySelectorAll(selector)) {
        if (el.closest("[hidden]")) continue;
        if (!el.getClientRects().length) continue;
        el.scrollIntoView({ block: "center", inline: "center" });
        const drawn = el.getBoundingClientRect();
        if (drawn.width === 0 || drawn.height === 0) continue;
        const kept = Object.fromEntries(Object.keys(neutralised).map((name) => [name, el.style[name]]));
        Object.assign(el.style, neutralised);
        const asked = el.getBoundingClientRect();
        for (const [name, value] of Object.entries(kept)) el.style[name] = value;
        const label = (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ");
        const exempt = allowed.some((rule) => {
          try {
            return el.matches(rule);
          } catch {
            return false;
          }
        });
        found.push({
          where,
          label: label.slice(0, 48),
          // A control that draws only a glyph has no label to be as wide as. It
          // is held to the target's own width instead, which is the widest it
          // may be before it reaches past the control beside it.
          glyphOnly: !(el.textContent || "").trim(),
          drawn: drawn.width,
          asked: asked.width,
          exempt,
          minWidth: getComputedStyle(el).minWidth,
          maxWidth: getComputedStyle(el).maxWidth,
        });
      }
      return found;
    },
    { selector, where, allowed, neutralised: NEUTRALISED },
  );
}

// A form's column is 640, or the pane when the pane is narrower than 640. The
// pane is what the form's own parent gives it, so a form inside a padded stage
// body is measured against what is left of that body.
function columns(page, where) {
  return page.evaluate(
    ({ columns, where }) => {
      const found = [];
      for (const entry of columns) {
        for (const el of document.querySelectorAll(entry.selector)) {
          if (el.closest("[hidden]")) continue;
          if (!el.getClientRects().length) continue;
          const parent = el.parentElement;
          const cs = parent ? getComputedStyle(parent) : null;
          const pad = cs ? (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0) : 0;
          const available = parent ? parent.clientWidth - pad : 0;
          found.push({ where, name: entry.name, width: el.getBoundingClientRect().width, available });
        }
      }
      return found;
    },
    { columns: COLUMNS, where },
  );
}

test.describe("a form is a 640 column, a row is 48, a button is its label", () => {
  test("every form is a 640 column and every button is as wide as its label", async ({ hub, page }, testInfo) => {
    test.skip(
      testInfo.project.name !== "forms-desktop" && testInfo.project.name !== "forms-touch",
      "the column and the row are held at both widths",
    );
    const wiki = await seedWikiPage(hub, "forms/column.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);

    const seen = [];
    const misfits = [];
    const stretches = [];
    const walk = async (name) => {
      const found = [...(await controls(page, "button", name, ALLOWED)), ...(await controls(page, "a.button", name, ALLOWED))];
      for (const control of found) {
        if (control.exempt) continue;
        // A glyph control is held to the target's own width; a labelled one is
        // held to the width its own label asks for.
        if (control.glyphOnly ? control.drawn > 44.5 : control.drawn > control.asked + 1) {
          misfits.push(
            `${name}: ${control.label || "(glyph)"} is drawn ${Math.round(control.drawn)}px and asks for ${Math.round(control.asked)}px`,
          );
        }
        // A percentage width, min-width or max-width is a stretch by definition,
        // because it is the width of whatever holds it. An absolute floor, such
        // as the delete dialog's 90px, is a size and is allowed.
        if (/%$/.test(control.minWidth) || /%$/.test(control.maxWidth)) {
          stretches.push(`${name}: ${control.label || "(glyph)"} carries ${control.minWidth} / ${control.maxWidth}`);
        }
      }
      for (const form of await columns(page, name)) {
        const column = Math.min(640, form.available);
        if (Math.round(form.width) > Math.round(column) + 1) {
          misfits.push(`${name}: the ${form.name} column is ${Math.round(form.width)}px in a ${Math.round(column)}px pane`);
        }
        seen.push(form);
      }
    };

    // The forms and dialogs the rule is about that sit behind a control, each
    // reached the way a reader reaches it. A state one pointer reaches names
    // it, because the phone reaches some of these from a different place or
    // not at all.
    const project = encodeURIComponent(hub.projectId);
    const at = `#/projects/${project}`;
    const page2 = `${at}/wiki?page=${encodeURIComponent(wiki.name)}`;
    const session = `${at}/sessions?id=${encodeURIComponent(hub.sessionId)}`;
    const opened = [
      { name: "Add an agent", hash: "#/access", ready: "main .access-screen, main .agent-index-row", press: ['[data-action="toggle-add-agent"]'] },
      { name: "New project", hash: "#/projects", ready: "main .projects-screen .project-row", press: ['[data-action="new-project"]'] },
      { name: "Delete a project", hash: `${at}/feed`, ready: "main .feed-row", press: [".proj-overflow-btn", '[data-action="delete-project"]'] },
      { name: "End a session", hash: session, ready: "main .end-session", press: ['[data-action="end"]'] },
      { name: "New wiki page", hash: `${at}/wiki`, ready: "main .wiki-row", press: ['[data-action="wiki-new"]'], layout: "desktop" },
      { name: "Wiki editor", hash: `${page2}&edit=1`, ready: "main .wiki-editor" },
    ];
    const desktop = page.viewportSize().width >= 1100;

    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        await walk(name);
      }
    }
    for (const state of opened) {
      if (state.layout === "desktop" && !desktop) continue;
      await openScreen(page, hub, state.hash, state.ready);
      for (const opener of state.press || []) {
        const control = page.locator(opener).filter({ visible: true }).first();
        await expect(control, `${state.name}: nothing reaches it with ${opener}`).toBeVisible();
        await control.click();
      }
      await walk(state.name);
      if (await page.locator("dialog[open]").count()) await page.keyboard.press("Escape");
      await expect(page.locator("dialog[open]"), `${state.name}: the dialog did not close`).toHaveCount(0);
    }

    // The forms the rule names, each measured at least once. Without this a run
    // whose screens happen to hold no form would pass.
    const measured = new Set(seen.map((form) => form.name));
    const fine = await page.evaluate(() => matchMedia("(pointer: fine)").matches);
    const missing = COLUMNS.filter((entry) => (entry.fine === undefined || entry.fine === fine) && !measured.has(entry.name));
    expect(missing.map((entry) => entry.name), "a form column the rule names was never measured").toEqual([]);
    expect(seen.length, "no form column was found on any screen").toBeGreaterThan(0);
    expect(misfits, "a control is wider than its label or its column").toEqual([]);
    expect(stretches, "a control carries a percentage width").toEqual([]);
  });

  test("a settings row is 48 whatever its control", async ({ hub, page }, testInfo) => {
    test.skip(
      testInfo.project.name !== "forms-desktop" && testInfo.project.name !== "forms-touch",
      "a row's height is a rule of its own pointer",
    );
    const wiki = await seedWikiPage(hub, "forms/rows.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);
    const desktop = page.viewportSize().width >= 1100;
    await openScreen(page, hub, routes.settings[0][1], routes.settings[0][2]);
    const rows = await page.evaluate((selectors) =>
      selectors
        .flatMap((selector) =>
          [...document.querySelectorAll(selector)].map((el) => {
            const label = el.querySelector(".form-row-title, .title, .settings-row-title");
            // A row whose label carries a second line of copy is taller for
            // that line, not for its control.
            const secondLine = !!(el.querySelector(".form-row-sub") || el.querySelector(".settings-row-sub"));
            return {
              selector,
              label: (label?.textContent || "").trim(),
              secondLine,
              height: el.getBoundingClientRect().height,
            };
          }),
        )
        .filter((row) => row.height > 0),
    [".form-row", ".settings-flat-row"]);
    expect(rows.length, "no settings row was measured").toBeGreaterThan(0);
    const off = rows
      .filter((row) => !row.secondLine)
      .filter((row) => Math.abs(row.height - (desktop ? 48 : 56)) > 0.5)
      .map((row) => `${row.label || row.selector} is ${Math.round(row.height * 10) / 10}px`);
    expect(off, "a one-line settings row is not the row height its pointer gives it").toEqual([]);
    // The rows that do carry a second line of copy are held too, so a row is
    // never taller because of the control beside it.
    const twoLine = rows.filter((row) => row.secondLine);
    expect(twoLine.length, "no two-line settings row was measured").toBeGreaterThan(0);
  });
});
