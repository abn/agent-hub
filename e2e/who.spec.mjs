// The Who menu: one control in the control row of the feed, the artifacts and
// search. It is a menu a keyboard drives end to end, and what it picks goes to
// the hub as the hub's own filter, so the hub does the narrowing.

import { expect } from "@playwright/test";
import { api, goto, test } from "./invariants.mjs";

const toggle = (page, scope) => page.locator(`.who-menu[data-who^="${scope}"] [data-group-toggle]`);
const menu = (page, scope) => page.locator(`.who-menu[data-who^="${scope}"] [role="menu"]`);

// Which request carries which parameter: the hub is asked, not the page.
const asking = (page, path, param) =>
  page.waitForRequest((request) => request.url().includes(path) && new URL(request.url()).searchParams.has(param));

test.describe("the Who menu", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "the menu sits in a desktop's control row");
  });

  test("is reached, walked and left by keyboard on the feed", async ({ hub, page }) => {
    await goto(page, `#/projects/${hub.projectId}/feed`);
    const who = toggle(page, "feed:");
    await expect(who, "the feed has no Who menu").toBeVisible();
    await who.focus();
    await page.keyboard.press("ArrowDown");
    await expect(menu(page, "feed:")).toBeVisible();
    await expect(who).toHaveAttribute("aria-expanded", "true");
    const items = menu(page, "feed:").getByRole("menuitemradio");
    await expect(items.first(), "the menu opens on Anyone").toBeFocused();
    await expect(items.first()).toHaveAttribute("aria-checked", "true");

    await page.keyboard.press("ArrowDown");
    await expect(items.nth(1)).toBeFocused();
    await page.keyboard.press("ArrowUp");
    await expect(items.first()).toBeFocused();
    await page.keyboard.press("End");
    await expect(items.last()).toBeFocused();
    await page.keyboard.press("Home");
    await expect(items.first()).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(menu(page, "feed:")).toBeHidden();
    await expect(who, "Escape leaves focus on the trigger").toBeFocused();
    await expect(who).toHaveAttribute("aria-expanded", "false");
  });

  test("sends the choice to the hub as a filter and keeps the place after the repaint", async ({ hub, page }) => {
    await goto(page, `#/projects/${hub.projectId}/feed`);
    const who = toggle(page, "feed:");
    await who.focus();
    await page.keyboard.press("ArrowDown");
    const sent = asking(page, "/feed", "kind");
    await menu(page, "feed:").getByRole("menuitemradio", { name: "You" }).press("Enter");
    const request = await sent;
    expect(new URL(request.url()).searchParams.get("kind")).toBe("human");
    await expect(toggle(page, "feed:"), "the choice is named in the trigger").toHaveText("Who: You");
    await expect(toggle(page, "feed:"), "focus did not return to the trigger").toBeFocused();

    // Anyone puts the question back, and the trigger says only Who.
    await toggle(page, "feed:").focus();
    await page.keyboard.press("ArrowDown");
    await menu(page, "feed:").getByRole("menuitemradio", { name: "Anyone" }).press("Enter");
    await expect(toggle(page, "feed:")).toHaveAccessibleName("Who");
  });

  test("keeps a long choice inside the control row", async ({ hub, page }) => {
    const long = "build-farm-runner-number-twelve-in-the-rack";
    await page.route(/\/api\/v1\/members$/, (route) =>
      route.fulfill({
        json: {
          members: [
            { id: "farm-12", display_name: long, state: "active", created_at: new Date().toISOString() },
            { id: "farm-13", display_name: "Second runner", state: "active", created_at: new Date().toISOString() },
          ],
        },
      }),
    );
    await page.route(/\/api\/v1\/projects\/[^/]+\/feed\?.*member=/, (route) =>
      route.fulfill({ json: { events: [], last_seen: null, next_before: null } }),
    );
    await goto(page, `#/projects/${hub.projectId}/feed`);
    await toggle(page, "feed:").click();
    await menu(page, "feed:").getByRole("menuitemradio", { name: long }).click();
    const who = toggle(page, "feed:");
    await expect(who).toContainText("Who: build-farm");

    const held = await who.evaluate((el) => {
      const row = el.closest(".shell-controls");
      const a = el.getBoundingClientRect();
      const b = row.getBoundingClientRect();
      return {
        inside: a.left >= b.left - 0.5 && a.right <= b.right + 0.5,
        fits: row.scrollWidth <= row.clientWidth + 0.5,
        height: Math.round(a.height),
        rowHeight: Math.round(b.height),
        page: document.documentElement.scrollWidth <= window.innerWidth + 0.5,
      };
    });
    expect(held.inside, "the trigger overflows the control row").toBe(true);
    expect(held.fits, "the control row scrolls sideways").toBe(true);
    expect(held.page, "the page scrolls sideways").toBe(true);
    expect(held.height, "the trigger is taller than its band").toBeLessThanOrEqual(32);
    expect(held.rowHeight, "the control row grew").toBe(40);
  });

  test("filters by member on the feed", async ({ hub, page }) => {
    // Members are offered from two, so the check makes sure the hub has two.
    for (const [id, name] of [["who-probe-a", "Who probe A"], ["who-probe-b", "Who probe B"]]) {
      await api(hub, "POST", "/api/v1/members", { id, display_name: name });
    }
    const members = (await api(hub, "GET", "/api/v1/members")).json().members.filter((m) => m.state !== "pending");
    expect(members.length, "the hub has fewer than two members to choose between").toBeGreaterThan(1);
    const one = members[0];
    await goto(page, `#/projects/${hub.projectId}/feed`);
    await toggle(page, "feed:").click();
    const sent = asking(page, "/feed", "member");
    await menu(page, "feed:").getByRole("menuitemradio", { name: one.display_name || one.id }).click();
    expect(new URL((await sent).url()).searchParams.get("member")).toBe(one.id);
  });

  test("filters the artifact list and search by the same menu", async ({ hub, page }) => {
    await goto(page, "#/artifacts");
    await expect(toggle(page, "artifacts"), "the artifact list has no Who menu").toBeVisible();
    await toggle(page, "artifacts").click();
    const listed = asking(page, "/artifacts", "kind");
    await menu(page, "artifacts").getByRole("menuitemradio", { name: "You" }).click();
    expect(new URL((await listed).url()).searchParams.get("kind")).toBe("human");
    // The choice is the screen's own: search starts with none.
    await goto(page, "#/search?q=check");
    await expect(toggle(page, "search")).toBeVisible();
    await expect(toggle(page, "search")).toHaveAccessibleName("Who");
    await toggle(page, "search").click();
    const found = asking(page, "/api/v1/search", "kind");
    await menu(page, "search").getByRole("menuitemradio", { name: "You" }).click();
    expect(new URL((await found).url()).searchParams.get("kind")).toBe("human");
  });

  test("keeps a project's Artifacts choice out of every other project and the global list", async ({ hub, page }) => {
    const projects = (await api(hub, "GET", "/api/v1/projects")).json().projects.map((p) => p.id);
    const other = projects.find((id) => id !== hub.projectId);
    expect(other, "the seeded hub has only one project").toBeTruthy();

    // The kind a request to a project's artifacts carried, per project.
    const sent = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      const at = url.pathname.match(/^\/api\/v1\/projects\/([^/]+)\/artifacts$/);
      if (at) sent.push({ project: decodeURIComponent(at[1]), kind: url.searchParams.get("kind") });
    });

    await goto(page, `#/projects/${hub.projectId}/artifacts`);
    await toggle(page, `artifacts:${hub.projectId}`).click();
    const picked = asking(page, `/projects/${hub.projectId}/artifacts`, "kind");
    await menu(page, `artifacts:${hub.projectId}`).getByRole("menuitemradio", { name: "You" }).click();
    await picked;
    await expect(toggle(page, `artifacts:${hub.projectId}`)).toHaveText("Who: You");

    sent.length = 0;
    await goto(page, `#/projects/${other}/artifacts`);
    await expect(toggle(page, `artifacts:${other}`), "another project's Who menu is named for it").toHaveAccessibleName("Who");
    await expect.poll(() => sent.some((r) => r.project === other)).toBe(true);
    expect(sent.filter((r) => r.project === other).every((r) => r.kind === null), "the pick filtered another project").toBe(true);

    sent.length = 0;
    await goto(page, "#/artifacts");
    await expect(toggle(page, "artifacts"), "the global list is named Who, with no pick").toHaveAccessibleName("Who");
    await expect.poll(() => sent.length).toBeGreaterThan(0);
    expect(sent.every((r) => r.kind === null), "the pick filtered the global list").toBe(true);

    // And the pick is still where it was made.
    await goto(page, `#/projects/${hub.projectId}/artifacts`);
    await expect(toggle(page, `artifacts:${hub.projectId}`)).toHaveText("Who: You");
  });

  test("names the choice by its name, never by an id, and keeps it with the pick", async ({ hub, page }) => {
    const id = "01JZ0000000000000000ABCDEF";
    await page.route(/\/api\/v1\/sessions\?/, (route) =>
      route.fulfill({
        json: {
          sessions: [
            { id, project_id: hub.projectId, session_name: "release-train", status: "active", participants: [], started_label: "human" },
            { id: "01JZ0000000000000000ZZZZZZ", project_id: hub.projectId, session_name: "", status: "active", participants: [{ member: "laptop", agent: "opencode" }], started_label: "human" },
          ],
        },
      }),
    );
    await goto(page, `#/projects/${hub.projectId}/feed`);
    await toggle(page, "feed:").click();
    const items = menu(page, "feed:").getByRole("menuitemradio");
    await expect(items.filter({ hasText: "opencode …ZZZZZZ" }), "an unnamed session is read by its agent and a short id").toHaveCount(1);
    await expect(menu(page, "feed:")).not.toContainText(id);
    await page.route(/\/api\/v1\/projects\/[^/]+\/feed\?.*session=/, (route) =>
      route.fulfill({ json: { events: [], last_seen: null, next_before: null } }),
    );
    await items.filter({ hasText: "release-train" }).click();
    await expect(toggle(page, "feed:")).toHaveText("Who: release-train");
  });

  test("offers Members from two members and Before members only where such rows are", async ({ hub, page }) => {
    await page.route(/\/api\/v1\/members$/, (route) =>
      route.fulfill({ json: { members: [{ id: "laptop", display_name: "Laptop", state: "active", created_at: new Date().toISOString() }] } }),
    );
    await page.route(/\/api\/v1\/sessions\?/, (route) => route.fulfill({ json: { sessions: [] } }));
    await goto(page, `#/projects/${hub.projectId}/feed`);
    await toggle(page, "feed:").click();
    const list = menu(page, "feed:");
    await expect(list.getByRole("group", { name: "Members" }), "one member needs no list of members").toHaveCount(0);
    await expect(list.getByRole("menuitemradio", { name: "You" })).toBeVisible();
    await expect(list.getByRole("menuitemradio", { name: "Before members" }), "a hub with no such rows offers no such choice").toHaveCount(0);
  });

  test("is the person glyph and a chevron, 32px tall, and a sunken chip once it has a pick", async ({ hub, page }) => {
    await goto(page, `#/projects/${hub.projectId}/feed`);
    const who = toggle(page, "feed:");
    await expect(who).toHaveAccessibleName("Who");
    const rest = await who.evaluate((el) => ({
      height: Math.round(el.getBoundingClientRect().height),
      glyphs: el.querySelectorAll("svg").length,
      text: el.textContent.trim(),
      ground: getComputedStyle(el).backgroundColor,
      rule: getComputedStyle(el).boxShadow,
    }));
    expect(rest.height, "the trigger is not 32px tall").toBe(32);
    expect(rest.glyphs, "the trigger is not the person glyph and the chevron").toBe(2);
    expect(rest.text).toBe("");
    expect(rest.rule, "the trigger carries the selected rule with nothing chosen").toBe("none");

    await who.click();
    await menu(page, "feed:").getByRole("menuitemradio", { name: "You" }).click();
    await expect(toggle(page, "feed:")).toHaveText("Who: You");
    const chosen = await toggle(page, "feed:").evaluate((el) => ({
      height: Math.round(el.getBoundingClientRect().height),
      ground: getComputedStyle(el).backgroundColor,
      rule: getComputedStyle(el).boxShadow,
      glyphs: el.querySelectorAll("svg").length,
    }));
    expect(chosen.height).toBe(32);
    expect(chosen.glyphs, "a pick keeps the chevron").toBe(1);
    expect(chosen.ground, "a pick is not drawn on the sunken ground").not.toBe(rest.ground);
    expect(chosen.rule, "a pick has no 2px ink rule").toMatch(/(inset )?0px -2px 0px 0px( inset)?/);
  });
});

