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

// The action rows the coarse-pointer rule is about: a dialog's or a form's
// footer, holding the verbs a reader commits with. The phone's add-agent form
// is a column whose fields and one submit sit together, so its action is the
// submit and the row is the form, and the desktop's add-agent column holds its
// own pair. Each one has to be measured somewhere in the walk, so the check
// cannot pass by walking a set of screens that happens to hold no action row.
const ACTION_ROWS = [
  { selector: ".dialog-actions" },
  { selector: ".dialog-del-actions" },
  { selector: ".project-create-actions" },
  { selector: ".pset-actions" },
  { selector: ".agent-create-actions", fine: true },
  { selector: ".wiki-editor-actions" },
  { selector: ".session-actions-footer" },
  { selector: ".access-add-fields", fine: false },
];

// Under a coarse pointer an action row's controls take the sheet's width
// between them, so the label check cannot hold them and the third one does. A
// button in one of these rows that is not named here is still held to its label
// at either pointer, and the delete dialog's footer is already allowed at both
// because it shipped this way before the rule was written down.
const ALLOWED_COARSE = [
  ".dialog-actions :is(button, .button)",
  ".project-create-actions button",
  ".pset-actions button",
  ".agent-create-actions button",
  ".wiki-editor-actions :is(button, .button)",
  ".session-actions-footer button",
  ".access-add-fields button",
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

// The forms and dialogs the rule is about that sit behind a control, each
// reached the way a reader reaches it. A state one pointer reaches names it,
// because the phone reaches some of these from a different place or not at all.
const openedStates = (hub, wiki) => {
  const project = encodeURIComponent(hub.projectId);
  const at = `#/projects/${project}`;
  const page2 = `${at}/wiki?page=${encodeURIComponent(wiki.name)}`;
  const session = `${at}/sessions?id=${encodeURIComponent(hub.sessionId)}`;
  return [
    { name: "Add an agent", hash: "#/access", ready: "main .access-screen, main .agent-index-row", press: ['[data-action="toggle-add-agent"]'] },
    { name: "New project", hash: "#/projects", ready: "main .projects-screen .project-row", press: ['[data-action="new-project"]'] },
    { name: "Delete a project", hash: `${at}/feed`, ready: "main .feed-row", press: [".proj-overflow-btn", '[data-action="delete-project"]'] },
    { name: "End a session", hash: session, ready: "main .end-session", press: ['[data-action="end"]'] },
    { name: "New wiki page", hash: `${at}/wiki`, ready: "main .wiki-row", press: ['[data-action="wiki-new"]'], layout: "desktop" },
    { name: "Wiki editor", hash: `${page2}&edit=1`, ready: "main .wiki-editor" },
  ];
};

// An action row as the third check reads it: the width it has to give, the
// share each control is drawn at, and whether its width is accounted for. A row
// divides between its controls, so a row is full when its children and their
// gaps account for it and a control's share is an equal part of what the
// controls take. A column stretches its children across it, so a column is full
// when every child spans it and a control's share is the column's own width.
function actionRows(page, where, selectors) {
  return page.evaluate(
    ({ selectors, where }) => {
      const isControl = (el) => el.tagName === "BUTTON" || (el.tagName === "A" && el.classList.contains("button"));
      const found = [];
      for (const selector of selectors) {
        for (const row of document.querySelectorAll(selector)) {
          if (row.closest("[hidden]")) continue;
          if (!row.getClientRects().length) continue;
          const style = getComputedStyle(row);
          const pad = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0);
          const content = row.clientWidth - pad;
          if (content === 0) continue;
          const items = [...row.children].filter((el) => el.getClientRects().length);
          if (!items.length) continue;
          const column = !style.flexDirection.startsWith("row");
          const gap = parseFloat(column ? style.rowGap : style.columnGap) || 0;
          const controls = items.filter(isControl);
          if (!controls.length) continue;
          const widths = items.map((el) => el.getBoundingClientRect().width);
          const accounted = column
            ? Math.max(...widths)
            : widths.reduce((sum, width) => sum + width, 0) + (items.length - 1) * gap;
          // What the controls have to divide: the row's width, less what the
          // other children take and less every gap the row carries, because a
          // flex row divides its free space and the gaps are not free space.
          let share = content;
          if (!column) {
            const others = items.filter((el) => !isControl(el));
            const space = content
              - others.reduce((sum, el) => sum + el.getBoundingClientRect().width, 0)
              - (items.length - 1) * gap;
            share = space / controls.length;
          }
          found.push({
            where,
            selector,
            column,
            content,
            accounted,
            shares: controls.map((el) => ({
              label: (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 32),
              width: el.getBoundingClientRect().width,
              share,
            })),
          });
        }
      }
      return found;
    },
    { selectors, where },
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
    const fine = await page.evaluate(() => matchMedia("(pointer: fine)").matches);
    const allowed = fine ? ALLOWED : ALLOWED.concat(ALLOWED_COARSE);

    const seen = [];
    const misfits = [];
    const stretches = [];
    const walk = async (name) => {
      const found = [...(await controls(page, "button", name, allowed)), ...(await controls(page, "a.button", name, allowed))];
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
    const opened = openedStates(hub, wiki);
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

  test("a coarse pointer divides an action row between its actions", async ({ hub, page }, testInfo) => {
    test.skip(
      testInfo.project.name !== "forms-touch",
      "an action row divides only under a coarse pointer",
    );
    const wiki = await seedWikiPage(hub, "forms/actions.md");
    const routes = screenRoutes(hub, wiki);
    await page.goto(`${hub.baseUrl}/`);
    await expectEveryScreen(page, routes);
    const opened = openedStates(hub, wiki);
    const fine = await page.evaluate(() => matchMedia("(pointer: fine)").matches);
    const selectors = ACTION_ROWS.filter((row) => row.fine === undefined || row.fine === fine).map((row) => row.selector);

    const measured = [];
    const ragged = [];
    const empty = [];
    const walk = async (name) => {
      for (const row of await actionRows(page, name, selectors)) {
        measured.push(row);
        // The row is full: its children and their gaps account for the width it
        // has to give, which is what keeps a verb from sitting beside an empty
        // sheet.
        if (Math.abs(row.accounted - row.content) > 1.5) {
          const what = row.column ? "its widest child spans" : "its children account for";
          empty.push(
            `${name}: ${row.selector} ${what} ${Math.round(row.accounted)}px of ${Math.round(row.content)}px`,
          );
        }
        // The controls share the row. One of them at the row's whole width while
        // the others stay at their labels is the shape this refuses.
        for (const item of row.shares) {
          if (Math.abs(item.width - item.share) > 1.5) {
            ragged.push(
              `${name}: ${row.selector} draws ${item.label || "(glyph)"} at ${Math.round(item.width)}px where the row gives ${Math.round(item.share)}px`,
            );
          }
        }
      }
    };

    for (const entries of Object.values(routes)) {
      for (const [name, hash, ready] of entries) {
        await openScreen(page, hub, hash, ready);
        await walk(name);
      }
    }
    for (const state of opened) {
      if (state.layout === "desktop" && !(page.viewportSize().width >= 1100)) continue;
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

    // Every row the rule names is measured somewhere, so a run whose screens
    // happen to hold no action row cannot pass.
    const missing = ACTION_ROWS.filter(
      (row) => (row.fine === undefined || row.fine === fine)
        && !measured.some((found) => found.selector === row.selector),
    );
    expect(missing.map((row) => row.selector), "an action row the rule names was never measured").toEqual([]);
    expect(measured.length, "no action row was found on any screen").toBeGreaterThan(0);
    expect(empty, "an action row leaves the sheet's width unaccounted for").toEqual([]);
    expect(ragged, "an action row's controls do not share its width").toEqual([]);
  });
});
