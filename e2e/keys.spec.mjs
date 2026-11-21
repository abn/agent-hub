// The single-key shortcuts and where they are named. A key is drawn on a
// control only where pressing it now would act: on Now the verbs show on the
// selected ask, with their keys, and on a hovered or focused ask without
// them, because the map presses the selected row and no other; a screen's own
// hints appear only when the screen has something for the keys to act on. Each check presses the key as well as looking for the
// chip, so a hint that lies fails here.

import { expect } from "@playwright/test";
import { BODY, open, test } from "./app.mjs";
import { api, oneOffEvent, oneOffQuestion, quietHomePayload } from "./invariants.mjs";

test.beforeEach(({ page: _ }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "the keys and their hints are a desktop's");
});

const asks = (page) => page.locator(".now-ask");
const ask = (page, subject) => asks(page).filter({ hasText: subject });
const selected = (page) => page.locator('.now-ask[tabindex="0"]');
// The key chips an ask draws on its verbs, and so the ones a reader could read
// as a promise.
const chips = (row) => row.locator(".now-acts kbd");

async function select(page, row) {
  await row.click({ position: { x: 2, y: 2 } });
  await expect(row).toHaveAttribute("tabindex", "0");
}

const questionOf = (hub, subject, options = ["Ship it", "Hold"]) =>
  oneOffQuestion(hub, hub.projectId, subject, { options });

async function settleAll(hub, questions, approvals = []) {
  for (const id of questions) await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "done" }).catch(() => {});
  for (const id of approvals) await api(hub, "POST", `/api/v1/approvals/${id}/decision`, { decision: "decline" }).catch(() => {});
}

