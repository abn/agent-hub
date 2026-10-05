// What every browser check shares: its own project's seeded hub and the way a
// screen is opened.
//
// One hub per project. A screen that has been read has moved the hub's own read
// cursor, and Home draws its unread dots from what is still above that cursor,
// so a project running after another would start from data the first one
// changed. Separate hubs keep the two runs independent, which is what lets each
// project assert against the same fixture the harness seeds.
//
// The token is written before any script on the page runs, which is the only
// moment it can be for a page that paints straight away. The errors listener is
// asserted at the end of every check rather than only the ones that ask for it.
// A rendered document is opaque-origin on purpose, so a frame of one that reads
// local storage throws in the frame and not in the app. That is the design's own
// rule for the viewer rather than what these checks are about, so it is the one
// error left unreported.

import { expect, test as base } from "@playwright/test";
import { readHub } from "./hub.mjs";

// The list a screen's index draws its rows into.
export const BODY = ".shell-index .shell-body";

export const test = base.extend({
  // What this check runs against: its own project's seeded hub, and the token
  // the app reads before it paints.
  hub: async ({ page }, use, testInfo) => {
    const hub = readHub(testInfo.project.name);
    await page.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
    await use(hub);
  },
  // What the browser reported while the check ran, asserted when it ends.
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
export async function open(page, hub, route, segment) {
  await page.goto(`${hub.baseUrl}/${route}`);
  const painted = segment
    ? page.locator(`.shell[data-segment="${segment}"] ${BODY}`)
    : page.locator(".shell-no-index .home-pad");
  await expect(painted).toBeVisible();
}
