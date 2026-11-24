// The release checks for members, agents and sessions as a reader of the PWA
// meets them: adding a member shows its token once and keeps it out of the
// repository forms, approving a request can rename it, Now counts sessions, a
// byline names its subject and receipts for every kind of author, and a row
// from before members says so. The last check is not about a screen: the Rust
// and the JavaScript credential writers share one lock and lose no entry.
//
// Each check makes its own member, so none reads another's.

import { execFile, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { api, expect, goto, newSession, structured, test, toolCall } from "./invariants.mjs";
import { parseConfig } from "../plugins/opencode/credential.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const run = promisify(execFile);

// A name no other check on this hub uses, valid as a member's.
const unique = (stem, testInfo) => `${stem}-${testInfo.project.name}-${Date.now().toString(36)}`.toLowerCase();

// The Access screen at either width: the member index beside a stage on a desktop,
// one or the other on a phone. Its own markup, not a shell column, says it painted.
async function openAccess(page, hub, hash) {
  await page.goto(`${hub.baseUrl}/${hash}`);
  await expect(page.locator("main .access-screen, main .member-index-row, main .member-detail-body").first()).toBeVisible();
}

test.describe("a member on the hub", () => {
  test("adding one shows the token once and keeps it out of the project forms", async ({ hub, page }, testInfo) => {
    const name = unique("add", testInfo);
    await openAccess(page, hub, "#/access");
    await page.getByRole("button", { name: "Add member" }).click();
    await page.getByLabel("Member name").fill(name);
    await page.getByLabel("Display name").fill("Added in a release check");
    await page.getByRole("button", { name: "Create member" }).click();

    const dialog = page.locator("dialog.dialog-reveal[open]");
    await expect(dialog).toBeVisible();
    const token = (await dialog.locator(".reveal-token").innerText()).trim();
    expect(token, "no token was revealed").not.toBe("");

    const groups = dialog.locator(".reveal-group");
    const store = groups.filter({ has: page.getByRole("heading", { name: "Store on that machine" }) });
    await expect(store, "the block that goes on the machine is missing").toHaveCount(1);
    const stored = (await store.locator(".reveal-setup-text").allInnerTexts()).join("\n");
    expect(stored, "the token is not in the block for the machine").toContain(token);
    expect(stored).toContain(`member = "${name}"`);

    // Every other block is something a repository may hold: it names the member
    // and never the token.
    const elsewhere = groups.filter({ hasNot: page.getByRole("heading", { name: "Store on that machine" }) });
    expect(await elsewhere.count(), "the project forms are missing").toBeGreaterThan(0);
    const forms = (await elsewhere.locator(".reveal-setup-text").allInnerTexts()).join("\n");
    expect(forms, "the token leaked into a project form").not.toContain(token);
    expect(forms).toContain(`member = "${name}"`);

    // The token leaves the page with the dialog, and only Done closes it.
    await page.keyboard.press("Escape");
    await expect(dialog, "Escape dismissed the one-time token").toBeVisible();
    await dialog.getByRole("button", { name: "Done" }).click();
    await expect(dialog).toHaveCount(0);
    expect(await page.evaluate(() => document.body.innerText), "the token stayed on the page").not.toContain(token);
  });

  test("approving a request under another name lands on that name", async ({ hub, page }, testInfo) => {
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
    await expect(page.getByText("Checking that approval can rename")).toBeVisible();
    await expect(page.getByText(`Approving covers every agent on ${asked}.`)).toBeVisible();
    await page.getByLabel("Member name").fill(approved);
    await page.getByRole("button", { name: "Approve" }).click();

    await expect(page).toHaveURL(new RegExp(`#/access\\?member=${approved}$`));
    const members = (await api(hub, "GET", "/api/v1/members")).json().members;
    expect(members.map((m) => m.id)).toContain(approved);
    expect(members.map((m) => m.id), "the old name is still a member").not.toContain(asked);
  });

  test("Now counts live sessions and folds a subagent under its parent", async ({ hub, page }, testInfo) => {
    const member = unique("live", testInfo);
    await api(hub, "POST", "/api/v1/members", { id: member, display_name: "Live probe" });
    const token = (await api(hub, "POST", `/api/v1/members/${encodeURIComponent(member)}/token`)).json().token;
    const as = { ...hub, token };
    const session = await newSession(as);
    const parent = `count-${testInfo.project.name}`;
    for (const name of [parent, `${parent}/review-ab12cd34`]) {
      const started = await toolCall(as, session, "session_start", { project_id: hub.projectId, session_name: name });
      expect(structured(started).session_id, `${name} did not start`).toBeTruthy();
    }

    const live = (await api(hub, "GET", "/api/v1/sessions?live=1")).json().sessions;
    const roots = live.filter((s) => !s.parent_id).length;
    const children = live.length - roots;
    expect(children, "the child session is not live on the hub").toBeGreaterThan(0);

    // Now keeps the live sessions in its margin, which a phone does not draw;
    // the member's own stage counts them the same way at either width.
    const counted = new RegExp(`^${roots} live sessions?, ${children} subagents?$`);
    if (testInfo.project.name === "desktop") {
      await goto(page, "#/home");
      const who = page.locator(".now .now-who");
      await expect(who, "Now counts sessions, not agents, and folds a subagent under its parent").toHaveText(
        counted,
      );
    }
    await openAccess(page, hub, `#/access?member=${encodeURIComponent(member)}`);
    await expect(
      page.locator(".member-detail-label, .access-section-label").filter({ hasText: /live sessions/i }),
      "the stage counts one session and its subagent apart",
    ).toHaveText(/live sessions\s*·?\s*1 · 1 subagent$/i);
  });
});

// What a reader sees beside a feed row, for each kind of author. The feed is
// answered from a fixed page, so the words are the author record's alone.
const NOW = Date.now();
const ago = (minutes) => new Date(NOW - minutes * 60000).toISOString();

const participant = (agent, member, session) => ({
  kind: "participant",
  member,
  agent,
  session: session ? { id: `s-${session}`, name: session } : null,
});

const ROWS = [
  { n: 6, author: { kind: "legacy", label: "laptop/claude" }, origin: null, summary: "Session old-run ended", words: "laptop/claude", receipts: "before members" },
  { n: 5, author: { kind: "local", agent: "claude-code", session: { id: "s-scratch", name: "scratch" } }, origin: null, summary: "Cache hit rate 94%", words: "scratch", receipts: "claude-code · local" },
  { n: 4, author: { kind: "human" }, origin: "hub", summary: "Finished the nightly run", words: "the hub", receipts: "" },
  { n: 3, author: { kind: "human" }, origin: null, summary: "Finished the audit", words: "you", receipts: "" },
  { n: 2, author: participant("opencode", "desktop", null), origin: null, summary: "Disk at 40%", words: "opencode", receipts: "desktop" },
  { n: 1, author: participant("claude-code", "laptop", "feat-x"), origin: null, summary: "The nightly run scored 4 of 5", words: "feat-x", receipts: "claude-code · laptop" },
];

async function openBylines(page, hub) {
  const events = ROWS.map((row) => ({
    id: `e000${row.n}`,
    project_id: hub.projectId,
    kind: "signal",
    actor: "opaque-key",
    author: row.author,
    origin: row.origin,
    summary: row.summary,
    payload: null,
    thread_id: null,
    needs_action: false,
    created_at: ago(row.n),
    inbox_status: null,
  }));
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\?/, (route) =>
    route.fulfill({ json: { events, last_seen: "e0006", next_before: null } }),
  );
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\/seen$/, (route) => route.fulfill({ status: 204, body: "" }));
  // Two members, so receipts name the member rather than leaving it out.
  await page.route(/\/api\/v1\/members$/, (route) =>
    route.fulfill({ json: { members: [{ id: "laptop", state: "active" }, { id: "desktop", state: "active" }] } }),
  );
  await page.goto(`${hub.baseUrl}/#/projects/${encodeURIComponent(hub.projectId)}/feed`);
  const rows = page.locator(".shell-index .feed-row");
  await expect(rows).toHaveCount(ROWS.length);
  return rows;
}

