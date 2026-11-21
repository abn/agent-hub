// What the reader can reach with a list, a filter, and one key: the rows a query
// keeps standing under the groups that hold them, the verbs a and r find on an
// ask on Now, where / lands when the screen is showing a field and when it is
// not, and what a chip's number counts.
//
// Nothing here reads geometry or the source text, so a redesign that keeps the
// behaviour passes. Every wait is a web-first assertion on a rendered value, so
// a slow paint is waited out and a missing one fails with a message rather than
// with a timeout.
//
// The filter is asked on a project feed, which sets its rows beside the day
// heading. A walk that judged a group by its own block hid a group that still
// held rows, which is how typing into a filter once emptied the list while the
// count line above it still said what had matched.

import { expect } from "@playwright/test";
import { BODY, open, test } from "./app.mjs";

// The group headings a filter judges, which is the filter's own list rather than
// a second one here, so a screen that draws another shape is walked by both.
const HEADINGS = ".day, .hub-group-header";
// The rows the keyboard map moves through, as Now registers them.
const KEY_ROWS = ".now-row:not(.now-quiet[hidden] .now-row)";
// The field Search opens on, named by its own label.
const SEARCH_FIELD = "Search the feed, artifacts and session brains";

// What the index holds, asked of the rendered page: every row and whether the
// reader can see it, every group heading with the rows it heads, and every
// heading that names a group. A heading's rows are read the way the filter
// reads them, from the heading up to the next one, so the two shapes (rows
// inside the group, rows beside the heading) are described by one walk.
const readIndex = (page) =>
  page.evaluate((headings) => {
    const body = document.querySelector(".shell-index .shell-body");
    const text = (el) => (el.textContent || "").replace(/\s+/g, " ").trim();
    const drawn = (el) => !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
    const headed = (head) => {
      const held = [];
      for (let node = head; node; node = node.nextElementSibling) {
        if (node !== head && node.matches(headings)) break;
        held.push(...node.querySelectorAll(".row"));
      }
      return held.map(text);
    };
    const rows = [...body.querySelectorAll(".row")];
    return {
      rows: rows.map((row) => ({ text: text(row), visible: drawn(row) })),
      groups: [...body.querySelectorAll(headings)].map((head) => ({
        key: head.dataset.group || text(head).slice(0, 24),
        visible: drawn(head),
        rows: headed(head),
      })),
    };
  }, HEADINGS);

// What one query owes the reader, read off the list as it was before it: the
// rows that match, and for each group whether it heads one. A group stays while
// any row it heads matches, and a heading that names such a group stays with it.
const owes = (term, before) => {
  const needle = term.toLowerCase();
  const matches = (said) => said.toLowerCase().includes(needle);
  return {
    matching: before.rows.filter((row) => matches(row.text)).map((row) => row.text),
    holding: Object.fromEntries(before.groups.map((group) => [group.key, group.rows.some(matches)])),
  };
};

// Typing a query stands or hides a group by whether it still heads a row the
// query kept, names that in one count line, and puts every heading back when the
// query is cleared. What it does with a row of its own is held separately, in
// the check that says why.
async function filterHoldsItsGroups(page, { name, terms, miss, where }) {
  const field = page.getByRole("searchbox", { name });
  await expect(field).toBeVisible();
  const before = await readIndex(page);
  const groups = page.locator(BODY).locator(HEADINGS);
  const count = page.locator(`${BODY} .shell-count`);

  for (const term of terms) {
    const want = owes(term, before);
    expect(want.matching.length, `${where}: nothing on the list matches ${term}`).toBeGreaterThan(0);
    await field.fill(term);
    // The count line is the only statement of what the query did, and the same
    // pass that writes it stands or hides the groups, so waiting on it is
    // waiting on the filter having run.
    await expect(count).toBeVisible();
    await expect(count).toHaveText(`${want.matching.length} of ${before.rows.length} match "${term}"`);
    for (const [at, group] of before.groups.entries()) {
      if (!group.visible) continue;
      if (want.holding[group.key]) await expect(groups.nth(at)).toBeVisible();
      else await expect(groups.nth(at)).toBeHidden();
    }
    // The count line gives way to the headings again when the query is cleared.
    await field.fill("");
    await expect(count).toBeHidden();
    for (const [at, group] of before.groups.entries()) {
      if (group.visible) await expect(groups.nth(at)).toBeVisible();
    }
  }

  // A query nothing matches leaves no group standing over an empty list.
  await field.fill(miss);
  await expect(count).toHaveText(`0 of ${before.rows.length} match "${miss}"`);
  for (const [at, group] of before.groups.entries()) {
    if (group.visible) await expect(groups.nth(at)).toBeHidden();
  }
}

