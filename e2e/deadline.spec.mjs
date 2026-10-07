// A waiting item the asking agent gave a deadline says, in words on its row
// and on its card, what the deadline will do and how long is left. The harness
// seeds the second approval with a three hour deadline and no outcome, so it
// declines by default.

import { expect } from "@playwright/test";
import { BODY, open, test } from "./app.mjs";

const WORDS = /Declines itself in \d+h/;

test("a waiting approval with a deadline says what it will do", async ({ hub, page }) => {
  const { secondApproval } = hub.fixture;
  await open(page, hub, "#/inbox", "inbox");
  const name = page.getByRole("link", { name: secondApproval });
  const row = page.locator(BODY).locator(".inbox-row", { has: name });
  await expect(row).toContainText(WORDS);
  const link = row.getByRole("link", { name: secondApproval });

  await link.click();
  await expect(page.getByRole("article")).toContainText(WORDS);
});
