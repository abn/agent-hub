// A copy glyph confirms on itself. It turns into the ok-tone check with the
// label "Copied" for a moment and no toast says so. A write that fails leaves
// the glyph alone and the toast says nothing was copied.

import { expect } from "@playwright/test";
import { test } from "./app.mjs";
import { api, newSession, structured, toolCall } from "./invariants.mjs";

async function artifact(hub) {
  const session = await newSession(hub);
  const published = await toolCall(hub, session, "artifact_publish", {
    project_id: hub.projectId,
    title: "Copy check",
    kind: "markdown",
    content: "# Copy check\n\nBody.\n",
  });
  return structured(published).artifact_id;
}

const open = async (page, hub, id) => {
  await page.goto(`${hub.baseUrl}/#/artifacts/${encodeURIComponent(id)}?project=${encodeURIComponent(hub.projectId)}`);
  const glyph = page.locator('.shell-controls button.hub-btn-glyph[data-action="copy-path"]');
  await expect(glyph).toBeVisible();
  return glyph;
};

test.describe("a copy glyph confirms on itself", () => {
  for (const theme of ["light", "dark"]) {
  test(`a granted write turns the glyph into Copied in ${theme}, with no toast`, async ({ hub, page, context }) => {
    await page.emulateMedia({ colorScheme: theme });
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const id = await artifact(hub);
    await page.clock.install();
    const glyph = await open(page, hub, id);
    // From here the page's timers move only when the test says so.
    await page.clock.pauseAt(new Date(Date.now() + 60_000));
    const before = await glyph.getAttribute("aria-label");
    const idleShape = await glyph.locator("svg").innerHTML();
    // The announcer is in the page before the first copy, empty.
    await expect(page.locator("#copy-status")).toHaveAttribute("aria-live", "polite");
    await expect(page.locator("#copy-status")).toHaveText("");
    await glyph.click();
    await expect(glyph).toHaveAttribute("aria-label", "Copied");
    await page.clock.runFor(50);
    // The check icon: the resolve glyph's tick path, not the copy outline.
    const checkShape = await glyph.locator("svg").innerHTML();
    expect(checkShape).not.toBe(idleShape);
    await expect(glyph.locator("svg path")).toHaveCount(1);
    await expect(glyph.locator("svg path")).toHaveAttribute("d", /^M\s*5\s+12\s*l\s*5\s+5\s+9\s*-\s*9$/);
    await expect(glyph).toHaveClass(/is-copied/);
    await expect(glyph.locator("svg")).toBeVisible();
    await expect(page.locator("#copy-status")).toHaveText("Copied");
    await expect(page.locator(".toast-region .toast, .toast")).toHaveCount(0);
    await glyph.hover();
    const ok = await page.evaluate(() => {
      const probe = document.createElement("i");
      probe.style.color = "var(--ok)";
      document.body.append(probe);
      const c = getComputedStyle(probe).color;
      probe.remove();
      return c;
    });
    expect(await glyph.evaluate((el) => getComputedStyle(el).color)).toBe(ok);
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBeTruthy();
    await page.clock.runFor(1250);
    await expect(glyph).toHaveAttribute("aria-label", "Copied");
    await page.clock.runFor(150);
    await expect(glyph).toHaveAttribute("aria-label", before);
    await expect(glyph).not.toHaveClass(/is-copied/);
    expect(await glyph.locator("svg").innerHTML()).toBe(idleShape);
  });
  }

  test("a refused write leaves the glyph alone and toasts that nothing was copied", async ({ hub, page }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText: () => Promise.reject(new DOMException("denied", "NotAllowedError")) },
        configurable: true,
      });
    });
    const id = await artifact(hub);
    const glyph = await open(page, hub, id);
    const before = await glyph.getAttribute("aria-label");
    await glyph.click();
    await expect(page.getByText("Nothing copied.")).toBeVisible();
    await expect(glyph).toHaveAttribute("aria-label", before);
    await expect(glyph).not.toHaveClass(/is-copied/);
  });
});