test.describe("the Who trigger on Search", () => {
  test("sits in the header an 8px from the field, with the 16px gutter kept", async ({ hub, page }) => {
    await goto(page, "#/search?q=check");
    const who = toggle(page, "search");
    await expect(who).toBeVisible();
    const placed = await page.evaluate(() => {
      const head = document.querySelector(".shell-index .shell-head");
      const form = head.querySelector(".search-form").getBoundingClientRect();
      const trigger = head.querySelector(".who-menu [data-group-toggle]").getBoundingClientRect();
      const box = head.getBoundingClientRect();
      return { gap: Math.round(trigger.left - form.right), gutter: Math.round(box.right - trigger.right) };
    });
    expect(placed.gap, "the trigger is not an --s-2 from the field").toBe(8);
    expect(placed.gutter, "the header's 16px gutter is not kept after the trigger").toBe(16);
  });
});

test.describe("a Who pick on a phone", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(testInfo.project.name !== "phone", "the filter glyph is a phone's");
  });

  test("marks the Filter and group glyph, and a kind filter does too", async ({ hub, page }) => {
    await goto(page, `#/projects/${hub.projectId}/feed`);
    const glyph = page.locator(".project-filter-btn").first();
    await expect(glyph).toHaveAccessibleName("Filter and group");
    await expect(glyph).toHaveAttribute("aria-pressed", "false");

    await glyph.click();
    await toggle(page, "feed:").click();
    await menu(page, "feed:").getByRole("menuitemradio", { name: "You" }).click();
    const marked = page.locator(".project-filter-btn").first();
    await expect(marked).toHaveAccessibleName("Filter and group, 1 active");
    await expect(marked).toHaveAttribute("aria-pressed", "true");
    await expect(marked.locator(".project-filter-dot"), "the glyph has no dot").toBeVisible();

    // Clearing the pick clears the mark.
    await marked.click();
    await toggle(page, "feed:").click();
    await menu(page, "feed:").getByRole("menuitemradio", { name: "Anyone" }).click();
    await expect(page.locator(".project-filter-btn").first()).toHaveAccessibleName("Filter and group");
    await expect(page.locator(".project-filter-btn").first()).toHaveAttribute("aria-pressed", "false");

    // A kind filter alone marks it as well.
    await page.locator(".project-filter-btn").first().click();
    await page.locator('.shell-controls:not(.project-tools-mobile) [data-group-toggle]').first().click();
    await page.locator('.shell-controls:not(.project-tools-mobile) button[data-action="kind"]:not([data-kind="all"])').first().click();
    await expect(page.locator(".project-filter-btn").first()).toHaveAccessibleName("Filter and group, 1 active");
  });
});

