// Feed dismissal: the menu's two verbs, the selection they build, the tray a
// left swipe uncovers on a phone, and the line between the human's view and an
// agent's read. Each width runs on its own hub, so the dismissal one check makes
// is not the context the next one reads.

import AxeBuilder from "@axe-core/playwright";
import { api, expect, goto, newSession, oneOffEvent, structured, test, toolCall } from "./invariants.mjs";

const feedRoute = (hub) => `#/projects/${encodeURIComponent(hub.projectId)}/feed`;
const row = (id) => `main .feed-item[data-id="${id}"]`;

// The feed's verbs ride in the project's one overflow at both widths, so the
// control row keeps its filter's full width and a phone adds no second menu.
async function openFeedMenu(page) {
  await page.getByRole("button", { name: "Project actions" }).click();
}

// A left swipe on a row, as the phone sends it: a primary touch pointer down on
// the row, a move past the tray width, and the release.
async function swipeLeft(page, item) {
  await item.scrollIntoViewIfNeeded();
  const box = await item.boundingBox();
  expect(box, "the row has no box to swipe").toBeTruthy();
  const x = box.x + box.width / 2;
  const y = box.y + Math.min(box.height / 2, 24);
  await page.evaluate(
    ({ x, y }) => {
      const target = document.elementFromPoint(x, y);
      const send = (type, clientX, on) =>
        on.dispatchEvent(
          new PointerEvent(type, {
            bubbles: true,
            cancelable: true,
            pointerId: 7,
            pointerType: "touch",
            isPrimary: true,
            clientX,
            clientY: y,
          }),
        );
      send("pointerdown", x, target);
      send("pointermove", x - 120, window);
      send("pointerup", x - 120, window);
    },
    { x, y },
  );
}

