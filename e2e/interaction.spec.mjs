// What the reader can reach with a list, a filter, and one key: the rows a query
// keeps standing under the groups that hold them, the verbs a and r find beside
// an inbox row rather than only on it, where / lands when the screen is showing
// a field and when it is not, and what a filter chip's number counts.
//
// Nothing here reads geometry or the source text, so a redesign that keeps the
// behaviour passes. Every wait is a web-first assertion on a rendered value, so
// a slow paint is waited out and a missing one fails with a message rather than
// with a timeout.
//
// The filter is asked on the inbox and on a project feed together because the
// two draw their rows differently: the inbox wraps a group's rows in a block
// inside the group, while the feed sets them beside the day heading. A walk that
// knows only one of the two hides a group that still holds rows, which is how
// typing into the inbox filter emptied the list while the count line above it
// still said what had matched.

import { expect, test as base } from "@playwright/test";
import { readHub } from "./hub.mjs";

// The list the filter judges: the index pane's body, and the field above it.
const BODY = ".shell-index .shell-body";
// The group headings a filter judges, which is the filter's own list rather than
// a second one here, so a screen that draws another shape is walked by both.
const HEADINGS = ".day, .hub-group-header, .inbox-group";
// The rows the keyboard map moves through, as the inbox registers them.
const KEY_ROWS = ".inbox-row:not(details:not([open]) .inbox-row)";
// The field Search opens on, named by its own label.
const SEARCH_FIELD = "Search the feed, artifacts and session brains";

const test = base.extend({
  // What this check runs against: its own project's seeded hub, and the token
  // the app reads before it paints. The token is written before any script on
  // the page runs, which is the only moment it can be for a page that paints
  // straight away.
  hub: async ({ page }, use, testInfo) => {
    const hub = readHub(testInfo.project.name);
    await page.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
    await use(hub);
  },
  // What the browser reported while the check ran, asserted when it ends, for
  // every check rather than for the ones that ask for it. A rendered document
  // is opaque-origin on purpose, so a frame of one that reads local storage
  // throws in the frame and not in the app. That is the design's own rule for
  // the viewer rather than what these checks are about, so it is the one error
  // left unreported.
  errors: [
    async ({ page }, use) => {
      const errors = [];
      page.on("pageerror", (error) => {
        const said = String(error);
        if (!said.includes("sandboxed")) errors.push(said);
      });
      await use(errors);
      expect(errors, "the browser reported uncaught errors").toEqual([]);
    },
    { auto: true },
  ],
});