test.describe("who wrote a row", () => {
  test("a byline names the subject and the receipts for each kind of author", async ({ hub, page }) => {
    const rows = await openBylines(page, hub);
    for (const [at, want] of ROWS.entries()) {
      const row = rows.nth(at);
      await expect(row.locator(".by-s"), `row ${at}: the subject`).toHaveText(want.words);
      if (want.receipts) await expect(row.locator(".by-r"), `row ${at}: the receipts`).toHaveText(want.receipts);
      else await expect(row.locator(".by-r"), `row ${at}: receipts where there are none`).toHaveCount(0);
    }
    // The wire's opaque key is never drawn.
    await expect(page.locator(".shell-index")).not.toContainText("opaque-key");
  });

  test("a row from before members keeps its stored label and says so", async ({ hub, page }) => {
    const rows = await openBylines(page, hub);
    const legacy = rows.first();
    await expect(legacy.locator(".by-s")).toHaveText("laptop/claude");
    const said = await legacy.evaluate(
      (el) => `${el.innerText} ${[...el.querySelectorAll("[title]")].map((node) => node.title).join(" ")}`,
    );
    expect(said, "a reader cannot tell why the row has no member").toContain("before members");
  });
});

// The Rust and the JavaScript writers take one lock on config.toml. Fifty
// entries from each, written at once into one file, all survive.
test.describe("the credential store", () => {
  test("the Rust and the JavaScript writer share one lock and lose no entry", async () => {
    const bin = process.env.HUB_BIN || path.join(ROOT, "target", "debug", "agent-hub");
    expect(existsSync(bin), `the hub binary is not built at ${bin}`).toBe(true);

    const scratchRoot = path.join(ROOT, "target", "tmp");
    mkdirSync(scratchRoot, { recursive: true });
    const scratch = mkdtempSync(path.join(scratchRoot, "release-lock-"));
    const url = "https://hub.example.net";
    const config = path.join(scratch, "config.toml");
    try {
      // Fifty working trees, each with a project file naming a member of its own.
      const trees = Array.from({ length: 50 }, (_, i) => {
        const tree = path.join(scratch, `tree-${i}`);
        mkdirSync(path.join(tree, ".git"), { recursive: true });
        writeFileSync(path.join(tree, ".agent-hub.toml"), `project = "p"\nurl = "${url}"\nmember = "rust-${i}"\n`);
        // `trust` records the path as the system names it.
        return realpathSync(tree);
      });
      const env = { PATH: process.env.PATH, HOME: scratch, HUB_CONFIG: config, RUST_LOG: "error" };

      // Every writer is a process of its own, all started before any is awaited:
      // two JavaScript writers (twenty-five entries each, a few milliseconds
      // apart) and four queues of `trust` runs are in flight together, so the lock
      // is contended. More at once only outwait the lock's five second budget on a
      // busy host, which is a refusal, not a lost write.
      const mod = path.join(ROOT, "plugins", "opencode", "credential.mjs");
      const script = (from) =>
        `import { writeCredential } from ${JSON.stringify(mod)}; import { setTimeout as sleep } from "node:timers/promises"; ` +
        `for (let i = ${from}; i < ${from + 25}; i++) { writeCredential(${JSON.stringify(config)}, ${JSON.stringify(url)}, "js-" + i, "tok-js-" + i); await sleep(Math.random() * 8); }`;
      const js = Array.from({ length: 2 }, (_, n) =>
        new Promise((resolve) => {
          const child = spawn(process.execPath, ["--input-type=module", "-e", script(n * 25)], { stdio: ["ignore", "ignore", "pipe"] });
          let err = "";
          child.stderr.on("data", (chunk) => (err += chunk));
          child.on("close", (code) => resolve({ code, err }));
        }),
      );
      let next = 0;
      const worker = async () => {
        while (next < trees.length) {
          const tree = trees[next++];
          await run(bin, ["trust", "--path", tree], { env, cwd: tree });
        }
      };
      const rust = Array.from({ length: 4 }, worker);
      const [written] = await Promise.all([Promise.all(js), Promise.all(rust)]);
      for (const result of written) expect(result, result.err).toMatchObject({ code: 0 });

      const text = readFileSync(config, "utf8");
      const parsed = parseConfig(text);
      expect(
        parsed.credentials.map((c) => `${c.member} ${c.token}`).sort(),
        "a JavaScript write was lost",
      ).toEqual(Array.from({ length: 50 }, (_, i) => `js-${i} tok-js-${i}`).sort());
      expect(
        parsed.trust.pairings.map((p) => `${p.member} ${p.path}`).sort(),
        "a Rust write was lost",
      ).toEqual(trees.map((tree, i) => `rust-${i} ${tree}`).sort());
      expect(statSync(config).mode & 0o777, "the file holds tokens").toBe(0o600);
      expect(readdirSync(scratch).filter((entry) => entry.startsWith("config.toml") && entry !== "config.toml"), "a lock or temp file was left behind").toEqual([]);
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  });
});
