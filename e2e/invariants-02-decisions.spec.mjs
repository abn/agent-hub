// The gate and crypto behaviours, and the one-decision guarantees. This is the
// slice of the old invariants.py that drives a real password gate, a public
// page that remembers, a live share link, and two presses that must decide one
// thing once.
//
// The password the gate opens with and the plaintext marker it must show are
// the harness's fixture, read from the descriptor rather than repeated here.

import { api, expect, gateOf, goto, oneOffEvent, oneOffQuestion, settle, test } from "./invariants.mjs";

const storedPassword = (page, project) =>
  page.evaluate((value) => {
    try {
      const raw = window.localStorage.getItem("hub-artifact-passwords");
      return raw ? JSON.parse(raw)[value] ?? null : null;
    } catch {
      return null;
    }
  }, project);

test.describe("gate and crypto", () => {
  // The in-app frame has an opaque origin, so remembering cannot work there. An
  // option that cannot work is not offered: the checkbox is absent from the
  // gate, not merely disabled. Unlocking still works, from typing alone.
  test("the in-app gate does not offer to remember, and unlocks from typing", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    const project = hub.projectId;
    const artifact = hub.protectedId;
    await page.evaluate((value) => {
      location.hash = `#/artifacts?project=${encodeURIComponent(value)}`;
    }, project);
    if (await settle(page, `!!document.querySelector('[data-action="artifact-open"][data-id="${artifact}"]')`, 3000)) {
      await page.click(`[data-action="artifact-open"][data-id="${artifact}"]`);
    } else {
      await page.evaluate((value) => {
        location.hash = `#/artifacts/${encodeURIComponent(value)}`;
      }, artifact);
    }
    const gate = gateOf(page);
    await expect(gate.locator("#hub-password")).toBeVisible({ timeout: 10_000 });
    await expect(gate.locator("#hub-remember")).toHaveCount(0);
    await expect(gate.locator("#hub-forget")).toHaveCount(1);
    await expect(gate.locator("#hub-forget")).toBeHidden();

    const field = gate.locator("#hub-password");
    const toggle = gate.locator("#hub-show-password");
    await expect(toggle).toHaveCount(1);
    expect(await field.getAttribute("type"), "the password field does not start masked").toBe("password");
    await toggle.check();
    expect(await field.getAttribute("type"), "show password left the field masked").toBe("text");
    await toggle.uncheck();
    expect(await field.getAttribute("type"), "clearing show password left the value on screen").toBe("password");

    await field.fill(hub.fixture.protectedPassword);
    await gate.locator('#hub-unlock-form button[type="submit"]').click();
    const opened = gate.frameLocator("#hub-frame");
    await expect(opened.getByText(hub.fixture.protectedBody)).toBeVisible({ timeout: 15_000 });
    await expect(opened.locator("h1").getByText("Sealed note")).toBeVisible({ timeout: 10_000 });
    await expect(opened.locator("body > pre"), "the unlocked note is shown as source").toHaveCount(0);
    const bodyText = await opened.locator("body").innerText();
    expect(bodyText, "the unlocked note does not carry its own text").toContain(hub.fixture.protectedBody);
    await expect(opened.locator("img"), "markup inside a sealed note became an element").toHaveCount(0);
    await expect(opened.locator("body[data-sealed-pwned]"), "a handler inside a sealed note ran").toHaveCount(0);
    expect(
      bodyText,
      "the sealed note's markup did not reach the reader as text",
    ).toContain(hub.fixture.protectedHostile);
  });

  // The public page is a top-level document, so the store works there.
  // Remembering unlocks the next visit without asking, the chrome then offers
  // to forget it, and forgetting sends the next visit back to the gate. A
  // remembered password that no longer opens the artifact is dropped.
  test("the public page remembers, forgets, and drops a stale password", async ({ hub, context }) => {
    const failures = [];
    const page = await context.newPage();
    page.on("dialog", (dialog) => failures.push(`a native dialog fired: ${dialog.message()}`));
    page.on("pageerror", (error) => failures.push(`uncaught error: ${error}`));
    const url = `${hub.baseUrl}/artifacts/${hub.protectedId}`;
    const content = page.frameLocator("#hub-frame");
    try {
      await page.goto(url, { waitUntil: "load" });
      await page.waitForSelector("#hub-password");
      expect(await page.locator("#hub-remember").count(), "the public gate does not offer to remember").toBe(1);
      expect(await page.locator("#hub-forget").isHidden(), "forget shows before anything is remembered").toBe(true);
      await page.fill("#hub-password", hub.fixture.protectedPassword);
      await page.check("#hub-remember");
      await page.click('#hub-unlock-form button[type="submit"]');
      await expect(content.getByText(hub.fixture.protectedBody)).toBeVisible({ timeout: 15_000 });
      expect(await storedPassword(page, hub.projectId), "the ticked box remembered nothing").toBe(
        hub.fixture.protectedPassword,
      );

      await page.goto(url, { waitUntil: "load" });
      await expect(content.getByText(hub.fixture.protectedBody)).toBeVisible({ timeout: 15_000 });
      expect(
        await page.locator("#hub-unlock-form").isHidden(),
        "the gate still asks after unlocking from the remembered password",
      ).toBe(true);
      const forget = page.locator("#hub-forget");
      await expect(forget, "an auto-unlocked artifact offers no way to forget").toBeVisible({ timeout: 5000 });
      await forget.click();
      const note = page.locator("#hub-forget-note");
      expect(await note.getAttribute("role"), "forgetting is not announced in a live region").toBe("status");
      expect((await note.innerText()).trim(), "forgetting says nothing in the page").not.toBe("");
      expect(await storedPassword(page, hub.projectId), "forgetting left the password in the store").toBeNull();

      await page.goto(url, { waitUntil: "load" });
      await page.waitForSelector("#hub-password");
      expect(
        await page.locator("#hub-unlock-form").isHidden(),
        "the gate did not come back after the password was forgotten",
      ).toBe(false);

      await page.evaluate((value) => {
        window.localStorage.setItem("hub-artifact-passwords", JSON.stringify({ [value]: "not the password" }));
      }, hub.projectId);
      await page.goto(url, { waitUntil: "load" });
      const errorLine = page.locator("#hub-unlock-error");
      await expect(errorLine, "a stale remembered password reported nothing").toBeVisible({ timeout: 15_000 });
      expect(await errorLine.innerText(), "the stale password said the wrong thing").toContain("Wrong password");
      expect(
        await page.evaluate(() => document.activeElement && document.activeElement.id),
        "the field does not hold focus after a wrong password",
      ).toBe("hub-password");
      expect(await storedPassword(page, hub.projectId), "a wrong remembered password stayed in the store").toBeNull();
    } finally {
      await page.close();
    }
    expect(failures).toEqual([]);
  });

  // A live share link must not break the owner's own reader. The plain
  // artifact's page is concealed while a link is live, so the frame reads it
  // with a pass instead.
  test("a live share link still reads in the owner's app", async ({ hub, page }) => {
    const project = hub.projectId;
    const artifact = hub.artifactId;
    await api(hub, "POST", `/api/v1/artifacts/${artifact}/share`);
    try {
      await goto(page, `#/artifacts/${encodeURIComponent(artifact)}?project=${encodeURIComponent(project)}`);
      await expect(page.locator("main iframe#hub-frame"), "the viewer opened no frame").toBeVisible({ timeout: 8000 });
      const inner = gateOf(page);
      const document = inner.frameLocator("#hub-frame");
      await expect(document.getByText("check").first(), "the shared artifact did not render").toBeVisible({
        timeout: 15_000,
      });
      const pageText = await inner.locator("body").innerText();
      expect(pageText, "the stage painted the concealed page instead of the document").not.toMatch(
        /Not Found|not found/,
      );
    } finally {
      await api(hub, "DELETE", `/api/v1/artifacts/${artifact}/share`).catch(() => {});
      await goto(page, `#/projects/${encodeURIComponent(project)}/artifacts`, "Artifacts");
    }
  });
});