test.describe("the Who trigger under a coarse pointer", () => {
  test.use({ hasTouch: true, viewport: { width: 1440, height: 900 } });
  test.beforeEach(({}, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "one run of the coarse check is enough");
  });

  test("keeps its 44px hit area off the glyph beside it", async ({ hub, page }) => {
    await goto(page, `#/projects/${hub.projectId}/feed`);
    expect(await page.evaluate(() => matchMedia("(any-pointer: coarse)").matches), "the run is not under a coarse pointer").toBe(true);
    const who = toggle(page, "feed:");
    await expect(who).toBeVisible();
    // Every point inside the trigger's own box answers to the trigger, and every
    // point inside a neighbour's box answers to that neighbour: a hit area that
    // reached over the glyph beside it would take that glyph's taps.
    const stolen = await page.evaluate(() => {
      const trigger = document.querySelector(".who-menu [data-group-toggle]");
      const row = trigger.closest(".shell-controls");
      const owner = (x, y) => document.elementFromPoint(x, y)?.closest("button, input, a");
      const problems = [];
      for (const control of row.querySelectorAll("button, input")) {
        if (control === trigger || !control.getClientRects().length) continue;
        const b = control.getBoundingClientRect();
        for (const [x, y] of [[b.left + 1, b.top + b.height / 2], [b.right - 1, b.top + b.height / 2]]) {
          const hit = owner(x, y);
          if (hit === trigger) problems.push(`${control.className || control.tagName} loses a tap to Who`);
        }
      }
      const t = trigger.getBoundingClientRect();
      for (const [x, y] of [[t.left + 1, t.top + t.height / 2], [t.right - 1, t.top + t.height / 2], [t.left + t.width / 2, t.top + 1], [t.left + t.width / 2, t.bottom - 1]]) {
        if (owner(x, y) !== trigger) problems.push("Who loses a tap inside its own box");
      }
      return problems;
    });
    expect(stolen).toEqual([]);
    const box = await who.boundingBox();
    expect(box.width, "the trigger is narrower than a 44px target").toBeGreaterThanOrEqual(44);
  });
});
