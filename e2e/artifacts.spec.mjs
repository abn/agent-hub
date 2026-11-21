// The artifacts list: every project's artifacts in two tiers, what moved since
// the reader looked over everything earlier, with the older rest folded; the
// margin that previews the focused row on a wide stage; the keys that move and
// open; and the project chips that narrow it.
//
// The tier, fold and preview checks answer the list from a fixed fixture through
// the router, so the counts and the fold are known exactly and nothing is
// written to the hub the other checks share. The checks that open an artifact,
// and the one that proves opening it takes it out of the first tier, read the
// seeded hub itself.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";

const isDesktop = (testInfo) => testInfo.project.name === "desktop";

const NOW = Date.now();
const ago = (minutes) => new Date(NOW - minutes * 60000).toISOString();
const LOOKED = NOW - 90 * 60000;

const fixture = (id, project, minutes, fields = {}) => ({
  id,
  project_id: project,
  session_id: null,
  actor: "docs-writer",
  title: id.replace(/-/g, " "),
  description: "",
  label: null,
  kind: "markdown",
  version: 1,
  protected: false,
  envelope: null,
  size_bytes: 1200,
  created_at: ago(minutes + 600),
  updated_at: ago(minutes),
  live_version: null,
  live_rev: 0,
  live_session: null,
  comments_count: 0,
  comments_open: 0,
  ...fields,
});

// Two moved since the reader looked (one updated past the version they saw,
// one published after they looked); twelve earlier, so two are folded.
const CHECKS = [
  fixture("release-notes", "checks", 10, { version: 3, comments_count: 2, label: "3 lines changed" }),
  fixture("nightly-report", "checks", 5, { created_at: ago(5), kind: "html", actor: "build-bot" }),
  ...Array.from({ length: 11 }, (_, i) => fixture(`earlier-${i + 1}`, "checks", 200 + i * 60)),
];
const ATTIC = [fixture("sealed-plan", "attic", 150, { protected: true, actor: "archivist", version: 2 })];

const SEEN = {
  at: LOOKED,
  seen: Object.fromEntries(
    [...CHECKS, ...ATTIC].filter((a) => a.id !== "nightly-report").map((a) => [a.id, a.id === "release-notes" ? 2 : a.version]),
  ),
};

async function fixedList(page) {
  await page.route(/\/api\/v1\/projects$/, (route) =>
    route.fulfill({
      json: {
        projects: [
          { id: "checks", display_name: "checks", confidential: false },
          { id: "attic", display_name: "attic", confidential: false },
        ],
      },
    }),
  );
  await page.route(/\/api\/v1\/projects\/checks\/artifacts$/, (route) => route.fulfill({ json: { artifacts: CHECKS } }));
  await page.route(/\/api\/v1\/projects\/attic\/artifacts$/, (route) => route.fulfill({ json: { artifacts: ATTIC } }));
  await page.route(/\/api\/v1\/artifacts\/[^/]+\/versions$/, (route) =>
    route.fulfill({
      json: {
        versions: [
          { version: 1, title: "x", description: "", kind: "markdown", label: "skeleton", protected: false, envelope: null, size_bytes: 100, created_at: ago(300) },
          { version: 2, title: "x", description: "", kind: "markdown", label: "notes drafted", protected: false, envelope: null, size_bytes: 900, created_at: ago(120) },
          { version: 3, title: "x", description: "", kind: "markdown", label: "3 lines changed", protected: false, envelope: null, size_bytes: 1200, created_at: ago(10) },
        ],
      },
    }),
  );
  await page.route(/\/api\/v1\/projects\/[^/]+\/feed\?/, (route) =>
    route.fulfill({
      json: { events: [{ id: "01EVENTRELEASE", kind: "artifact", payload: { artifact_id: "release-notes" }, created_at: ago(10) }] },
    }),
  );
  await page.addInitScript((seen) => window.localStorage.setItem("hub.artifacts.seen", seen), JSON.stringify(SEEN));
}

async function openList(page, hub, hash = "#/artifacts") {
  await page.goto(`${hub.baseUrl}/${hash}`);
  await expect(page.locator('.shell[data-segment="artifact-list"] .arts-row').first()).toBeVisible();
}

const rows = (page) => page.locator(".arts-row");
const tier = (page, name) => page.locator("section.arts-tier", { has: page.getByRole("heading", { name }) });

