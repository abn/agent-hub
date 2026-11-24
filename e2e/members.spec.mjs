// Members: adding one shows its token once and splits what goes on the machine
// from the two forms that go in a repository, a pending member is approved under a name of
// the operator's choosing, and a member's live sessions are listed with the verb
// that ends them. Each check makes its own member, so none reads another's.

import { expect } from "@playwright/test";
import { api, goto, newSession, structured, test, toolCall } from "./invariants.mjs";

// A name no other check on this hub uses, valid as a member's.
const unique = (stem, testInfo) => `${stem}-${testInfo.project.name}-${Date.now().toString(36)}`.toLowerCase();

// The Access screen's own body: the shell's index is a desktop pane, and a
// phone paints the screen alone.
async function openAccess(page, hub, route) {
  await page.goto(`${hub.baseUrl}/${route}`);
  await expect(page.locator('.shell[data-segment="access"] .shell-index .shell-body, main .access-screen, main .member-detail-body').filter({ visible: true }).first()).toBeVisible();
}

const memberRow = (page, name) => page.locator(".member-index-row, .member-row").filter({ hasText: name });

test.describe("adding a member", () => {
  test("shows the token once, with the store and the two project forms apart", async ({ hub, page }, testInfo) => {
    const name = unique("add", testInfo);
    await openAccess(page, hub, "#/access");
    await page.getByRole("button", { name: "Add member" }).click();
    await page.getByLabel("Member name").fill(name);
    await page.getByLabel("Display name").fill("Added in a check");
    await page.getByRole("button", { name: "Create member" }).click();

    const dialog = page.locator("dialog.dialog-reveal[open]");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("heading", { name: "Token for Added in a check" })).toBeVisible();
    const token = (await dialog.locator(".reveal-token").innerText()).trim();
    expect(token, "no token was revealed").not.toBe("");

    const store = dialog.locator(".reveal-group").filter({ has: page.getByRole("heading", { name: "Store on that machine" }) });
    const binary = dialog.locator(".reveal-group").filter({ has: page.getByRole("heading", { name: "With the agent-hub binary" }) });
    const without = dialog.locator(".reveal-group").filter({ has: page.getByRole("heading", { name: "Without it" }) });
    await expect(store, "the store block is missing").toHaveCount(1);
    await expect(binary, "the binary block is missing").toHaveCount(1);
    await expect(without, "the environment block is missing").toHaveCount(1);

    const stored = await store.locator(".reveal-setup-text").allInnerTexts();
    expect(stored.join("\n"), "the token is not in the store block").toContain(token);
    expect(stored.join("\n")).toContain("[[credential]]");
    expect(stored.join("\n")).toContain(`member = "${name}"`);

    // With the binary the entry names no variable: it finds the token in the store.
    const withBinary = (await binary.locator(".reveal-setup-text").allInnerTexts()).join("\n");
    expect(withBinary, "the token leaked into the project config").not.toContain(token);
    expect(withBinary).toContain(`member = "${name}"`);
    expect(withBinary).toContain('"args": [\n        "mcp"');
    expect(withBinary).not.toContain("HUB_TOKEN");
    // Without it each entry reads the token from the environment.
    const committed = (await without.locator(".reveal-setup-text").allInnerTexts()).join("\n");
    expect(committed, "the token leaked into the project config").not.toContain(token);
    expect(committed).toContain("Bearer ${HUB_TOKEN}");
    expect(committed).toContain("Bearer {env:HUB_TOKEN}");

    // Each string is copied from the row that owns it, with a glyph.
    for (const group of [store, binary, without]) {
      const copies = group.getByRole("button", { name: /^Copy / });
      expect(await copies.count(), "a snippet has no copy glyph").toBe(await group.locator(".reveal-setup-text").count());
    }

    // Done is the only way out, and the token leaves the page with the dialog.
    await page.keyboard.press("Escape");
    await expect(dialog, "Escape dismissed the one-time token").toBeVisible();
    await dialog.getByRole("button", { name: "Done" }).click();
    await expect(dialog).toHaveCount(0);
    expect(await page.evaluate(() => document.body.innerText), "the token stayed on the page").not.toContain(token);
  });

  test("refuses a name the hub would not take, before it asks", async ({ hub, page }) => {
    await openAccess(page, hub, "#/access");
    await page.getByRole("button", { name: "Add member" }).click();
    await page.getByLabel("Member name").fill("Not A Host");
    await page.getByLabel("Display name").fill("Bad name");
    await page.getByRole("button", { name: "Create member" }).click();
    await expect(page.locator("dialog[open]")).toHaveCount(0);
    await expect(page.getByText(/Lowercase letters, digits, dots and dashes only\./).first()).toBeVisible();
  });
});

