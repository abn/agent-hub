// The feed stage's actions follow the event's state: a decided or answered
// event offers nothing to answer, an open one offers the row's Answer, and the
// stage, the decision dialog and the capture dialog say what they are.
//
// The checks post their own events into a project of their own, so reading its
// feed moves no cursor the other checks count against.

import { api, expect, goto, newSession, structured, test, toolCall } from "./invariants.mjs";

const feedHash = (project, event) =>
  `#/projects/${encodeURIComponent(project)}/feed${event ? `?event=${encodeURIComponent(event)}` : ""}`;

async function ownProject(hub, tag) {
  const id = `stage-${tag}-${Date.now().toString(36)}`;
  await api(hub, "POST", "/api/v1/projects", { id, display_name: `Stage ${tag}` });
  return id;
}

async function post(hub, project, kind, summary, fields = {}) {
  const session = await newSession(hub);
  await toolCall(hub, session, "signal_append", { project_id: project, kind, summary, ...fields });
  const feed = (await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(project)}/feed?limit=50`)).json();
  return (feed.events.find((event) => event.summary === summary) || {}).id || "";
}

const stage = (page) => page.locator(".feed-stage");

test.describe("the feed stage", () => {
  test("a declined approval shows no verbs and names its outcome", async ({ hub, page }) => {
    const project = await ownProject(hub, "declined");
    const id = await post(hub, project, "approval", "Spare approval declined");
    const decided = await api(hub, "POST", `/api/v1/approvals/${id}/decision`, { decision: "decline", note: "Not now." });
    expect(decided.status, "the decline did not go through").toBeLessThan(300);

    await goto(page, feedHash(project, id));
    await expect(stage(page).getByRole("heading", { name: "Spare approval declined" })).toBeVisible();
    await expect(stage(page).getByRole("button", { name: "Approve" })).toHaveCount(0);
    await expect(stage(page).getByRole("button", { name: "Reply" })).toHaveCount(0);
    await expect(stage(page).getByRole("link", { name: "Answer" })).toHaveCount(0);
    await expect(stage(page).locator(".feed-stage-kind")).toContainText("Declined");
    await expect(stage(page)).toContainText("declined: Not now.");
    await expect(stage(page)).not.toContainText("answered: Declined");
  });

  test("the outcome holds under the kind filter, which loads no replies", async ({ hub, page }) => {
    test.skip(test.info().project.name === "phone", "the phone keeps the kind filter behind its own control");
    const project = await ownProject(hub, "filtered");
    const id = await post(hub, project, "approval", "Spare approval filtered");
    await api(hub, "POST", `/api/v1/approvals/${id}/decision`, { decision: "decline", note: "Not now." });
    await goto(page, feedHash(project, id));
    await expect(stage(page).locator(".feed-stage-kind")).toContainText("Declined");
    await page.getByRole("button", { name: /^Group/ }).first().click();
    await page.getByRole("menuitemradio", { name: /^Approvals/ }).click();
    // The filtered paint is done once the pill names the kind and no reply row is left.
    await expect(page.locator(".shell-group-pill")).toHaveText("Approvals");
    await expect(page.locator(".feed-row")).toHaveCount(1);
    await expect(stage(page).getByRole("heading", { name: "Spare approval filtered" })).toBeVisible();
    await expect(stage(page).locator(".feed-stage-kind")).toContainText("Declined");
    await expect(stage(page).locator(".feed-stage-kind")).not.toContainText(/Decided|Closed/);
  });

  test("an answered question shows no verbs and names its outcome", async ({ hub, page }) => {
    const project = await ownProject(hub, "answered");
    const session = await newSession(hub);
    await toolCall(hub, session, "question_post", { project_id: project, subject: "Roll the log files?" });
    const held = (await api(hub, "GET", "/api/v1/inbox?status=action&limit=500")).json();
    const item = held.items.find((entry) => entry.summary === "Roll the log files?");
    expect(item, "the question was not posted").toBeTruthy();
    const said = await api(hub, "POST", `/api/v1/questions/${item.event_id}/answer`, { body: "Yes, roll them." });
    expect(said.status, "the answer did not go through").toBeLessThan(300);

    await goto(page, feedHash(project, item.event_id));
    await expect(stage(page).getByRole("heading", { name: "Roll the log files?" })).toBeVisible();
    await expect(stage(page).getByRole("button", { name: "Reply" })).toHaveCount(0);
    await expect(stage(page).getByRole("button", { name: "Approve" })).toHaveCount(0);
    await expect(stage(page).locator(".feed-stage-kind")).toContainText("Answered");
  });

  test("an open approval offers the row's Answer, which opens the item", async ({ hub, page }) => {
    const project = await ownProject(hub, "open");
    const id = await post(hub, project, "approval", "Spare approval open");
    await goto(page, feedHash(project, id));
    const answer = stage(page).getByRole("link", { name: "Answer" });
    await expect(answer).toBeVisible();
    await expect(stage(page).getByRole("button", { name: "Approve" })).toHaveCount(0);
    await answer.click();
    await expect(page).toHaveURL(new RegExp(`#/inbox\\?open=${id}`));
    await expect(page.getByRole("heading", { name: "Spare approval open" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Decline" }).first()).toBeVisible();
  });

  test("the crumb names the staged event's kind, not its reply's", async ({ hub, page }) => {
    const project = await ownProject(hub, "crumb");
    const id = await post(hub, project, "approval", "Spare approval crumb");
    await api(hub, "POST", `/api/v1/approvals/${id}/decision`, { decision: "decline", note: "Not now." });
    const feed = (await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(project)}/feed?limit=50`)).json();
    const reply = feed.events.find((event) => event.kind === "answer");
    expect(reply, "the decision left no reply in the feed").toBeTruthy();

    await goto(page, feedHash(project, reply.id));
    await expect(stage(page).getByRole("heading", { name: "Spare approval crumb" })).toBeVisible();
    const crumb = page.locator(".shell-stage .shell-head .shell-meta, .shell-stage .shell-controls .shell-meta").last();
    await expect(crumb).toHaveText(new RegExp(`${project} / feed / approval$`));
  });

  test("a long subject's title never ends in a comma and an ellipsis", async ({ hub, page }) => {
    const project = await ownProject(hub, "long");
    const subject =
      "Before the nightly roll runs tonight, the archive on the second disk needs a decision about whether to keep the old logs, or move them somewhere cheaper for good";
    const id = await post(hub, project, "finished", subject);
    await goto(page, feedHash(project, id));
    const title = stage(page).locator(".feed-stage-title");
    await expect(title).toBeVisible();
    expect(await title.innerText()).not.toMatch(/[,;:]…$/);
  });

  test("the points-at card names the session and sets its head in sans", async ({ hub, page }) => {
    const project = await ownProject(hub, "points");
    const session = await newSession(hub);
    const started = await toolCall(hub, session, "session_start", { project_id: project, session_name: "points-check" });
    const sessionId = structured(started).session_id || "";
    expect(sessionId, "no session started").toBeTruthy();
    const feed = (await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(project)}/feed?limit=50`)).json();
    const event = feed.events.find((entry) => entry.kind === "session");
    expect(event, "the session left no feed event").toBeTruthy();

    await goto(page, feedHash(project, event.id));
    const card = page.locator(".points-card");
    await expect(card).toBeVisible();
    await expect(card.getByRole("link")).toContainText("points-check");
    await expect(card.locator(".points-meta")).toHaveText(sessionId);
    const sansHead = await card.locator(".points-head").evaluate((el) => {
      const style = getComputedStyle(el);
      const body = getComputedStyle(document.body);
      return { family: style.fontFamily, bodyFamily: body.fontFamily };
    });
    expect(sansHead.family, "the card head is set in mono").toBe(sansHead.bodyFamily);
  });
});

