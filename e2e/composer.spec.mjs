// Every composer is the one composer, held at both pointers.
//
// DESIGN.md, Composer: a field drawn at the pointer's form size that grows a
// line at a time to eight lines and then scrolls; a send control beside it
// that is --ctl on a fine pointer and --tap on a coarse one, centred on the
// field's last line however many lines it holds; an icon with a name; dimmed
// with its reason while the field is empty; Enter for a line break and the
// command key with Enter to send, named under the field; long words wrapped
// rather than scrolled sideways. Each place a reader writes is opened the way a
// reader opens it and held to all of that, so a screen that builds a composer
// of its own fails here.
//
// Nothing is really sent: each send is answered with a refusal at the network,
// which is also how the check proves the words survive one. The sans face is
// pinned, so the sizes and the photographs do not depend on the host's fonts.

import { expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { test } from "./app.mjs";
import { SCREENSHOT, fontsSettled, pinFont, pinnedFontOptions } from "./pinned-font.mjs";
import { seedWikiPage } from "./screens.mjs";

test.use(pinnedFontOptions);

const LINE = 20;
const MAX_LINES = 8;
const SIZES = {
  "composer-desktop": { field: 36, send: 32, coarse: false },
  "composer-touch": { field: 44, send: 44, coarse: true },
};

const lines = (n) => Array.from({ length: n }, (_, i) => `line ${i + 1}`).join("\n");

// The form a field belongs to: the composer is the field's own form.
const formOf = (field) => field.locator("xpath=ancestor::form[1]");

async function seedComment(hub) {
  const response = await fetch(`${hub.baseUrl}/api/v1/artifacts/${hub.artifactId}/comments`, {
    method: "POST",
    headers: { Authorization: `Bearer ${hub.token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ body: "A thread for the composer check." }),
  });
  expect(response.ok, `seeding a comment answered ${response.status}`).toBe(true);
}

const viewer = (hub) => `${hub.baseUrl}/#/artifacts/${hub.artifactId}?project=${hub.projectId}`;

// Where a reader writes, how they get there, and what the composer is called.
const COMPOSERS = [
  {
    name: "question card",
    label: "Your answer",
    action: "Send",
    empty: "Write a reply to send it.",
    open: async (page, hub) => {
      await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(hub.questionId)}`);
    },
  },
  {
    name: "inbox row",
    label: "Your answer",
    action: "Send",
    empty: "Write a reply to send it.",
    open: async (page, hub) => {
      await page.goto(`${hub.baseUrl}/#/inbox`);
      // A question row's Reply lives in the tray a swipe uncovers; the press is
      // what a reader's swipe and tap end in.
      const reply = page
        .locator(".shell-index")
        .getByRole("button", { name: "Reply", exact: true, includeHidden: true })
        .first();
      await reply.dispatchEvent("click");
    },
  },
  {
    name: "feed stage",
    label: "Your answer",
    action: "Send",
    empty: "Write a reply to send it.",
    open: async (page, hub) => {
      await page.goto(
        `${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/feed?event=${encodeURIComponent(hub.questionId)}`,
      );
      await page.locator(".feed-stage-actions").getByRole("button", { name: "Reply", exact: true }).click();
    },
  },
  {
    name: "home row",
    label: "Your answer",
    action: "Send",
    empty: "Write a reply to send it.",
    // A phone's Home lists what waits without the row's own Reply.
    pointers: ["composer-desktop"],
    open: async (page, hub) => {
      await page.goto(`${hub.baseUrl}/#/home`);
      await page.locator(".home-actions").getByRole("button", { name: "Reply", exact: true }).first().click();
    },
  },
  {
    name: "new comment",
    label: "New comment",
    action: "Post",
    empty: "Write a comment to post it.",
    open: async (page, hub) => {
      await page.goto(viewer(hub));
      await page.getByRole("button", { name: "Start a thread" }).click();
    },
  },
  {
    name: "comment list",
    label: "New comment",
    action: "Post",
    empty: "Write a comment to post it.",
    // A fine pointer reads an artifact's threads in the aside, which carries no
    // list composer; the drawer's list is the coarse pointer's surface.
    pointers: ["composer-touch"],
    open: async (page, hub) => {
      await seedComment(hub);
      await page.goto(viewer(hub));
      await page.getByRole("button", { name: /^Comments, \d+$/ }).first().click();
    },
  },
  {
    name: "comment reply",
    label: "Reply",
    action: "Post",
    empty: "Write a comment to post it.",
    open: async (page, hub) => {
      await seedComment(hub);
      await page.goto(viewer(hub));
      await expect(page.locator("#hub-frame")).toBeVisible();
      // A thread opens from its highlight in the document, which lives in a
      // sandboxed frame; the call is what that press makes.
      await page.evaluate(async () => {
        const comments = await import("./comments.mjs");
        for (let i = 0; i < 50 && !comments.commentsState.comments.length; i += 1) {
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
        comments.openCommentSheet(comments.commentsState.comments[0]);
      });
    },
  },
  {
    name: "wiki comment",
    label: "Comment",
    action: "Post",
    empty: "Write a comment to post it.",
    open: async (page, hub) => {
      const wiki = await seedWikiPage(hub, `composer/${Date.now()}.md`);
      await page.goto(
        `${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/wiki?page=${encodeURIComponent(wiki.name)}`,
      );
    },
  },
];

// The geometry a reader sees: the field's height, the send control's box, and
// how far the send control's centre is from the centre of the field's last
// visible line.
function geometry([field, send]) {
  const style = getComputedStyle(field);
  const f = field.getBoundingClientRect();
  const s = send.getBoundingClientRect();
  const line = parseFloat(style.lineHeight);
  const lastLine = f.bottom - parseFloat(style.borderBottomWidth) - parseFloat(style.paddingBottom) - line / 2;
  return {
    fieldHeight: f.height,
    fieldWidth: f.width,
    sendWidth: s.width,
    sendHeight: s.height,
    offset: s.top + s.height / 2 - lastLine,
    scrolls: field.scrollHeight > field.clientHeight + 1,
    sideways: field.scrollWidth > field.clientWidth + 1,
    pageSideways: document.documentElement.scrollWidth > innerWidth,
    inline: field.getAttribute("style"),
  };
}

async function measure(field, send) {
  const handles = [await field.elementHandle(), await send.elementHandle()];
  return field.page().evaluate(geometry, handles);
}

for (const target of COMPOSERS) {
  test(`the ${target.name} composer is the one composer`, async ({ hub, page }, testInfo) => {
    const size = SIZES[testInfo.project.name];
    test.skip(!size, "the composer is held by its own two projects");
    test.skip(Boolean(target.pointers) && !target.pointers.includes(testInfo.project.name), "not on this pointer");
    expect(
      await page.evaluate(() => matchMedia("(pointer: coarse)").matches),
      "the project's pointer is not the one it is named for",
    ).toBe(size.coarse);

    const posts = [];
    await page.route(/\/api\/v1\/.*(answer|comments)(\?|$)/, async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      posts.push(route.request().url());
      return route.fulfill({
        status: 409,
        contentType: "application/json",
        body: JSON.stringify({ error: "refused by the check" }),
      });
    });

    await pinFont(page);
    await target.open(page, hub);
    const field = page.getByRole("textbox", { name: target.label, exact: true }).last();
    await expect(field, "the composer never opened").toBeVisible();
    await fontsSettled(page);
    const form = formOf(field);
    const send = form.getByRole("button", { name: target.action, exact: true });
    await expect(send).toBeVisible();
    expect(
      await field.evaluate((el) => Boolean(el.closest('[role="log"], [aria-live]'))),
      "the composer sits inside a live region, which reads out every change to it",
    ).toBe(false);
    await expect(form.locator("svg"), "the send control draws no glyph").toHaveCount(1);

    // Empty: the send control is dimmed and says why, in words under the field.
    await field.fill("");
    await expect(send, "an empty composer offers its send").toBeDisabled();
    await expect(send).toHaveAccessibleDescription(target.empty);
    await expect(form.getByText(target.empty, { exact: true })).toBeVisible();
    // A press on the dimmed control sends nothing and puts the caret in the
    // field. The press is the pointer's own, at the control's centre, because
    // a dimmed control is not one the runner's click will act on.
    await field.blur();
    const dimmed = await send.boundingBox();
    await page.mouse.click(dimmed.x + dimmed.width / 2, dimmed.y + dimmed.height / 2);
    await expect(field, "a press on the dimmed send did not put the caret in the field").toBeFocused();
    expect(posts, "a press on the dimmed send sent").toEqual([]);

    const rest = await measure(field, send);
    expect(rest.inline, "the field carries an inline style").toBeNull();
    expect(rest.fieldHeight, "the field at rest is not the pointer's form size").toBeCloseTo(size.field, 0);
    expect(rest.sendWidth, "the send control is not the pointer's control size").toBeCloseTo(size.send, 0);
    expect(rest.sendHeight, "the send control is not the pointer's control size").toBeCloseTo(size.send, 0);
    expect(Math.abs(rest.offset), "the send control is off the field's line").toBeLessThanOrEqual(0.5);

    // Growth: one line at a time, to the cap, and then the field scrolls.
    for (const n of [1, 3, MAX_LINES, MAX_LINES + 4]) {
      await field.fill(lines(n));
      const shown = Math.min(n, MAX_LINES);
      const at = await measure(field, send);
      expect(at.fieldHeight, `${n} lines`).toBeCloseTo(size.field + (shown - 1) * LINE, 0);
      expect(Math.abs(at.offset), `${n} lines: the send control is off the last line`).toBeLessThanOrEqual(0.5);
      expect(at.scrolls, `${n} lines: the field scrolls`).toBe(n > MAX_LINES);
      await expect(send, `${n} lines: the send control is dimmed with words in the field`).toBeEnabled();
    }
    await field.fill("one line again");
    expect((await measure(field, send)).fieldHeight, "the field did not shrink back").toBeCloseTo(size.field, 0);

    // A long word wraps in the field rather than widening it or the page.
    await field.fill("x".repeat(400));
    const long = await measure(field, send);
    expect(long.fieldWidth, "a long word widened the field").toBeCloseTo(rest.fieldWidth, 0);
    expect(long.sideways, "a long word scrolls the field sideways").toBe(false);
    expect(long.pageSideways, "a long word scrolls the page sideways").toBe(false);

    // The keyboard rule, and the hint that names it on a fine pointer.
    await field.fill("two words");
    const keys = form.getByText(/^(Ctrl|Cmd)\+Enter to (send|post)$/);
    if (size.coarse) {
      await expect(keys, "a touch keyboard is told a key it does not have").toHaveCount(0);
      await expect(send, "a touch reader hears a key they do not have").toHaveAccessibleDescription("");
    } else {
      await expect(keys).toHaveText(`Ctrl+Enter to ${target.action.toLowerCase()}`);
      await expect(send).toHaveAccessibleDescription(`Ctrl+Enter to ${target.action.toLowerCase()}`);
    }
    // Under the pointer the enabled control keeps its glyph: the ink on the
    // fill is held to the 3:1 a control's mark needs.
    await send.hover();
    const hovered = await send.evaluate((el) => {
      const rgb = (value) => value.match(/\d+(\.\d+)?/g).slice(0, 3).map(Number);
      const lum = ([r, g, b]) => {
        const c = [r, g, b].map((v) => {
          const x = v / 255;
          return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
      };
      const style = getComputedStyle(el);
      const [a, b] = [lum(rgb(style.color)), lum(rgb(style.backgroundColor))].sort((x, y) => y - x);
      return (a + 0.05) / (b + 0.05);
    });
    expect(hovered, "the send glyph fades into its fill under the pointer").toBeGreaterThanOrEqual(3);
    await field.press("Enter");
    await expect(field, "Enter did not break the line").toHaveValue("two words\n");
    await field.press("Shift+Enter");
    await expect(field).toHaveValue("two words\n\n");
    expect(posts, "Enter sent").toEqual([]);
    await field.press("ControlOrMeta+Enter");
    await expect(form.getByRole("alert"), "the refusal is not said in the composer").toContainText("Nothing was sent");
    expect(posts.length, "the command key with Enter did not send once").toBe(1);
    await expect(field, "the refused words were lost").toHaveValue("two words\n\n");
    await expect(field).toBeFocused();

    // Accessible in both themes, empty and holding words.
    await form.evaluate((el) => {
      el.dataset.composerCheck = "1";
    });
    for (const theme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: theme });
      for (const value of ["", lines(3)]) {
        await field.fill(value);
        const results = await new AxeBuilder({ page })
          .include("[data-composer-check]")
          .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
          .analyze();
        expect(results.violations.map((v) => `${theme}: ${v.id} ${v.help}`)).toEqual([]);
      }
    }
  });
}

// A send on its way holds the composer: a second press of the key sends
// nothing, the field keeps focus so a phone keeps its keyboard, and once the
// hub says yes the field is empty and still has the caret. A thread's reply
// composer is the one that stays on screen at both pointers after a send.
test("a slow send is sent once and leaves the caret in an empty field", async ({ hub, page }, testInfo) => {
  test.skip(!SIZES[testInfo.project.name], "the composer is held by its own two projects");
  await seedComment(hub);
  let posts = 0;
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  await page.route(/\/api\/v1\/artifacts\/[^/]+\/comments$/, async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    posts += 1;
    await held;
    return route.continue();
  });
  await COMPOSERS.find((entry) => entry.name === "comment reply").open(page, hub);
  const field = page.getByRole("textbox", { name: "Reply", exact: true });
  await field.fill("a slow reply");
  await field.press("ControlOrMeta+Enter");
  await expect.poll(() => posts, { message: "the first press sent nothing" }).toBe(1);
  await field.press("ControlOrMeta+Enter");
  await expect(field, "the field let go of the caret while the send was on its way").toBeFocused();
  release();
  await expect(field, "the sent words stayed in the field").toHaveValue("");
  await expect(field, "the caret left the field after the send").toBeFocused();
  expect(posts, "a second press sent the reply again").toBe(1);
});

// A reply in a thread's sheet draws the thread again with the reply in it,
// and the reader is put back in the new composer under it.
test("a reply in a thread puts the caret back in the thread's composer", async ({ hub, page }, testInfo) => {
  test.skip(!SIZES[testInfo.project.name], "the composer is held by its own two projects");
  await seedComment(hub);
  const reply = COMPOSERS.find((entry) => entry.name === "comment reply");
  await reply.open(page, hub);
  const field = page.getByRole("textbox", { name: "Reply", exact: true });
  await field.fill("a reply from the check");
  const posted = page.waitForResponse(
    (response) => response.request().method() === "POST" && /\/comments$/.test(new URL(response.url()).pathname),
  );
  await field.press("ControlOrMeta+Enter");
  expect((await posted).ok(), "the reply was not posted").toBe(true);
  await expect(page.getByRole("textbox", { name: "Reply", exact: true })).toBeFocused();
  await expect(page.getByRole("textbox", { name: "Reply", exact: true })).toHaveValue("");
});

// What the composer looks like, at rest and holding three lines, in both
// themes. The question card's is photographed because its quick answers make
// it the fullest one; every other composer is the same component.
test("the composer looks the same as its baseline @visual", async ({ hub, page }, testInfo) => {
  test.skip(!SIZES[testInfo.project.name], "the composer is held by its own two projects");
  await pinFont(page);
  await page.goto(`${hub.baseUrl}/#/inbox?open=${encodeURIComponent(hub.questionId)}`);
  const field = page.getByRole("textbox", { name: "Your answer" });
  await expect(field).toBeVisible();
  await fontsSettled(page);
  const form = formOf(field);
  // The focus ring is drawn outside the form's box, so the photograph takes
  // the gutter around it too.
  const around = async () => {
    const box = await form.boundingBox();
    return { x: box.x - 8, y: box.y - 8, width: box.width + 16, height: box.height + 16 };
  };
  for (const theme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: theme });
    await field.fill("");
    await field.blur();
    await expect(page).toHaveScreenshot(`composer-${theme}-rest.png`, { ...SCREENSHOT, clip: await around() });
    await field.fill(lines(3));
    await expect(page).toHaveScreenshot(`composer-${theme}-three-lines.png`, { ...SCREENSHOT, clip: await around() });
  }
});