test.describe("approving a member", () => {
  test("renames it on approval and lands on the name it was approved under", async ({ hub, page }, testInfo) => {
    const asked = unique("ask", testInfo);
    const approved = unique("ok", testInfo);
    const enrol = await api(hub, "POST", "/api/v1/enrol", {
      member: asked,
      display_name: asked,
      why: "Checking that approval can rename",
      agent: "claude-code",
      os: "linux",
      host: asked,
    });
    expect(enrol.status, enrol.text).toBeLessThan(300);

    await openAccess(page, hub, `#/access?member=${encodeURIComponent(asked)}`);
    await expect(page.locator(".member-approve")).toBeVisible();
    // What the machine said about itself is read before Approve.
    const facts = await page.locator(".member-facts").innerText();
    expect(facts).toContain("claude-code");
    expect(facts).toContain("linux");
    await expect(page.getByText("Checking that approval can rename")).toBeVisible();
    await expect(page.getByText(`Approving covers every agent on ${asked}.`)).toBeVisible();

    await page.getByLabel("Member name").fill(approved);
    await page.getByRole("button", { name: "Approve" }).click();

    await expect(page).toHaveURL(new RegExp(`#/access\\?member=${approved}$`));
    await expect(page.locator(".member-approve")).toHaveCount(0);
    await expect(page.getByText("Live sessions", { exact: false }).first()).toBeVisible();
    const members = (await api(hub, "GET", "/api/v1/members")).json().members;
    expect(members.map((m) => m.id)).toContain(approved);
    expect(members.map((m) => m.id), "the old name is still a member").not.toContain(asked);
  });

  test("a refused request is gone from the list", async ({ hub, page }, testInfo) => {
    const asked = unique("no", testInfo);
    await api(hub, "POST", "/api/v1/enrol", { member: asked, display_name: asked, why: "Checking that refusal removes it", agent: "cli" });
    await openAccess(page, hub, `#/access?member=${encodeURIComponent(asked)}`);
    await page.getByRole("button", { name: "Refuse" }).click();
    await page.locator("dialog[open] .dialog-commit").click();
    await expect(page).toHaveURL(/#\/access$/);
    await expect(memberRow(page, asked)).toHaveCount(0);
  });
});

test.describe("a member's live sessions", () => {
  test("lists them under the member and ends one from there", async ({ hub, page }, testInfo) => {
    const member = unique("live", testInfo);
    await api(hub, "POST", "/api/v1/members", { id: member, display_name: "Live probe" });
    const token = (await api(hub, "POST", `/api/v1/members/${encodeURIComponent(member)}/token`)).json().token;
    const as = { ...hub, token };
    const session = await newSession(as);
    const name = `end-probe-${testInfo.project.name}`;
    const started = await toolCall(as, session, "session_start", { project_id: hub.projectId, session_name: name });
    expect(structured(started).session_id, "the member's session did not start").toBeTruthy();

    await openAccess(page, hub, `#/access?member=${encodeURIComponent(member)}`);
    const row = page.locator(".member-session-row").filter({ hasText: name });
    await expect(row, "the live session is not listed under its member").toBeVisible();
    await expect(row).toContainText(hub.projectId);
    await expect(page.getByText(/Live sessions/i).first()).toBeVisible();

    await row.getByRole("button", { name: `End ${name}` }).click();
    const dialog = page.locator("dialog.dialog[open]");
    await expect(dialog.getByRole("heading", { name: `End ${name}?` })).toBeVisible();
    await expect(dialog).toContainText("for every participant");
    await dialog.locator(".dialog-commit").click();

    await expect(page.locator(".member-session-row").filter({ hasText: name })).toHaveCount(0);
    const ended = (await api(hub, "GET", `/api/v1/sessions?project=${encodeURIComponent(hub.projectId)}`)).json().sessions;
    expect(ended.find((s) => s.session_name === name)?.status, "the session did not end").toBe("ended");
  });

  test("Now counts live sessions, not agents", async ({ hub, page }) => {
    await goto(page, "#/home");
    await expect(page.locator(".now .now-sum")).toBeVisible();
    await expect(page.locator(".now-who")).toHaveText(/^(No live sessions|\d+ live sessions?(, \d+ subagents?)?|Could not list live sessions\.)$/);
  });
});