async function waitSeen(hub) {
  await expect
    .poll(
      async () => {
        const response = await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/feed?limit=1`);
        return response.json().last_seen || "";
      },
      { message: "opening the feed never moved the hub's read cursor", timeout: 8000 },
    )
    .not.toBe("");
}

test.describe("feed dismissal", () => {
  test("picks events and dismisses them together, and an agent still reads them", async ({ hub, page }, testInfo) => {
    const kept = await oneOffEvent(hub, hub.projectId, "signal", "Dismiss check: kept");
    const gone = await oneOffEvent(hub, hub.projectId, "signal", "Dismiss check: gone");
    const alsoGone = await oneOffEvent(hub, hub.projectId, "signal", "Dismiss check: also gone");
    expect(kept && gone && alsoGone, "the check could not post its events").toBeTruthy();

    await goto(page, feedRoute(hub), "Feed");
    await expect(page.locator(row(gone))).toBeVisible({ timeout: 8000 });

    await openFeedMenu(page);
    await page.getByRole("menuitem", { name: "Select items" }).click();

    // The filter's row becomes the count, so the list itself does not move.
    const bar = page.locator(".feed-select-bar:visible");
    await expect(bar).toBeVisible();
    if (testInfo.project.name === "dismiss-phone") {
      await expect(page.locator(".project-tools-mobile .project-tools-seg")).toBeHidden();
    } else {
      await expect(page.locator(".shell-index > .shell-controls .shell-filter")).toBeHidden();
    }

    for (const id of [gone, alsoGone]) {
      await page.evaluate(
        (eventId) =>
          document
            .querySelector(`.feed-item[data-id="${eventId}"]`)
            ?.scrollIntoView({ block: "center" }),
        id,
      );
      const box = page.locator(`${row(id)} .feed-pick-wrap`);
      await box.click();
      await expect(page.locator(`${row(id)} .feed-pick`)).toBeChecked();
    }
    await expect(bar.locator(".feed-select-count")).toHaveText("2 selected");

    await bar.getByRole("button", { name: "Dismiss", exact: true }).click();
    const dialog = page.locator("dialog.dialog[open]");
    await expect(dialog, "bulk dismissal asked nothing").toBeVisible();
    await dialog.locator(".dialog-commit").click();

    await expect(page.locator(row(gone)), "the dismissed row is still in the human feed").toHaveCount(0);
    await expect(page.locator(row(alsoGone))).toHaveCount(0);
    await expect(page.locator(row(kept)), "the event nobody picked was taken too").toHaveCount(1);
    await expect(page.locator(".feed-select-bar:visible")).toHaveCount(0);

    // A dismissal is the human's view, not a deletion: an agent's read still
    // carries the events.
    const session = await newSession(hub);
    const read = structured(
      await toolCall(hub, session, "feed_read", {
        project_id: hub.projectId,
        since: "0",
        limit: 500,
      }),
    );
    const ids = (read.events || []).map((event) => event.id);
    expect(ids, "an agent no longer reads a dismissed event").toContain(gone);
    expect(ids).toContain(alsoGone);

    // A link that still names the dismissed event does not open some other row
    // under the same address.
    await page.goto(`${hub.baseUrl}/${feedRoute(hub)}?event=${encodeURIComponent(gone)}`);
    await expect(page.locator(".shell-stage")).toContainText("This event is not in the feed", {
      timeout: 8000,
    });
  });

  test("dismiss read takes the read page and leaves what is still unread", async ({ hub, page }) => {
    await goto(page, feedRoute(hub), "Feed");
    const first = page.locator("main .feed-item").first();
    await expect(first, "the feed drew no row to dismiss").toBeVisible({ timeout: 8000 });
    const readId = await first.getAttribute("data-id");
    expect(readId, "the first row carries no id").toBeTruthy();
    await waitSeen(hub);

    // An event written after the cursor was set is unread and must survive the
    // bulk dismissal.
    const unread = await oneOffEvent(hub, hub.projectId, "signal", "Dismiss read: still unread");
    expect(unread, "the check could not post the unread event").toBeTruthy();

    await openFeedMenu(page);
    await page.getByRole("menuitem", { name: "Dismiss read" }).click();
    const dialog = page.locator("dialog.dialog[open]");
    await expect(dialog, "dismissing the read page asked nothing").toBeVisible();
    await dialog.locator(".dialog-commit").click();

    await expect(page.locator(row(readId)), "a read event survived Dismiss read").toHaveCount(0);
    await expect(page.locator(row(unread)), "Dismiss read took an unread event").toBeVisible({ timeout: 8000 });
  });

  test("a left swipe on a phone uncovers Dismiss and the tap removes the row", async ({ hub, page }, testInfo) => {
    test.skip(testInfo.project.name !== "dismiss-phone", "the gesture is a phone check");
    const gone = await oneOffEvent(hub, hub.projectId, "signal", "Dismiss check: swiped");
    expect(gone, "the check could not post its event").toBeTruthy();
    await goto(page, feedRoute(hub), "Feed");
    const item = page.locator(row(gone));
    await expect(item).toBeVisible({ timeout: 8000 });
    await swipeLeft(page, item);

    const tray = item.locator(".swipe-tray-dismiss");
    await expect(tray, "the swipe uncovered no Dismiss action").toBeVisible();
    await tray.getByRole("button", { name: "Dismiss" }).click();
    await expect(page.locator(row(gone)), "the swiped row was not dismissed").toHaveCount(0);
  });
  test("the picking row and the revealed tray pass the accessibility audit", async ({ hub, page }, testInfo) => {
    const marker = await oneOffEvent(hub, hub.projectId, "signal", "Dismiss audit row");
    expect(marker, "the check could not post its row").toBeTruthy();
    await goto(page, feedRoute(hub), "Feed");
    const item = page.locator(row(marker));
    await expect(item).toBeVisible({ timeout: 8000 });

    if (testInfo.project.name === "dismiss-phone") {
      // Entering picking from the open filter panel must not leave its row's
      // bar beside the tools row's own. The panel is in front of the menu for a
      // pointer, so the sequence goes through the keyboard.
      const panel = page.locator(".shell-index .shell-controls:not(.project-tools-mobile)");
      await page.locator('[data-action="project-filter-toggle"]').click();
      await expect(panel).toBeVisible();
      await page.getByRole("button", { name: "Project actions" }).focus();
      await page.keyboard.press("Enter");
      await page.getByRole("menuitem", { name: "Select items" }).press("Enter");
      await expect(panel, "the filter panel stayed open beside the selection").toBeHidden();
    } else {
      await openFeedMenu(page);
      await page.getByRole("menuitem", { name: "Select items" }).click();
    }
    await expect(page.locator(".feed-select-bar:visible")).toHaveCount(1);
    await page.locator(`${row(marker)} .feed-pick-wrap`).click();

    const audit = async (state) => {
      // Audit the resting state: a pointer left over a control would hold its
      // hover colours, and the design gate is the screen a reader gets.
      await page.mouse.move(0, 0);
      const results = await new AxeBuilder({ page })
        .include("main .shell-index")
        .withTags(["wcag2a", "wcag2aa"])
        .analyze();
      expect(
        results.violations.map(
          (violation) => `${violation.id}: ${violation.help} [${violation.nodes.map((node) => `${node.target.join(" ")} ${node.failureSummary || ""} ${JSON.stringify(node.any?.[0]?.data || {})}`).join("; ")}]`,
        ),
        `${state} has accessibility violations`,
      ).toEqual([]);
    };
    await audit("the picker");

    if (testInfo.project.name === "dismiss-phone") {
      // The bar's controls draw at the coarse 44px target, and the box's 24px
      // column answers to 44 through its own hit area.
      for (const name of ["Dismiss", "Cancel"]) {
        const control = page.locator(".feed-select-bar:visible").getByRole("button", { name, exact: true });
        const box = await control.boundingBox();
        expect(box, `the ${name} control has no box`).toBeTruthy();
        expect(
          box.width >= 44 && box.height >= 44,
          `the ${name} control is ${box.width}x${box.height}, not 44x44`,
        ).toBe(true);
      }
      const hit = await page.evaluate(() => {
        const label = document.querySelector(".feed-pick-wrap");
        const box = label.getBoundingClientRect();
        const x = box.left + box.width / 2;
        const y = box.top + box.height / 2;
        return [
          [0, 0],
          [-21, -21],
          [21, 21],
        ].every(([dx, dy]) => document.elementFromPoint(x + dx, y + dy)?.closest(".feed-pick-wrap") === label);
      });
      expect(hit, "the pick box does not answer to a 44 by 44 target").toBe(true);
    }

    // Cancelling puts the reader back on the control that opened the picker,
    // at both widths.
    await page.locator(".feed-select-bar:visible").getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("button", { name: "Project actions" })).toBeFocused();

    if (testInfo.project.name === "dismiss-phone") {
      await swipeLeft(page, item);
      await expect(item.locator(".swipe-tray-dismiss")).toBeVisible();
      await audit("the revealed tray");
    }
  });

});