test.describe("Now shows an ask's verbs on the selected row", () => {
  test("the selected ask shows its verbs and keys, the others show neither, and j moves them", async ({ hub, page }) => {
    const first = await questionOf(hub, "keys hint one");
    const second = await questionOf(hub, "keys hint two");
    try {
      await open(page, hub, "#/inbox", "");
      const one = ask(page, "keys hint one");
      const two = ask(page, "keys hint two");
      await select(page, one);
      await page.mouse.move(4, 600);
      await expect(one.getByRole("button", { name: "Ship it" })).toBeVisible();
      await expect(one.getByRole("link", { name: "Read" })).toBeVisible();
      await expect(chips(one).filter({ visible: true })).not.toHaveCount(0);
      await expect(one.getByRole("link", { name: "Read" }).locator("kbd")).toBeVisible();
      await expect(two.getByRole("button", { name: "Ship it" })).toBeHidden();
      await expect(two.getByRole("link", { name: "Read" })).toBeHidden();
      await expect(chips(two).filter({ visible: true })).toHaveCount(0);
      // Exactly one ask names its keys, and it is the selected one.
      await expect(asks(page).filter({ has: page.locator(".now-acts kbd:visible") })).toHaveCount(1);

      await page.keyboard.press("j");
      await expect(selected(page)).toBeFocused();
      await expect(selected(page).locator(".now-acts button:visible").first()).toBeVisible();
      await expect(asks(page).filter({ has: page.locator(".now-acts kbd:visible") })).toHaveCount(1);
      await expect(one.getByRole("button", { name: "Ship it" })).toBeHidden();
      await expect(chips(one).filter({ visible: true })).toHaveCount(0);
    } finally {
      await settleAll(hub, [first, second]);
    }
  });

  test("hovering an unselected ask shows its verbs as outlines with no keys", async ({ hub, page }) => {
    const first = await questionOf(hub, "keys hover one");
    const second = await questionOf(hub, "keys hover two");
    try {
      await open(page, hub, "#/inbox", "");
      const one = ask(page, "keys hover one");
      const two = ask(page, "keys hover two");
      await select(page, one);
      await page.mouse.move(4, 600);
      await expect(two.getByRole("button", { name: "Ship it" })).toBeHidden();
      await two.hover({ position: { x: 40, y: 4 } });
      await expect(two.getByRole("button", { name: "Ship it" })).toBeVisible();
      await expect(two.getByRole("link", { name: "Read" })).toBeVisible();
      await expect(chips(two).filter({ visible: true })).toHaveCount(0);
      await expect(two).not.toHaveAttribute("tabindex", "0");
      const fill = await two.getByRole("button", { name: "Ship it" }).evaluate((el) => getComputedStyle(el).backgroundColor);
      const filled = await one.getByRole("button", { name: "Ship it" }).evaluate((el) => getComputedStyle(el).backgroundColor);
      expect(fill, "a hovered row filled its primary").not.toBe(filled);
    } finally {
      await settleAll(hub, [first, second]);
    }
  });

  test("tabbing into a row's buttons keeps its verbs shown", async ({ hub, page }) => {
    const first = await questionOf(hub, "keys tab one");
    try {
      await open(page, hub, "#/inbox", "");
      const one = ask(page, "keys tab one");
      await select(page, one);
      await page.mouse.move(4, 600);
      await one.evaluate((el) => el.focus());
      await page.keyboard.press("Tab");
      const button = one.getByRole("button", { name: "Ship it" });
      await expect(button).toBeFocused();
      await expect(button).toBeVisible();
      await expect(one.getByRole("link", { name: "Read" })).toBeVisible();
    } finally {
      await settleAll(hub, [first]);
    }
  });

  test("moving the selection never changes a row's height, and no row carries a line of keys", async ({ hub, page }) => {
    const first = await questionOf(hub, "keys steady one");
    const second = await questionOf(hub, "keys steady two");
    try {
      await open(page, hub, "#/inbox", "");
      const one = ask(page, "keys steady one");
      const two = ask(page, "keys steady two");
      await select(page, one);
      await page.mouse.move(4, 600);
      const heights = async () => [(await one.boundingBox()).height, (await two.boundingBox()).height];
      const before = await heights();
      await select(page, two);
      await page.mouse.move(4, 600);
      await expect(chips(one).filter({ visible: true })).toHaveCount(0);
      expect(await heights(), "moving the selection changed a row's height").toEqual(before);
      await expect(page.locator(".now-keys")).toHaveCount(0);
    } finally {
      await settleAll(hub, [first, second]);
    }
  });

  test("1 answers the selected ask and leaves the others waiting", async ({ hub, page }) => {
    const first = await questionOf(hub, "keys press one");
    const second = await questionOf(hub, "keys press two");
    try {
      await open(page, hub, "#/inbox", "");
      await select(page, ask(page, "keys press two"));
      const sent = page.waitForRequest((request) => request.method() === "POST" && request.url().endsWith(`/api/v1/questions/${second}/answer`));
      await page.keyboard.press("1");
      expect((await sent).postDataJSON()).toEqual({ body: "Ship it" });
      await expect(ask(page, "keys press two")).toHaveCount(0);
      await expect(ask(page, "keys press one")).toBeVisible();
      const left = (await api(hub, "GET", `/api/v1/inbox?status=action&limit=500`)).json();
      expect(left.items.some((item) => item.event_id === first), "the other ask was answered too").toBe(true);
    } finally {
      await settleAll(hub, [first, second]);
    }
  });

  test("an approval's keys act, and it names no reply", async ({ hub, page }) => {
    const approval = await oneOffEvent(hub, hub.projectId, "approval", "keys approval row");
    const question = await questionOf(hub, "keys question row");
    try {
      await open(page, hub, "#/inbox", "");
      const row = ask(page, "keys approval row");
      await select(page, row);
      await expect(row.getByRole("button", { name: "Approve" })).toBeVisible();
      await expect(row.getByRole("button", { name: "Decline" })).toBeVisible();
      // An approval has no reply control, so no key for one is offered, and r
      // does nothing on it.
      await expect(row.getByRole("button", { name: "Reply" })).toHaveCount(0);
      await page.keyboard.press("r");
      await expect(page.getByRole("textbox", { name: "Your answer" })).toHaveCount(0);
      await expect(page.getByRole("dialog")).toHaveCount(0);

      await page.keyboard.press("2");
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByRole("heading", { level: 2 })).toContainText("keys approval row");
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);

      await select(page, ask(page, "keys question row"));
      await expect(ask(page, "keys question row").getByRole("button", { name: "Reply" })).toBeVisible();
    } finally {
      await settleAll(hub, [question], [approval]);
    }
  });

  test("the control row names j k and o only while the stream has rows", async ({ hub, page }) => {
    await open(page, hub, "#/home", "");
    const hints = page.locator(".now-hints");
    await expect(hints.getByText("move")).toBeVisible();
    await expect(hints.getByText("open")).toBeVisible();
    await expect(hints.getByText("keys")).toBeVisible();

    const payload = quietHomePayload();
    const home = /\/api\/v1\/home$/;
    await page.route(home, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(payload) }),
    );
    try {
      await page.goto(`${hub.baseUrl}/#/settings`);
      await page.goto(`${hub.baseUrl}/#/home`);
      await expect(page.locator(".now .now-sum")).toBeVisible();
      await expect(page.locator(".now-row")).toHaveCount(0);
      await expect(hints.getByText("move")).toBeHidden();
      await expect(hints.getByText("open")).toBeHidden();
      // The shortcut list opens from anywhere, so its hint stays.
      await expect(hints.getByText("keys")).toBeVisible();
    } finally {
      await page.unroute(home).catch(() => {});
    }
  });
});