// A query leaves the reader with the rows it matched and nothing else: the rows
// it did not match are gone from the screen, and a query that matched nothing
// leaves an empty list rather than one row of a day with no heading over it.
async function queryTakesTheRest(page, { name, term }) {
  const field = page.getByRole("searchbox", { name });
  await expect(field).toBeVisible();
  const before = await readIndex(page);
  const want = owes(term, before);
  expect(want.matching.length, `nothing on the list matches ${term}`).toBeGreaterThan(0);
  const count = page.locator(`${BODY} .shell-count`);
  const rows = page.locator(`${BODY} .row`);

  await field.fill(term);
  await expect(count).toHaveText(`${want.matching.length} of ${before.rows.length} match "${term}"`);
  for (const [at, row] of before.rows.entries()) {
    if (!row.visible || want.matching.includes(row.text)) continue;
    await expect(rows.nth(at)).toBeHidden();
  }
  for (const [at, row] of before.rows.entries()) {
    if (row.visible && want.matching.includes(row.text)) await expect(rows.nth(at)).toBeVisible();
  }

  await field.fill("");
  await expect(count).toBeHidden();
  await expect(page.locator(`${BODY} .row:visible`)).toHaveCount(
    before.rows.filter((row) => row.visible).length,
  );
}

// Walk the selection onto the row carrying this text, as a reader would: the map
// moves with j and k, and the row it lands on is the one the tab ring stops at,
// which is what is waited for rather than a pause after the last key.
async function walkTo(page, needle) {
  // The map is deliberately deaf while the keyboard is in a field, so a check
  // that came from a filter field hands the keyboard back to the page first,
  // which is what leaving the field does.
  await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  const at = await page.evaluate(
    ([rows, want]) => {
      const list = [...document.querySelectorAll(rows)];
      return {
        want: list.findIndex((row) => row.textContent.includes(want)),
        have: list.findIndex((row) => row.tabIndex === 0),
      };
    },
    [KEY_ROWS, needle],
  );
  if (at.want < 0) return false;
  const steps = at.want - (at.have >= 0 ? at.have : 0);
  for (let step = 0; step < Math.abs(steps); step += 1) {
    await page.keyboard.press(steps >= 0 ? "j" : "k");
  }
  await expect(page.locator('.now-row[tabindex="0"]')).toContainText(needle);
  return true;
}

test.describe("the Now chips", () => {
  // A chip is a filter, so its number counts what it shows: the Needs you chip
  // counts the asks that need the reader now, and pressing it leaves those and
  // the ones that can wait, and nothing that merely happened.
  test("the needs-you chip's number counts the asks it shows", async ({ hub, page }) => {
    await open(page, hub, "#/home", "");
    const chip = page.getByRole("button", { name: /^Needs you \d+$/ });
    await expect(chip).toBeVisible();
    const counted = Number((await chip.textContent()).replace(/\D/g, ""));
    expect(counted, "Now drew no asks, so the chip filters nothing to count").toBeGreaterThan(0);
    await chip.click();
    await expect(page.getByRole("button", { name: /^Needs you \d+$/ })).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByRole("heading", { name: /Since you looked/ })).toHaveCount(0);
    await expect(page.locator(".now-ask:not(.is-later)")).toHaveCount(counted);
  });
});