test.describe("the artifacts list", () => {
  test("puts what moved since the reader looked first and folds the earlier rest", async ({ hub, page }) => {
    await fixedList(page);
    await openList(page, hub);
    await expect(page.getByRole("heading", { level: 1, name: "Artifacts" })).toBeVisible();

    const since = tier(page, /^Since you looked · 2$/);
    await expect(since.locator(".arts-row")).toHaveCount(2);
    // Newest first: the report published after the reader looked, then the
    // notes that moved past the version they saw.
    await expect(since.getByRole("link")).toHaveText(["nightly-report.html", "release-notes.md"]);
    await expect(since.locator(".arts-line").last()).toHaveText("checks · docs-writer · v3 · 2 comments");

    const earlier = tier(page, /^Earlier · 12$/);
    await expect(earlier.locator(".arts-row")).toHaveCount(10);
    await expect(earlier.getByText(/^2 more, back to /)).toBeVisible();
    // A protected artifact is drawn with the lock and says it is encrypted.
    const sealed = earlier.locator(".arts-row", { hasText: "sealed-plan.md" });
    await expect(sealed.getByText("Protected")).toBeAttached();
    await expect(sealed.locator(".arts-line")).toHaveText("attic · archivist · v2 · encrypted in your browser");

    await earlier.getByRole("button", { name: "Show all" }).click();
    await expect(tier(page, /^Earlier · 12$/).locator(".arts-row")).toHaveCount(12);
    await expect(page.getByRole("button", { name: "Show all" })).toHaveCount(0);
  });

  test("Mark seen empties the first tier on this device", async ({ hub, page }) => {
    await fixedList(page);
    await openList(page, hub);
    await page.getByRole("button", { name: /^Mark seen/ }).click();
    await expect(page.getByRole("heading", { name: /^Since you looked/ })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: /^Earlier · 14$/ })).toBeVisible();
  });

  test("a project chip narrows the list and its address keeps it", async ({ hub, page }) => {
    await fixedList(page);
    await openList(page, hub);
    const chips = page.getByRole("toolbar", { name: "Filter artifacts" });
    await expect(chips.getByRole("button", { name: "All" })).toHaveAttribute("aria-pressed", "true");
    await chips.getByRole("button", { name: "attic 1" }).click();
    await expect(page).toHaveURL(/#\/artifacts\?project=attic$/);
    await expect(rows(page)).toHaveCount(1);
    await expect(page.getByRole("toolbar", { name: "Filter artifacts" }).getByRole("button", { name: "attic 1" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await page.reload();
    await expect(rows(page)).toHaveCount(1);
  });

  test("the margin previews the focused row and follows j and k", async ({ hub, page }, testInfo) => {
    test.skip(!isDesktop(testInfo), "the margin is a wide stage's");
    await fixedList(page);
    await openList(page, hub);
    const margin = page.getByRole("complementary", { name: "Focused artifact" });
    await expect(margin.getByRole("heading", { name: "nightly-report.html" })).toBeVisible();

    await page.keyboard.press("j");
    await expect(rows(page).nth(1)).toBeFocused();
    await expect(margin.getByRole("heading", { name: "release-notes.md" })).toBeVisible();
    await expect(margin.getByText("2, none open")).toBeVisible();
    await expect(margin.getByRole("link", { name: "Event 01EVENTRELEASE" })).toBeVisible();
    // Versions newest first, each with its summary.
    await expect(margin.locator(".arts-vers li")).toHaveText([/^v3\s*3 lines changed/, /^v2\s*notes drafted/, /^v1\s*skeleton/]);
    await expect(margin.getByRole("link", { name: /^Open/ })).toHaveAttribute(
      "href",
      "#/projects/checks/artifacts?artifact=release-notes",
    );

    // The path is copied by the glyph on the line that holds it.
    await margin.getByRole("button", { name: "Copy path release-notes.md" }).click();
    await expect(page.getByText("Path copied")).toBeVisible();

    await page.keyboard.press("k");
    await expect(margin.getByRole("heading", { name: "nightly-report.html" })).toBeVisible();
  });

  test("a phone reads the list with no margin", async ({ hub, page }, testInfo) => {
    test.skip(isDesktop(testInfo), "the phone's list");
    await fixedList(page);
    await openList(page, hub);
    await expect(page.getByRole("complementary", { name: "Focused artifact" })).toBeHidden();
    await expect(rows(page).first()).toBeVisible();
  });
});

test.describe("the artifacts list on the hub", () => {
  test("o opens the focused artifact in its project, and reading it takes it out of the first tier", async ({
    hub,
    page,
  }, testInfo) => {
    test.skip(!isDesktop(testInfo), "the keys are a desktop's");
    // A record that began at the epoch: every artifact on the hub is new.
    await page.addInitScript(() => window.localStorage.setItem("hub.artifacts.seen", JSON.stringify({ at: 1, seen: {} })));
    await openList(page, hub);
    const row = page.locator(`.arts-row[data-artifact="${hub.artifactId}"]`);
    await expect(page.locator("section.arts-tier").first().locator(`[data-artifact="${hub.artifactId}"]`)).toBeVisible();
    const name = (await row.locator(".now-open").textContent()).trim();
    await row.locator(".now-open").focus();
    await page.keyboard.press("o");
    await expect(page).toHaveURL(new RegExp(`#/projects/${hub.projectId}/artifacts\\?artifact=${hub.artifactId}`));
    await expect(page.getByRole("heading", { level: 1, name })).toBeVisible();

    await page.getByRole("navigation", { name: "Primary" }).first().getByRole("link", { name: "Artifacts" }).click();
    await expect(page).toHaveURL(/#\/artifacts$/);
    await expect(tier(page, /^Earlier · \d+$/).locator(`[data-artifact="${hub.artifactId}"]`)).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Primary" }).first().getByRole("link", { name: "Artifacts" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  test("a phone reaches the list from More", async ({ hub, page }, testInfo) => {
    test.skip(isDesktop(testInfo), "the phone's way in");
    await page.goto(`${hub.baseUrl}/#/more`);
    await page.locator(".more-screen").getByRole("link", { name: /^Artifacts/ }).click();
    await expect(page).toHaveURL(/#\/artifacts$/);
    await expect(rows(page).first()).toBeVisible();
    await expect(page.locator(".tabbar").getByRole("link", { name: "More" })).toHaveAttribute("aria-current", "page");
  });
});
