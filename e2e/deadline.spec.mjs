// An ask the asking agent gave a deadline says, in words in its receipts on Now
// and on its card, what the deadline will do and how long is left. The harness
// seeds the second approval with a three hour deadline and no outcome, so it
// declines by default.

import { expect } from "@playwright/test";
import { open, test } from "./app.mjs";

const WORDS = /Declines itself in \d+h/;

test("a waiting approval with a deadline says what it will do", async ({ hub, page }) => {
  const { secondApproval } = hub.fixture;
  await open(page, hub, "#/inbox", "");
  const row = page.locator(".now-ask", { hasText: secondApproval });
  await expect(row).toContainText(WORDS);

  await row.getByRole("link", { name: "Read" }).click();
  await expect(page.getByRole("article")).toContainText(WORDS);
});