test.describe("one decision", () => {
  // A second press while a decision is on its way decides nothing twice.
  test("a second press decides nothing twice", async ({ hub, page }) => {
    const eventId = await oneOffEvent(hub, hub.projectId, "approval", "double decision check");
    await goto(page, "#/settings", "Settings");
    await goto(page, "#/inbox", "Inbox");
    const row = `main .inbox-item[data-id="${eventId}"]`;
    expect(await settle(page, `!!document.querySelector('${row}')`), "the seeded approval is not in the inbox").toBe(
      true,
    );
    // The row opens the item; the verbs live on the stage.
    await page.click(`${row} .title a`);
    const approve = `main [data-action="inbox-detail-approve"][data-id="${eventId}"]`;
    expect(await settle(page, `!!document.querySelector('${approve}')`), "the opened approval offers no decision").toBe(
      true,
    );
    let sent = 0;
    page.on("request", (request) => {
      if (request.method() === "POST" && request.url().endsWith(`/api/v1/approvals/${eventId}/decision`)) sent += 1;
    });
    // Hold the decision request so the second press happens while the first is
    // still on its way.
    let release;
    const gate = new Promise((open) => {
      release = open;
    });
    const decision = new RegExp(`/api/v1/approvals/${eventId}/decision`);
    await page.route(decision, async (route) => {
      if (route.request().method() === "POST") await gate;
      // The route is torn down while a held handler may still be waiting; a
      // continue on a route the teardown already released is not a defect.
      await route.continue().catch(() => {});
    });
    try {
      await page.click(approve);
      const dialog = page.locator("dialog.dialog[open]");
      await expect(dialog).toBeVisible();
      // The decision is on its way, held at the network. A second press on the
      // commit is the case the guard is for: it must decide nothing twice.
      const commit = dialog.locator(".dialog-commit");
      await commit.click();
      await commit.click();
      release();
      await expect(page.locator(approve)).toHaveCount(0, { timeout: 8000 });
      await expect(page.locator("dialog.dialog[open]")).toHaveCount(0);
    } finally {
      release();
      await page.unroute(decision).catch(() => {});
      await api(hub, "POST", `/api/v1/approvals/${eventId}/decision`, { decision: "approve" }).catch(() => {});
      await goto(page, "#/inbox", "Inbox");
    }
    expect(sent, "two presses sent more than one decision").toBe(1);
    const said = await page.evaluate(() =>
      [...document.querySelectorAll(".toast-text")].map((node) => node.textContent).join(" | "),
    );
    expect(said, "an approval that went through was reported as a failure").not.toContain("Nothing changed");
    expect(said, "an approval that went through was reported as already done").not.toContain("already");
  });

  // A question that suggests answers offers each as a control on its card,
  // beside the composer, and the keyboard reaches them.
  test("a seeded question offers its quick answers on the card", async ({ hub, page }) => {
    await goto(page, `#/inbox?open=${encodeURIComponent(hub.questionId)}`, "Inbox");
    const group = page.getByRole("group", { name: "Quick answers" });
    await expect(group).toBeVisible();
    for (const option of hub.fixture.questionOptions) {
      const pick = group.getByRole("button", { name: option, exact: true });
      await expect(pick).toBeVisible();
      const box = await pick.boundingBox();
      expect(box.height, `the ${option} answer is under the 44px target`).toBeGreaterThanOrEqual(44);
    }
    await expect(page.getByRole("textbox", { name: "Your answer" }), "quick answers took the composer's place").toBeVisible();
    const first = group.getByRole("button", { name: hub.fixture.questionOptions[0], exact: true });
    await first.focus();
    await expect(first).toBeFocused();
  });

  // A quick answer is an answer: one press sends the option's text once, a
  // second press while it is on its way sends nothing, and the question is
  // answered with exactly those words.
  test("a quick answer answers the question once", async ({ hub, page }) => {
    const options = ["Keep it", "Drop it"];
    const id = await oneOffQuestion(hub, hub.projectId, "quick answer check", { options });
    expect(id, "the quick answer question never reached the inbox").toBeTruthy();
    let sent = 0;
    page.on("request", (request) => {
      if (request.method() === "POST" && request.url().endsWith(`/api/v1/questions/${id}/answer`)) sent += 1;
    });
    let release;
    const gate = new Promise((open) => {
      release = open;
    });
    const answerRoute = new RegExp(`/api/v1/questions/${id}/answer`);
    await page.route(answerRoute, async (route) => {
      if (route.request().method() === "POST") await gate;
      await route.continue().catch(() => {});
    });
    try {
      await goto(page, `#/inbox?open=${encodeURIComponent(id)}`, "Inbox");
      const group = page.getByRole("group", { name: "Quick answers" });
      const drop = group.getByRole("button", { name: "Drop it", exact: true });
      await drop.click();
      // Held at the network: both options are pressed again while it waits.
      await expect(drop).toBeDisabled();
      await drop.dispatchEvent("click");
      await group.getByRole("button", { name: "Keep it", exact: true }).dispatchEvent("click");
      release();
      await expect(page.locator(`main .inbox-detail`)).toHaveCount(0, { timeout: 8000 });
    } finally {
      release();
      await page.unroute(answerRoute).catch(() => {});
    }
    expect(sent, "two presses sent more than one answer").toBe(1);
    const resolved = (await api(hub, "GET", "/api/v1/inbox?status=resolved&limit=500")).json();
    const item = resolved.items.find((entry) => entry.event_id === id);
    expect(item && item.answer && item.answer.body, "the answer is not the option's text").toBe("Drop it");
    const said = await page.evaluate(() =>
      [...document.querySelectorAll(".toast-text")].map((node) => node.textContent).join(" | "),
    );
    expect(said).toContain("Answer sent");
  });

  // A question answered elsewhere while its card is open refuses the pick in
  // the composer, as it refuses a typed reply, and nothing is sent twice.
  test("a quick answer on a question already answered says nothing was sent", async ({ hub, page, watch }) => {
    const id = await oneOffQuestion(hub, hub.projectId, "late quick answer check", { options: ["Yes", "No"] });
    expect(id, "the late quick answer question never reached the inbox").toBeTruthy();
    // The refusal is what this check drives, so the hub's 409 is not a defect.
    watch.ignore(`/api/v1/questions/${id}/answer`);
    await goto(page, `#/inbox?open=${encodeURIComponent(id)}`, "Inbox");
    const yes = page.getByRole("group", { name: "Quick answers" }).getByRole("button", { name: "Yes", exact: true });
    await expect(yes).toBeVisible();
    await api(hub, "POST", `/api/v1/questions/${id}/answer`, { body: "No" });
    await yes.click();
    await expect(page.locator(".inbox-detail .composer-error")).toContainText("Nothing was sent");
    await expect(yes).toBeEnabled();
    const resolved = (await api(hub, "GET", "/api/v1/inbox?status=resolved&limit=500")).json();
    const item = resolved.items.find((entry) => entry.event_id === id);
    expect(item.answer.body, "the late pick replaced the first answer").toBe("No");
  });

  // A toast does not take the keyboard off a reader who is mid-sentence. Undo
  // is the only way back, so a toast moves focus there, but not while the
  // reader is writing.
  test("a toast leaves a writer alone", async ({ hub, page }) => {
    await goto(page, "#/inbox", "Inbox");
    const question = page.locator('.shell-index .inbox-row:has(.glyph[data-kind="question"]) .title a').first();
    await expect(question).toBeVisible();
    await question.click();
    const field = page.locator(".composer-field, textarea").first();
    await expect(field).toBeVisible();
    const typed = "half written reply";
    await field.fill(typed);
    await field.click();
    await page.keyboard.press("End");
    await page.evaluate(() => import("/toast.mjs").then((module) => module.toast("Pruned 1 session.", () => {})));
    await expect(page.locator(".toast-undo, [data-action='undo']").first()).toBeVisible();
    const after = await page.evaluate(() => {
      const el = document.activeElement;
      const field = document.querySelector(".composer-field, textarea");
      return { onField: el === field, value: field ? field.value : null, where: el ? el.className || el.tagName : "" };
    });
    expect(after.onField, `the toast took focus off the composer, onto ${after.where}`).toBe(true);
    expect(after.value, "the toast cost the reader what they typed").toBe(typed);
    const region = await page.evaluate(() => {
      const r = document.querySelector(".toast-region");
      return r && { live: r.getAttribute("aria-live"), text: r.textContent };
    });
    expect(region, "the toast was not announced in a live region").toBeTruthy();
    expect(region.live).toBe("polite");
    expect(region.text).toContain("Pruned 1 session.");
  });
});