test.describe("the artifacts list names its keys", () => {
  const OLD = JSON.stringify({ at: 1, seen: {} });

  test("o and the move keys are named while there are rows, and E clears the first tier", async ({ hub, page }) => {
    await page.addInitScript((seen) => window.localStorage.setItem("hub.artifacts.seen", seen), OLD);
    await page.goto(`${hub.baseUrl}/#/artifacts`);
    await expect(page.locator(".arts-row").first()).toBeVisible();
    const hints = page.locator(".arts-hints");
    await expect(hints.getByText("move")).toBeVisible();
    await expect(hints.getByText("open")).toBeVisible();

    const since = page.getByRole("heading", { name: /^Since you looked/ });
    await expect(since).toBeVisible();
    await expect(page.getByRole("button", { name: /^Mark seen/ }).locator("kbd")).toBeVisible();
    await page.keyboard.press("E");
    await expect(since).toHaveCount(0);
    // With nothing since, the button and its key are gone together.
    await expect(page.getByRole("button", { name: /^Mark seen/ })).toHaveCount(0);

    await page.keyboard.press("j");
    await expect(page.locator('.arts-row[tabindex="0"]')).toBeFocused();
    const before = page.url();
    await page.keyboard.press("o");
    await expect(page).not.toHaveURL(before);
  });

  test("an empty list names no key that has nothing to act on", async ({ hub, page }) => {
    await page.route(/\/api\/v1\/projects$/, (route) =>
      route.fulfill({ json: { projects: [{ id: "checks", display_name: "checks", confidential: false }] } }),
    );
    await page.route(/\/api\/v1\/projects\/checks\/artifacts$/, (route) => route.fulfill({ json: { artifacts: [] } }));
    await page.goto(`${hub.baseUrl}/#/artifacts`);
    await expect(page.locator(".arts-list .empty-state")).toBeVisible();
    await expect(page.locator(".arts-row")).toHaveCount(0);
    await expect(page.locator(".arts-hints").getByText("move")).toBeHidden();
    await expect(page.locator(".arts-hints").getByText("open")).toBeHidden();
  });
});

test.describe("the artifact viewer names c where c works", () => {
  test("c toggles the comments aside the hint names", async ({ hub, page }) => {
    await open(page, hub, `#/projects/${hub.projectId}/artifacts?artifact=${hub.artifactId}`, "artifacts");
    await expect(page.locator(".artifact-hint")).toBeVisible();
    await expect(page.locator(".artifact-hint")).toContainText("comments");
    const aside = page.locator(".shell-aside");
    const shown = await aside.isVisible();
    await page.keyboard.press("c");
    await expect(aside).toBeVisible({ visible: !shown });
    await page.keyboard.press("c");
    await expect(aside).toBeVisible({ visible: shown });
  });
});