test.describe("the decline dialog and the decided item", () => {
  test("Decline commits in the action fill and the outcome reads declined", async ({ hub, page }) => {
    const project = await ownProject(hub, "decline");
    const id = await post(hub, project, "approval", "Spare approval dialog");
    await goto(page, `#/inbox?open=${encodeURIComponent(id)}`);
    await page.getByRole("button", { name: "Decline" }).first().click();
    const dialog = page.locator("dialog.dialog[open]");
    await expect(dialog).toBeVisible();
    const commit = dialog.locator(".dialog-commit");
    const [fill, action] = await Promise.all([
      commit.evaluate((el) => getComputedStyle(el).backgroundColor),
      page.evaluate(() => {
        const probe = document.createElement("i");
        probe.style.background = "var(--action)";
        document.body.appendChild(probe);
        const value = getComputedStyle(probe).backgroundColor;
        probe.remove();
        return value;
      }),
    ]);
    expect(fill, "the Decline commit is not filled with --action").toBe(action);
    await dialog.getByRole("textbox").fill("Not now.");
    await commit.click();
    await expect(dialog).toHaveCount(0);
    // The phone reads a decided item in a sheet with its own state line.
    if (test.info().project.name === "phone") return;

    await goto(page, `#/inbox?open=${encodeURIComponent(id)}`);
    const state = page.locator(".item-state", { hasText: "Declined" });
    await expect(state).toBeVisible();
    expect(await state.locator("svg").count(), "a declined outcome carries a check glyph").toBe(0);
    await expect(page.locator(".item-thread")).toContainText("declined: Not now.");
    await expect(page.locator(".item-thread")).not.toContainText("answered: Declined");
  });
});

test.describe("capture idea", () => {
  test("is not labelled optional and focuses its field", async ({ hub, page }) => {
    // The phone keeps Capture idea behind its overflow; control-sizing covers it.
    test.skip(test.info().project.name === "phone", "the phone reaches Capture idea through a menu");
    const project = await ownProject(hub, "idea");
    await goto(page, feedHash(project));
    await page.locator('button[data-action="capture-idea"]:visible').click();
    const dialog = page.locator("dialog.dialog[open]");
    await expect(dialog).toBeVisible();
    await expect(dialog).not.toContainText("optional");
    await expect(dialog.getByRole("textbox", { name: "Idea" })).toBeFocused();
    await page.keyboard.press("Escape");
  });
});
