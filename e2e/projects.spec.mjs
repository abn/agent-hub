// The Projects register shows what its header counts. A desktop lists the
// agent spaces under their own label beside the projects; a phone keeps them
// behind the segmented switcher and, once the register is long, carries a
// filter in its tools row.

import { api, expect, goto, test } from "./invariants.mjs";

const isPhone = (testInfo) => testInfo.project.name === "phone";

async function seedProjects(hub, tag, total) {
  const agent = `reg-${tag}-agent`;
  const made = (await api(hub, "POST", "/api/v1/members", { id: agent, display_name: `Reg ${tag} space` })).json();
  for (let i = 0; i < total; i++) {
    await api(hub, "POST", "/api/v1/projects", { id: `reg-${tag}-${i}`, display_name: `Reg ${tag} ${i}` });
  }
  return made.personal_project_id;
}

// A register of exactly `total` regular projects and `spaces` agent spaces,
// whatever else the shared hub holds.
async function routeRegister(page, total, spaces = 0) {
  const projects = Array.from({ length: total }, (_, i) => ({
    id: `fx-${i}`,
    display_name: `Fixture ${i}`,
    is_personal: i >= total - spaces,
  }));
  await page.route(/\/api\/v1\/projects$/, (route) => route.fulfill({ json: { projects } }));
  await page.route(/\/api\/v1\/projects\/fx-\d+\/stats$/, (route) =>
    route.fulfill({ json: { artifacts: 0, sessions_live: 0 } }),
  );
}

test.describe("the projects register", () => {
  test("a desktop lists agent spaces under a label and counts what it shows", async ({ hub, page }, testInfo) => {
    test.skip(isPhone(testInfo), "the phone splits the lists behind a switcher");
    const spaceId = await seedProjects(hub, "desk", 1);
    await goto(page, "#/projects", "Projects");
    await expect(page.locator(".projects-section-label")).toHaveText("Agent spaces");
    await expect(page.locator(`.project-row:visible[data-id="${spaceId}"]`)).toHaveCount(1);
    const rows = await page.locator(".project-row:visible").count();
    const meta = await page.locator(".shell-head .shell-meta").innerText();
    expect(Number.parseInt(meta, 10), "the header count differs from the rows shown").toBe(rows);
    await expect(page.locator(".projects-tabs-track"), "a desktop draws the phone switcher").toHaveCount(0);
  });

  test("a desktop carries the filter field only past eight rows", async ({ hub, page }, testInfo) => {
    test.skip(isPhone(testInfo), "the desktop control row");
    const field = page.getByRole("searchbox", { name: "Filter projects" });
    await routeRegister(page, 8);
    await goto(page, "#/projects", "Projects");
    await expect(page.locator(".project-row:visible")).toHaveCount(8);
    await expect(field).toHaveCount(0);
    await page.unrouteAll();
    await routeRegister(page, 9);
    await page.reload();
    await expect(page.locator(".project-row:visible")).toHaveCount(9);
    await expect(field).toBeVisible();
    await field.fill("Fixture 3");
    await expect(page.locator(".project-row:visible")).toHaveCount(1);
  });

  test("a phone with many projects carries a filter that searches both lists", async ({ hub, page }, testInfo) => {
    test.skip(!isPhone(testInfo), "the phone-only filter control");
    const spaceId = await seedProjects(hub, "ph", 9);
    await goto(page, "#/projects", "Projects");
    const toggle = page.getByRole("button", { name: "Filter projects" });
    await expect(toggle).toBeVisible();
    await toggle.click();
    const field = page.getByRole("searchbox", { name: "Filter projects" });
    await expect(field).toBeFocused();
    await expect(page.locator(".projects-tabs-track")).toHaveCount(0);
    await field.fill("Reg ph space");
    await expect(page.locator(`.project-row:visible[data-id="${spaceId}"]`), "the filter skipped agent spaces").toHaveCount(1);
    await expect(page.locator(".project-row:visible")).toHaveCount(1);
    await field.press("Escape");
    await expect(page.locator(".projects-tabs-track")).toBeVisible();
    await expect(page.locator(`.project-row:visible[data-id="${spaceId}"]`)).toHaveCount(0);
    await expect(toggle).toBeFocused();
  });

  test("the phone filter opens from the keyboard and matches rows from both lists", async ({ page, hub }, testInfo) => {
    test.skip(!isPhone(testInfo), "the phone-only filter control");
    await routeRegister(page, 10, 2);
    await goto(page, "#/projects", "Projects");
    const toggle = page.getByRole("button", { name: "Filter projects" });
    await toggle.focus();
    await page.keyboard.press("Enter");
    const field = page.getByRole("searchbox", { name: "Filter projects" });
    await expect(field).toBeFocused();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    // Fixtures 8 and 9 are agent spaces, so one query spans both lists.
    await field.fill("Fixture");
    await expect(page.locator(".project-row:visible")).toHaveCount(10);
    await field.fill("Fixture 9");
    await expect(page.locator(".project-row:visible")).toHaveCount(1);
    await field.fill("Fixture 1");
    await expect(page.locator(".project-row:visible")).toHaveCount(1);
    await field.fill("Fixture 8");
    await expect(page.locator(".project-row:visible")).toHaveCount(1);
    await page.keyboard.press("Escape");
    await expect(toggle).toBeFocused();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  test("closing the filter restores an empty selected tab", async ({ page, hub }, testInfo) => {
    test.skip(!isPhone(testInfo), "the phone-only filter control");
    await routeRegister(page, 9, 0);
    await goto(page, "#/projects", "Projects");
    await page.getByRole("tab", { name: /Agent spaces/ }).click();
    await expect(page.getByText("No agent spaces yet")).toBeVisible();
    const toggle = page.getByRole("button", { name: "Filter projects" });
    await toggle.click();
    const field = page.getByRole("searchbox", { name: "Filter projects" });
    await field.fill("Fixture 2");
    await expect(page.locator(".project-row:visible")).toHaveCount(1);
    await field.press("Escape");
    await expect(page.getByText("No agent spaces yet")).toBeVisible();
    await expect(page.locator(".project-row:visible")).toHaveCount(0);
    await expect(page.getByRole("tab", { name: /Agent spaces/ })).toHaveAttribute("aria-selected", "true");
  });

  test("crossing the breakpoint draws the register for the new width", async ({ page, hub }) => {
    await routeRegister(page, 3, 1);
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/projects", "Projects");
    await expect(page.locator(".projects-section-label")).toHaveText("Agent spaces");
    await expect(page.locator(".projects-tabs-track")).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator(".projects-tabs-track")).toBeVisible();
    await expect(page.locator(".projects-section-label")).toHaveCount(0);
    await expect(page.locator(".project-row:visible")).toHaveCount(2);
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect(page.locator(".projects-section-label")).toHaveText("Agent spaces");
    await expect(page.locator(".project-row:visible")).toHaveCount(3);
    await expect(page.locator(".projects-tabs-track")).toHaveCount(0);
  });

  test("a register address with a query also redraws on crossing the breakpoint", async ({ page, hub }) => {
    await routeRegister(page, 3, 1);
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/projects?from=link", "Projects");
    await expect(page.locator(".projects-section-label")).toHaveText("Agent spaces");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator(".projects-tabs-track")).toBeVisible();
    await expect(page.locator(".projects-section-label")).toHaveCount(0);
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect(page.locator(".projects-section-label")).toHaveText("Agent spaces");
    await expect(page.locator(".projects-tabs-track")).toHaveCount(0);
  });
});