test.describe("an opened item names its keys", () => {
  const itemUrl = (hub, id) => `${hub.baseUrl}/#/inbox?open=${encodeURIComponent(id)}`;

  test("j and k are named only where there is an item to step to, and do nothing otherwise", async ({ hub, page }) => {
    await open(page, hub, "#/inbox", "");
    await asks(page).first().getByRole("link", { name: "Read" }).click();
    const hints = page.locator(".item-hints");
    await expect(page.getByRole("button", { name: "Previous item" })).toBeDisabled();
    await expect(hints.getByText("previous")).toBeHidden();
    await expect(hints.getByText("next")).toBeVisible();
    const start = page.url();
    await page.keyboard.press("k");
    await expect(page).toHaveURL(start);

    const next = page.getByRole("link", { name: "Next item" });
    const heading = page.locator("#inbox-detail-title");
    for (let guard = 0; guard < 40 && (await next.count()); guard += 1) {
      // The next item is on screen once its own title is, not once the address
      // changes, which is a repaint earlier.
      const here = await heading.textContent();
      await page.keyboard.press("j");
      await expect(heading).not.toHaveText(here);
    }
    await expect(page.getByRole("button", { name: "Next item" })).toBeDisabled();
    await expect(hints.getByText("next")).toBeHidden();
    await expect(hints.getByText("previous")).toBeVisible();
    const end = page.url();
    await page.keyboard.press("j");
    await expect(page).toHaveURL(end);
  });

  test("a and d decide an approval, s snoozes it and Esc goes back to Now", async ({ hub, page }) => {
    const id = await oneOffEvent(hub, hub.projectId, "approval", "keys item approval");
    expect(id, "the approval was not posted").toBeTruthy();
    try {
      await page.goto(itemUrl(hub, id));
      const answer = page.getByRole("region", { name: "Your answer" });
      await expect(answer.getByRole("button", { name: "Approve" }).locator("kbd")).toBeVisible();
      await expect(answer.getByRole("button", { name: "Decline" }).locator("kbd")).toBeVisible();
      const dialog = page.getByRole("dialog");
      await page.keyboard.press("d");
      await expect(dialog.getByRole("heading", { level: 2 })).toContainText("keys item approval");
      await expect(dialog.getByRole("button", { name: "Decline" })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);
      // The dialog hands focus back to the item's title once it has gone, and
      // the map answers again from there.
      await expect(page.getByRole("heading", { level: 1 })).toBeFocused();
      await page.keyboard.press("a");
      await expect(dialog.getByRole("button", { name: "Approve" })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);

      await page.keyboard.press("s");
      await expect(page).not.toHaveURL(new RegExp(`open=${id}$`));
      await page.goto(itemUrl(hub, id));
      await expect(answer.getByRole("button", { name: "Bring back" })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("heading", { level: 1, name: "Now" })).toBeVisible();
    } finally {
      await page.evaluate(() => localStorage.removeItem("hub.snooze")).catch(() => {});
      await settleAll(hub, [], [id]);
    }
  });

  test("e resolves an idea", async ({ hub, page }) => {
    const id = await oneOffEvent(hub, hub.projectId, "idea", "keys item idea");
    expect(id, "the idea was not posted").toBeTruthy();
    try {
      await page.goto(itemUrl(hub, id));
      const answer = page.getByRole("region", { name: "Your answer" });
      await expect(answer.getByRole("button", { name: "Resolve" }).locator("kbd")).toBeVisible();
      const sent = page.waitForRequest((request) => request.method() === "POST" && request.url().endsWith(`/api/v1/ideas/${id}/resolve`));
      await page.keyboard.press("e");
      await sent;
    } finally {
      await api(hub, "POST", `/api/v1/ideas/${id}/resolve`, {}).catch(() => {});
    }
  });
});

test.describe("the lists that move and open", () => {
  for (const [name, route, segment] of [
    ["the project feed", (hub) => `#/projects/${hub.projectId}/feed`, "feed"],
    ["the sessions list", (hub) => `#/projects/${hub.projectId}/sessions`, "sessions"],
    ["search", (hub) => `#/search?q=${encodeURIComponent(hub.fixture.searchTerm)}`, "search"],
  ]) {
    test(`j moves the selection and o opens it on ${name}`, async ({ hub, page }) => {
      await open(page, hub, route(hub), segment);
      const rows = page.locator(`${BODY} .row, .search-row`);
      await expect(rows.first()).toBeVisible();
      await page.keyboard.press("j");
      await expect(page.locator('[tabindex="0"].row, [tabindex="0"].search-row')).toBeFocused();
      const before = page.url();
      await page.keyboard.press("o");
      await expect(page).not.toHaveURL(before);
    });
  }
});