// A screen names its own section in `data-segment`, which is the screen's
// identity rather than whatever it happened to open: a project screen's `h1` is
// the selected event or artifact, not the screen's name. So the wait is on the
// shell, and it is an assertion that waits for the paint.
async function open(page, hub, route, segment) {
  await page.goto(`${hub.baseUrl}/${route}`);
  const painted = segment
    ? page.locator(`.shell[data-segment="${segment}"] ${BODY}`)
    : page.locator(".shell-no-index .home-pad");
  await expect(painted).toBeVisible();
}

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
      labels: [...body.querySelectorAll(".inbox-label")].map((label) => ({
        key: label.dataset.group || text(label).slice(0, 24),
        visible: drawn(label),
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

// Typing a query leaves the rows it matched on screen under the groups that hold
// them, and leaves nothing standing that holds nothing.
async function filterKeepsItsRows(page, { name, terms, miss, where }) {
  const field = page.getByRole("searchbox", { name });
  await expect(field).toBeVisible();
  const before = await readIndex(page);
  const rows = page.locator(`${BODY} .row`);
  const groups = page.locator(BODY).locator(HEADINGS);
  const labels = page.locator(`${BODY} .inbox-label`);
  const count = page.locator(`${BODY} .shell-count`);

  for (const term of terms) {
    const want = owes(term, before);
    expect(want.matching.length, `${where}: nothing on the list matches ${term}`).toBeGreaterThan(0);
    await field.fill(term);
    // The count line is the only statement of what the query did, and the same
    // pass that writes it hides the rows, so waiting on it is waiting on the
    // filter having run.
    await expect(count).toBeVisible();
    await expect(count).toHaveText(`${want.matching.length} of ${before.rows.length} match "${term}"`);
    await expect(rows).toHaveCount(before.rows.length);
    for (const [at, row] of before.rows.entries()) {
      // A row under a folded disclosure was not the reader's to begin with, so
      // only the rows that were on screen are asked about.
      if (!row.visible) continue;
      if (want.matching.includes(row.text)) await expect(rows.nth(at)).toBeVisible();
      else await expect(rows.nth(at)).toBeHidden();
    }
    for (const [at, group] of before.groups.entries()) {
      if (!group.visible) continue;
      if (want.holding[group.key]) await expect(groups.nth(at)).toBeVisible();
      else await expect(groups.nth(at)).toBeHidden();
    }
    for (const [at, heading] of before.labels.entries()) {
      const holds = want.holding[heading.key];
      if (holds === undefined || !heading.visible) continue;
      if (holds) await expect(labels.nth(at)).toBeVisible();
      else await expect(labels.nth(at)).toBeHidden();
    }
    await field.fill("");
    await expect(count).toBeHidden();
    await expect(page.locator(`${BODY} .row[hidden]`)).toHaveCount(0);
  }

  // A query nothing matches leaves no row showing and no group standing over an
  // empty list.
  await field.fill(miss);
  await expect(count).toHaveText(`0 of ${before.rows.length} match "${miss}"`);
  await expect(page.locator(`${BODY} .row:visible`)).toHaveCount(0);
  for (const [at, group] of before.groups.entries()) {
    if (group.visible) await expect(groups.nth(at)).toBeHidden();
  }
  await field.fill("");
  await expect(page.locator(`${BODY} .row[hidden]`)).toHaveCount(0);
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
  await expect(page.locator('.inbox-row[tabindex="0"]')).toContainText(needle);
  return true;
}

test.describe("the list filter", () => {
  test("leaves the rows it matched under the group that holds them, on the inbox", async ({
    hub,
    page,
  }) => {
    const { approval, inboxRead, searchMiss } = hub.fixture;
    // One term in the waiting group and one in the unread group: a filter that
    // judges a group by what follows it gets one of the two wrong.
    await open(page, hub, "#/inbox", "inbox");
    await filterKeepsItsRows(page, {
      name: "Filter inbox",
      terms: [approval.split(" ")[0], inboxRead.split(" ")[1]],
      miss: searchMiss,
      where: "inbox",
    });
  });

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
    await filterKeepsItsRows(page, {
      name: "Filter events",
      terms: [finished.split(" ")[1]],
      miss: searchMiss,
      where: "project feed",
    });
  });
});

test.describe("the single-key verbs", () => {
  // An inbox row carries no Approve or Reply of its own: the tray a swipe
  // uncovers is a sibling of the row, so a key that looked only inside the row
  // found nothing and did nothing at all, on the one screen whose whole purpose
  // is deciding and answering.
  test("a and r reach the verbs the selected row keeps beside it", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the key map moves a desktop index");
    const { approval, question } = hub.fixture;
    await open(page, hub, "#/inbox", "inbox");
    const waiting = page.locator('.inbox-group[data-group="waiting"] .inbox-row');
    const before = await waiting.count();

    await expect(walkTo(page, approval), `no row carries ${approval}`).resolves.toBe(true);
    await page.keyboard.press("a");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("heading", { level: 2 })).toContainText(approval);
    await dialog.getByRole("button", { name: "Approve" }).click();
    await expect(waiting).toHaveCount(before - 1);
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await expect(walkTo(page, question), `no row carries ${question}`).resolves.toBe(true);
    await page.keyboard.press("r");
    const field = page.getByRole("textbox", { name: "Your answer" });
    await expect(field).toBeVisible();
    // The card answers the row it sits under and opens with the keyboard in it.
    await expect(page.locator(".inbox-item", { has: field })).toContainText(question);
    await expect(page.locator(".inbox-row .composer")).toHaveCount(0);
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
    await open(page, hub, "#/inbox", "inbox");
    // The case that works: a screen showing its filter takes the key, and what
    // is typed after it lands in that filter.
    const filter = page.getByRole("searchbox", { name: "Filter inbox" });
    await page.keyboard.press("/");
    await expect(filter).toBeFocused();
    await filter.pressSequentially("release");
    await expect(filter).toHaveValue("release");

    for (const segment of ["feed", "wiki", "artifacts", "sessions"]) {
      const where = `phone project ${segment}`;
      await open(page, hub, `#/projects/${hub.projectId}/${segment}`, segment);
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

test.describe("the home filter chips", () => {
  // The Unread chip keeps the newest rows carrying the unread dot, which is what
  // sits above a project's cursor. Its number was the hub's own unread queue, so
  // it read "Unread 3" over seven rows. It is asked on a phone because Home is a
  // phone screen, and before the other checks because the hub's cursor moves as
  // soon as a project feed has been read and the dots go with it.
  test("the unread chip's number counts the rows it reveals", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "this is the phone's Home screen");
    await open(page, hub, "#/home", "");
    // The chips are drawn twice on Home, in the tools row and in the pad, so
    // the flow copy is the one the row of cards belongs to.
    const chip = page
      .locator(".home-chips-flow")
      .getByRole("button", { name: /^Unread \d+$/ });
    await expect(chip).toBeVisible();
    const counted = Number((await chip.textContent()).replace(/\D/g, ""));
    expect(counted, "Home drew no unread rows, so the chip filters nothing to count").toBeGreaterThan(0);
    // The dot's own label is what marks a row unread, so the rows are found by
    // what they say rather than by the class that draws it.
    const unread = page.locator(".home-row").filter({ has: page.getByText("Unread", { exact: true }) });
    await expect(unread).toHaveCount(counted);
    await chip.click();
    await expect(chip).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".home-row:visible")).toHaveCount(counted);
  });
});