test.describe("the list filter", () => {
  test("leaves the rows it matched under the group that holds them, on a project feed", async (
    { hub, page },
    testInfo,
  ) => {
    // A phone keeps a project's filter in the DOM and out of the layout until
    // its own control opens it, so there is no field there to type into and the
    // desktop is where the feed's filter is a filter.
    test.skip(testInfo.project.name !== "desktop", "a phone has no field to type this into");
    const { finished, searchMiss } = hub.fixture;
    await open(page, hub, `#/projects/${hub.projectId}/feed`, "feed");
    await filterHoldsItsGroups(page, {
      name: "Filter events",
      terms: [finished.split(" ")[1]],
      miss: searchMiss,
      where: "project feed",
    });
  });

  // A query the reader typed has to take the rows it did not match off the
  // screen, and not only out of the count. `.row`'s own `display` beat the
  // browser's `[hidden]` rule, so the filter's `hidden` flag left the row
  // drawn; `.row[hidden] { display: none }` is what makes these hold.
  test("takes the rows it did not match off the screen, on a project feed", async (
    { hub, page },
    testInfo,
  ) => {
    test.skip(testInfo.project.name !== "desktop", "a phone has no field to type this into");
    const { finished } = hub.fixture;
    await open(page, hub, `#/projects/${hub.projectId}/feed`, "feed");
    await queryTakesTheRest(page, { name: "Filter events", term: finished.split(" ")[1] });
  });
});

test.describe("the single-key verbs", () => {
  // An ask on Now names the keys that press its verbs, so a and r reach the
  // approval and the reply of the selected ask, and once an ask is answered
  // the selection moves to the ask that took its place.
  test("a and r reach the verbs of the selected ask", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the key map moves a desktop stream");
    const { approval, question } = hub.fixture;
    await open(page, hub, "#/inbox", "");
    const asks = page.locator(".now-ask");
    const before = await asks.count();

    await expect(walkTo(page, approval), `no row carries ${approval}`).resolves.toBe(true);
    const at = await asks.evaluateAll((rows, want) => rows.findIndex((row) => row.textContent.includes(want)), approval);
    await page.keyboard.press("a");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("heading", { level: 2 })).toContainText(approval);
    await dialog.getByRole("button", { name: "Approve" }).click();
    await expect(asks).toHaveCount(before - 1);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // The ask now in its place is the one selected and focused.
    await expect(asks.nth(Math.min(at, before - 2))).toBeFocused();

    await expect(walkTo(page, question), `no row carries ${question}`).resolves.toBe(true);
    await page.keyboard.press("r");
    const field = page.getByRole("textbox", { name: "Your answer" });
    await expect(field).toBeVisible();
    // The composer opens under the ask it answers.
    const card = field.locator("xpath=ancestor::form[1]/preceding-sibling::*[1]");
    await expect(card).toHaveClass(/\bnow-ask\b/);
    await expect(card).toContainText(question);
    await expect(field).toBeFocused();
  });
});

test.describe("the search key", () => {
  // A field with no box is accepted by the DOM and does nothing, so the key was
  // swallowed on the project segments that keep their filter out of the phone's
  // layout. The fall-through to Search is the phone's case, which is why the
  // check runs at 390 and not at 1440.
  test("/ reaches the field the screen shows, or falls through to Search", async (
    { hub, page },
    testInfo,
  ) => {
    test.skip(testInfo.project.name !== "phone", "a desktop shows the filter this key is about");
    const { searchTerm } = hub.fixture;
    for (const segment of ["now", "feed", "wiki", "artifacts", "sessions"]) {
      const where = segment === "now" ? "phone Now" : `phone project ${segment}`;
      if (segment === "now") await open(page, hub, "#/home", "");
      else await open(page, hub, `#/projects/${hub.projectId}/${segment}`, segment);
      await page.keyboard.press("/");
      const search = page.getByRole("textbox", { name: SEARCH_FIELD });
      await expect(page, `${where}: / did not fall through to Search`).toHaveURL(/#\/search/);
      await expect(search, `${where}: Search opened without the keyboard in its field`).toBeFocused();
      await search.pressSequentially(searchTerm);
      await expect(search).toHaveValue(searchTerm);
      await expect(page, `${where}: the query never reached the address`).toHaveURL(
        new RegExp(`#/search\\?q=${searchTerm}`),
      );
    }
  });
});
