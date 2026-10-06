// The frame, chrome, sync and settings invariants: the phone header's two
// heights and its tools row, the desktop panes landing at 52/92, the sync line
// that stays hidden until it has something to say, the token reveal, the
// access copy, the register, the connect screen, and the storage screen.
//
// The checks that measure a desktop width set it themselves and put the phone
// viewport back, which is the shape the old script ran in: one phone run with
// the desktop screens driven where they are the subject.

import { api, expect, goto, homeTitle, settle, test } from "./invariants.mjs";

const headerHeight = (page) =>
  page.evaluate(() => document.querySelector(".shell-head")?.getBoundingClientRect().height ?? null);

const createAgent = async (hub, id, name) => {
  await api(hub, "POST", "/api/v1/agents", { id, display_name: name });
  await api(hub, "POST", `/api/v1/agents/${encodeURIComponent(id)}/token`);
};

test.describe("phone frame", () => {
  test("five tabs, More, the collapsing header, and the tools row", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    const tabs = await page.evaluate(() =>
      [...document.querySelectorAll(".tabbar a .tab-label")].map((el) => el.textContent.trim()),
    );
    expect(tabs, "the tab bar does not have the five expected tabs").toEqual([
      "Home",
      "Inbox",
      "Projects",
      "Search",
      "More",
    ]);

    await goto(page, "#/more", "More");
    expect(
      await page.evaluate(() => document.querySelector('.tabbar a[href="#/more"]')?.getAttribute("aria-current") === "page"),
      "More tab is not current on #/more",
    ).toBe(true);
    const moreRows = await page.evaluate(() =>
      [...document.querySelectorAll(".more-screen .more-row .title")].map((el) => el.textContent.trim()),
    );
    expect(moreRows, "the More screen rows differ from expected").toEqual(["Storage", "Agents and tokens", "Settings"]);

    // Healthy means no sync element anywhere, and no space kept for one.
    const syncVisible = await page.evaluate(() => {
      const out = [];
      for (const sel of [".more-screen .more-sync-line", "#rail-sync"]) {
        const el = document.querySelector(sel);
        if (!el) continue;
        const cs = window.getComputedStyle(el);
        const box = el.getBoundingClientRect();
        if (cs.display !== "none" && !el.hidden && box.height > 0 && box.width > 0) out.push(sel);
      }
      return out;
    });
    expect(syncVisible, "a healthy hub still shows a sync element").toEqual([]);

    await goto(page, "#/settings", "Settings");
    const back = await page.evaluate(() => document.querySelector(".shell-head .shell-back")?.getAttribute("href"));
    expect(back && back.startsWith("#/more"), "Settings back chevron does not return to #/more").toBe(true);
    expect(
      await page.evaluate(() => document.querySelector('.tabbar a[href="#/more"]')?.getAttribute("aria-current") === "page"),
      "More tab does not stay current on Settings",
    ).toBe(true);

    // Header collapse and hysteresis: 76px at rest, 52px past 20px of scroll,
    // still 52 at 15px, back to 76 below 8px.
    await goto(page, "#/inbox", "Inbox");
    await page.evaluate(() => window.scrollTo(0, 0));
    await expect.poll(async () => Math.round((await headerHeight(page)) ?? 0)).toBe(76);
    await page.evaluate(() => window.scrollTo(0, 35));
    await expect.poll(async () => Math.round((await headerHeight(page)) ?? 0)).toBe(52);
    await page.evaluate(() => window.scrollTo(0, 15));
    expect(
      Math.round((await headerHeight(page)) ?? 0),
      "the header expanded at scrollTop=15, inside the hysteresis band",
    ).toBe(52);
    await page.evaluate(() => window.scrollTo(0, 4));
    await expect.poll(async () => Math.round((await headerHeight(page)) ?? 0)).toBe(76);
    await page.evaluate(() => window.scrollTo(0, 0));

    // CHECK 12.A: content y 120 at rest, y 96 after 40px, title glyph at x 48.
    for (const [url, title] of [
      ["#/inbox", "Inbox"],
      ["#/projects", "Projects"],
    ]) {
      await goto(page, url, title);
      await page.evaluate(() => window.scrollTo(0, 0));
      await expect.poll(async () => Math.round((await headerHeight(page)) ?? 0)).toBe(76);
      const titleX = await page.evaluate(() => {
        const el = document.querySelector(".shell-head .shell-title h1, .shell-head .shell-title .shell-title-line");
        return el ? Math.round(el.getBoundingClientRect().left) : null;
      });
      if (titleX !== null) expect(Math.abs(titleX - 48), `${url} title is not at x=48 at rest`).toBeLessThanOrEqual(2);
      const toolsBottom = await page.evaluate(() => {
        const el = document.querySelector(".shell-controls");
        return el ? Math.round(el.getBoundingClientRect().bottom) : null;
      });
      expect(Math.abs((toolsBottom ?? -1) - 120), `${url} tools row is not at y=120 at rest`).toBeLessThanOrEqual(2);
      await page.evaluate(() => window.scrollTo(0, 40));
      await expect.poll(async () => Math.round((await headerHeight(page)) ?? 0)).toBe(52);
      const toolsScrolled = await page.evaluate(() => {
        const el = document.querySelector(".shell-controls");
        return el ? Math.round(el.getBoundingClientRect().bottom) : null;
      });
      expect(Math.abs((toolsScrolled ?? -1) - 96), `${url} tools row is not at y=96 after 40px`).toBeLessThanOrEqual(2);
      await page.evaluate(() => window.scrollTo(0, 0));
    }

    // CHECK 12.B: no text in the 44px tools row occupies two lines at 390/360.
    for (const width of [390, 360]) {
      await page.setViewportSize({ width, height: 800 });
      await goto(page, "#/inbox", "Inbox");
      const wrapped = await page.evaluate(() => {
        const row = document.querySelector(".shell-controls");
        if (!row) return [];
        const bad = [];
        row.querySelectorAll("span, button, a, div, input").forEach((el) => {
          if (el.children.length === 0 && el.textContent.trim()) {
            if (el.getClientRects().length > 1) bad.push(el.textContent.trim());
          }
        });
        return bad;
      });
      expect(wrapped, `the inbox tools row wraps text at ${width}px`).toEqual([]);
    }
    await page.setViewportSize({ width: 390, height: 844 });

    // Home at rest paints no header bar and no title. Being absent is not the
    // rule; being unpainted is.
    await goto(page, "#/home", homeTitle());
    await page.evaluate(() => window.scrollTo(0, 0));
    const homeBarPainted = await page.evaluate(() => {
      const h = document.querySelector(".shell:has(.home-pad) .shell-head");
      if (!h) return false;
      const cs = window.getComputedStyle(h);
      const painted = cs.backgroundColor !== "rgba(0, 0, 0, 0)" || cs.borderBottomWidth !== "0px";
      const title = h.querySelector(".shell-title-line");
      const titleShown = title ? Number(window.getComputedStyle(title).opacity) > 0.05 : false;
      return painted || titleShown;
    });
    expect(homeBarPainted, "Home at rest paints a header bar or its title").toBe(false);
  });
});

test.describe("desktop chrome", () => {
  test("panes land at 52/92 and labels sit on the gutter", async ({ hub, page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/home");
    for (const [route, name] of [
      ["#/settings", "Settings"],
      ["#/storage", "Storage"],
      ["#/access", "Agents and tokens"],
    ]) {
      await goto(page, route);
      await expect(page.locator("main h1").first()).toBeVisible();
      const geom = await page.evaluate(() => {
        const vis = (e) => {
          const cs = getComputedStyle(e);
          return cs.display !== "none" && e.getBoundingClientRect().height > 2;
        };
        const head = [...document.querySelectorAll(".shell-head, .storage-head")].find(vis) || null;
        const ctl = [...document.querySelectorAll(".shell-controls, .storage-controls")].find(vis) || null;
        const row = ctl || head;
        const tall = row
          ? [...row.querySelectorAll("*")]
              .filter((e) => {
                const r = e.getBoundingClientRect();
                return r.height > 32.5 && r.height < 200;
              })
              .map((e) => `${e.tagName}.${(e.className || "").toString().split(" ")[0]}:${Math.round(e.getBoundingClientRect().height)}`)
          : [];
        const idx = document.querySelector(".shell-index");
        const stage = document.querySelector(".shell-stage");
        const labels = [];
        const sel = ".form-group-label, .settings-group-label, .shell-group-label, h2.day, .index-subhead, .section-label";
        for (const e of document.querySelectorAll(sel)) {
          const r = e.getBoundingClientRect();
          if (r.height < 2) continue;
          let textX = null;
          const walker = document.createTreeWalker(e, NodeFilter.SHOW_TEXT);
          const tn = walker.nextNode();
          if (tn) {
            const range = document.createRange();
            range.selectNodeContents(tn);
            const rr = range.getBoundingClientRect();
            if (rr.width > 0) textX = rr.left;
          }
          if (textX === null) textX = r.left + parseFloat(getComputedStyle(e).paddingLeft || "0");
          const pane = stage && r.left >= stage.getBoundingClientRect().left ? stage : idx || stage;
          const off = pane ? Math.round(textX - pane.getBoundingClientRect().left) : null;
          labels.push({ text: e.textContent.trim().slice(0, 20), offset: off });
        }
        return {
          head: head ? Math.round(head.getBoundingClientRect().bottom) : null,
          ctl: ctl ? { top: Math.round(ctl.getBoundingClientRect().top), bottom: Math.round(ctl.getBoundingClientRect().bottom) } : null,
          tall: tall.slice(0, 6),
          labels,
        };
      });
      expect(geom.head, `${name}: header does not land at 52`).toBe(52);
      expect(geom.ctl, `${name}: no control row found`).toBeTruthy();
      expect(geom.ctl.top === 52 && geom.ctl.bottom === 92, `${name}: control row does not span 52..92`).toBe(true);
      expect(geom.tall, `${name}: a control in the 40px row is taller than 32`).toEqual([]);
      for (const label of geom.labels) {
        expect(label.offset, `${name}: label ${JSON.stringify(label.text)} sits at the pane's x 0`).not.toBe(0);
        expect([16, 24], `${name}: label ${JSON.stringify(label.text)} is not on the 16/24 gutter`).toContain(label.offset);
      }
    }
    await page.setViewportSize({ width: 390, height: 844 });
  });

  test("Agents and tokens is a list and item, with reissue reachable at 1440", async ({ hub, page }) => {
    const agent = "desktop-agents-probe";
    await createAgent(hub, agent, "Desktop agents probe");
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/home");
    await goto(page, `#/access?agent=${encodeURIComponent(agent)}`);
    await expect(page.locator(".shell-index")).toBeVisible();
    expect(
      await page.evaluate(() => {
        const row = document.querySelector('.shell-index [aria-current="true"], .shell-index [aria-current="page"]');
        return row ? row.textContent.trim() : null;
      }),
      "no agent row is selected at 1440",
    ).toBeTruthy();
    await expect(page.locator('[data-action="agent-token"]').first()).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
  });
});

test.describe("phone home", () => {
  test("at rest, scrolled, and its quiet copy", async ({ hub, page }) => {
    await goto(page, "#/home", homeTitle());
    await page.evaluate(() => window.scrollTo(0, 0));

    const homeBarPainted = await page.evaluate(() => {
      const h = document.querySelector(".shell:has(.home-pad) .shell-head");
      if (!h) return false;
      const cs = window.getComputedStyle(h);
      return (
        cs.backgroundColor !== "rgba(0, 0, 0, 0)" ||
        cs.borderBottomWidth !== "0px" ||
        (h.querySelector(".shell-title-line") ? Number(window.getComputedStyle(h.querySelector(".shell-title-line")).opacity) > 0.05 : false)
      );
    });
    expect(homeBarPainted, "Home at rest paints a header bar or its title").toBe(false);
    await expect(page.locator(".home-gear"), "Home at rest has a gear button").toHaveCount(0);

    const greetingX = await page.evaluate(() => {
      const el = document.querySelector(".home-welcome h1, .home-greeting, .home-welcome-title");
      return el ? Math.round(el.getBoundingClientRect().left) : null;
    });
    expect(Math.abs((greetingX ?? -1) - 16), "the Home greeting is not at x=16").toBeLessThanOrEqual(2);

    await page.evaluate(() => window.scrollTo(0, 100));
    await expect.poll(async () => Math.round((await headerHeight(page)) ?? 0)).toBe(52);
    const barTitle = await page.evaluate(
      () => document.querySelector(".shell-head .shell-title h1, .shell-head .shell-title .shell-title-line")?.textContent?.trim(),
    );
    const greeting = await page.evaluate(() => document.querySelector(".home-greeting")?.textContent.trim() ?? null);
    expect(barTitle, "the Home scrolled header carries no title").toBeTruthy();
    expect(barTitle === "Home" || (greeting && barTitle !== greeting), "the Home bar is not the greeting").toBe(false);
    const titleX = await page.evaluate(() => {
      const el = document.querySelector(".shell-head .shell-title h1, .shell-head .shell-title .shell-title-line");
      return el ? Math.round(el.getBoundingClientRect().left) : null;
    });
    expect(Math.abs((titleX ?? -1) - 48), "the Home scrolled title is not at x=48").toBeLessThanOrEqual(2);
    const toolsPinned = await page.evaluate(() => {
      const row = document.querySelector(".shell-controls");
      if (!row) return false;
      const rect = row.getBoundingClientRect();
      return Math.abs(rect.top - 52) <= 2 && Math.abs(rect.height - 44) <= 2 && row.querySelectorAll(".chip").length >= 3;
    });
    expect(toolsPinned, "the Home chips are not pinned in the tools row").toBe(true);

    await page.evaluate(() => window.scrollTo(0, 0));
    const summary = await page.evaluate(
      () => document.querySelector(".home-status, .home-summary")?.textContent.trim() ?? "",
    );
    expect(summary, "the Home summary is not a sentence").toContain("waiting on you");
    const statLine = await page.evaluate(() => {
      const text = document.querySelector(".home")?.textContent || "";
      return /\d+\s+unread\s*·|·\s*\d+\s+unread|0 unread · 0 waiting/i.test(text);
    });
    expect(statLine, "Home still renders a bullet stat line").toBe(false);

    const cardAroundEvents = await page.evaluate(
      () =>
        document.querySelectorAll(
          ".home-waiting.card, .home-waiting.home-card, .home-newest .card, .home-newest .home-card, .card .home-row, .home-card .home-row",
        ).length > 0,
    );
    expect(cardAroundEvents, "the Home event list is wrapped in a card").toBe(false);
    const storageIsCard = await page.evaluate(() => {
      const st = document.querySelector(".home-storage");
      return st ? st.classList.contains("card") || st.classList.contains("home-card") : false;
    });
    expect(storageIsCard, "the Home storage summary is not a card").toBe(true);
  });
});

test.describe("sync states", () => {
  // One failure shows nothing; two show the failed line in every place that
  // carries it. The counter is driven through the module's own functions.
  test("hidden when healthy, shown after two failures, at both widths", async ({ hub, page }) => {
    const shown = () =>
      page.evaluate(() => {
        const out = [];
        for (const el of document.querySelectorAll("#rail-sync, .more-sync-line")) {
          const cs = window.getComputedStyle(el);
          if (cs.display === "none") continue;
          const box = el.getBoundingClientRect();
          if (box.height <= 0 || box.width <= 0) continue;
          const text = el.textContent.trim();
          if (el.hidden) {
            out.push(`HIDDEN-BUT-PAINTED:${el.id || el.className}:${text}`);
            continue;
          }
          if (text) out.push(`${el.id || el.className}:${text}`);
        }
        return out;
      });
    for (const [width, label] of [
      [390, "phone"],
      [1440, "desktop"],
    ]) {
      await page.setViewportSize({ width, height: 900 });
      await goto(page, width < 720 ? "#/more" : "#/home");
      await expect(page.locator("main h1").first()).toBeVisible();
      expect(await shown(), `${label}: healthy still shows a sync element`).toEqual([]);
      await page.evaluate(() => window.__sync.noteSyncFailure({ status: 500, kind: "error" }));
      expect(await shown(), `${label}: one failure already shows a sync element`).toEqual([]);
      await page.evaluate(() => window.__sync.noteSyncFailure({ status: 500, kind: "error" }));
      const two = await shown();
      expect(two.length, `${label}: two failures did not show the sync line`).toBeGreaterThan(0);
      expect(two.some((entry) => entry.includes("not synced")), `${label}: two failures did not show a 'not synced' line`).toBe(true);
      await page.evaluate(() => window.__sync.noteSyncSuccess());
      expect(await shown(), `${label}: a success left the sync line showing`).toEqual([]);
    }
    await page.setViewportSize({ width: 390, height: 844 });
  });
});

test.describe("token reveal", () => {
  // The token exists in the DOM only while the dialog is open. The claim is
  // about a real issued string, so this captures it while the dialog is open
  // and looks for that exact string again after Done.
  test("the issued token leaves the DOM when the reveal closes", async ({ hub, page }) => {
    const agent = "reveal-probe";
    await createAgent(hub, agent, "Reveal probe");
    await goto(page, `#/access?agent=${encodeURIComponent(agent)}`);
    await expect(page.locator('[data-action="agent-token"]').first()).toBeVisible();
    await page.locator('[data-action="agent-token"]').first().click();
    const dialog = page.locator("dialog.dialog-reveal[open]");
    await expect(dialog).toBeVisible();
    const state1 = await page.evaluate(() => {
      const d = document.querySelector("dialog.dialog-reveal[open]");
      const f = d.querySelector(".dialog-field-wrap");
      return {
        hidden: !!f && f.hidden,
        buttons: [...d.querySelectorAll(".dialog-actions button")].map((b) => b.textContent.trim()),
      };
    });
    expect(state1.hidden, "state 1 already shows the token field").toBe(true);
    expect(state1.buttons, "state 1 does not offer Reissue").toContain("Reissue");
    await page.evaluate(() =>
      [...document.querySelectorAll("dialog.dialog-reveal .dialog-actions button")]
        .find((b) => b.textContent.trim() === "Reissue")
        .click(),
    );
    await expect
      .poll(async () => (await page.locator(".reveal-token").innerText().catch(() => "")).length, { timeout: 10_000 })
      .toBeGreaterThanOrEqual(20);
    const issued = (await page.locator(".reveal-token").innerText()).trim();
    expect(issued.length, "state 2 did not show an issued token").toBeGreaterThanOrEqual(20);
    await page.keyboard.press("Escape");
    await expect(page.locator("dialog.dialog-reveal[open]"), "Escape closed state 2").toBeVisible();
    await page.evaluate(() =>
      [...document.querySelectorAll("dialog.dialog-reveal .dialog-actions button")]
        .find((b) => b.textContent.trim() === "Done")
        .click(),
    );
    await expect(page.locator("dialog.dialog-reveal[open]"), "Done did not close the reveal").toHaveCount(0);
    await expect
      .poll(() => page.evaluate((token) => document.documentElement.innerHTML.includes(token), issued), {
        timeout: 10_000,
      })
      .toBe(false);
  });
});

test.describe("round 14", () => {
  test("access copy, token states, the share sheet, and the type floor", async ({ hub, page, watch }) => {
    // The type floor walks eleven routes at two widths in both themes, so this
    // check is a survey rather than a single screen and needs the room.
    test.setTimeout(180_000);
    const agent = "round14-probe";
    const confidential = "round14-confidential";
    await createAgent(hub, agent, "Round 14 probe");
    await api(hub, "POST", "/api/v1/projects", { id: confidential, display_name: "Round 14 confidential", confidential: true });
    await api(hub, "POST", `/api/v1/agents/${encodeURIComponent(agent)}/grants`, { project_id: confidential });

    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/home");
    await goto(page, `#/access?agent=${encodeURIComponent(agent)}`);
    await expect(page.locator(".shell")).toBeVisible();
    const screen = await page.locator(".shell").innerText();
    for (const token of ["write on", "read on"]) {
      expect(screen, `the agents screen still renders '${token}' as an access value`).not.toContain(token);
    }

    // Revoke leaves the grants, issue restores access with no new grant.
    const rowsExpr = () =>
      page.evaluate(() => [...document.querySelectorAll(".agent-project-row")].map((row) => row.textContent.trim()));
    await expect(page.locator(".agent-project-row").first(), "the probe agent's project rows did not render").toBeVisible({
      timeout: 8000,
    });
    const rowsBefore = await rowsExpr();
    expect(rowsBefore, "the probe agent's project rows did not render").toBeTruthy();
    await page.click('[data-action="agent-revoke"]');
    await expect(page.locator("dialog .dialog-commit")).toBeVisible();
    await page.click("dialog .dialog-commit");
    await expect(page.locator(".shell-stage")).toContainText("No live token", { timeout: 8000 });
    await expect(page.locator(".shell-stage")).toContainText("Issue token");
    expect(await rowsExpr(), "revoking the token changed the project rows").toEqual(rowsBefore);
    await page.click('[data-action="agent-issue"]');
    await expect(page.locator("dialog .reveal-token")).toBeVisible();
    await page.click("dialog .dialog-commit");
    await expect(page.locator(".shell-stage")).not.toContainText("No live token", { timeout: 8000 });
    expect(await rowsExpr(), "issuing a token changed the project rows").toEqual(rowsBefore);

    // The share sheet's actions follow the artifact.
    const openShare = async (which) => {
      // The share sheet asks the share endpoint, and a 404 there is the "no
      // link yet" answer, so the response watch stands down for the open.
      const restore = watch.disarm();
      try {
        // A fresh document per artifact: the two viewer routes differ only in
        // the fragment, so a same-document hop would keep the previous viewer,
        // its menu and its sheet mounted under the new one. A reload of the
        // artifact's own address is a whole boot rather than a fragment move.
        await page.goto(
          `${hub.baseUrl}/#/artifacts/${encodeURIComponent(which)}?project=${encodeURIComponent(hub.projectId)}`,
        );
        await page.reload();
        // The viewer's own spec waits for the frame before reaching for the
        // overflow; the viewer re-renders as it loads, and a menu opened into a
        // render that is still in flight is closed again under the click.
        await expect(page.locator("#hub-frame")).toBeVisible({ timeout: 8000 });
        // The menu is built before the share sheet it opens, so the sheet being
        // in the DOM is what says the item's handler is wired.
        await expect(page.locator(".hub-share-sheet")).toBeAttached({ timeout: 8000 });
        await expect(page.locator(".hub-more")).toBeVisible({ timeout: 8000 });
        // The viewer re-renders as its frame settles, which closes an open
        // overflow menu under a pointer. The menu's own handler is driven in one
        // page task, so the item the reader's click reaches is the one that
        // opens the sheet; the menu's mechanics are the viewer spec's subject.
        const opened = await page.evaluate(() => {
          const more = document.querySelector(".hub-more");
          const share = document.querySelector('[data-action="share"]');
          if (!more || !share) return false;
          more.click();
          share.click();
          return true;
        });
        expect(opened, `the artifact viewer for ${which} has no overflow control`).toBe(true);
        await expect(page.locator(".hub-share-sheet:not([hidden])")).toBeVisible({ timeout: 8000 });
        await expect(page.locator(".hub-share-primary")).toBeVisible({ timeout: 10_000 });
        const text = await page.locator(".hub-share-sheet:not([hidden])").innerText();
        await page.evaluate(() => {
          for (const backdrop of document.querySelectorAll(".hub-share-backdrop")) backdrop.click();
        });
        await expect(page.locator(".hub-share-sheet:not([hidden])")).toHaveCount(0, { timeout: 5000 });
        return text;
      } finally {
        restore();
      }
    };
    const plain = await openShare(hub.artifactId);
    expect(plain, "the plain artifact's share sheet offers Delete artifact").not.toContain("Delete artifact");
    expect(
      plain.includes("Make link") || plain.includes("Revoke link"),
      "the plain artifact's share sheet offers neither Make link nor Revoke link",
    ).toBe(true);
    const protectedSheet = await openShare(hub.protectedId);
    expect(protectedSheet, `the protected artifact's share sheet offers Revoke link (${JSON.stringify(protectedSheet)})`).not.toContain("Revoke link");
    expect(protectedSheet, `the protected artifact's share sheet reads ${JSON.stringify(protectedSheet)}`).toContain(
      "Delete artifact",
    );

    // No text under the 12px floor, at both widths and both themes.
    const routes = [
      "#/home",
      `#/projects/${encodeURIComponent(hub.projectId)}/feed`,
      `#/projects/${encodeURIComponent(hub.projectId)}/artifacts`,
      `#/projects/${encodeURIComponent(hub.projectId)}/sessions`,
      "#/inbox",
      "#/search",
      "#/settings",
      "#/storage",
      "#/access",
      "#/more",
      "#/projects",
    ];
    for (const [width, height] of [
      [390, 844],
      [1440, 900],
    ]) {
      await page.setViewportSize({ width, height });
      for (const theme of ["light", "dark"]) {
        await page.evaluate((value) => {
          document.documentElement.dataset.theme = value;
        }, theme);
        for (const route of routes) {
          await goto(page, route);
          await expect(page.locator("main h1").first()).toBeVisible();
          const bad = await page.evaluate(() => {
            const out = [];
            const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
            let node;
            while ((node = walker.nextNode())) {
              const text = (node.textContent || "").trim();
              if (!text) continue;
              const el = node.parentElement;
              if (!el || el.closest('[aria-hidden="true"]')) continue;
              const style = getComputedStyle(el);
              if (style.display === "none" || style.visibility === "hidden") continue;
              if (!el.getClientRects().length) continue;
              const size = parseFloat(style.fontSize);
              if (size && size < 12) out.push(`${el.tagName.toLowerCase()} ${size}px ${JSON.stringify(text.slice(0, 24))}`);
            }
            return out.slice(0, 8);
          });
          expect(bad, `under the 12px floor at ${width}/${theme} on ${route}`).toEqual([]);
        }
      }
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page.evaluate(() => {
      delete document.documentElement.dataset.theme;
    });
  });
});

test.describe("register and connect", () => {
  test("the projects register files agent spaces apart from projects", async ({ hub, page }) => {
    const agent = "space-test-agent";
    const created = (await api(hub, "POST", "/api/v1/agents", { id: agent, display_name: "Test Space" })).json();
    const spaceId = created.personal_project_id;
    await goto(page, "#/projects", "Projects");
    await expect(page.locator(".project-row").first()).toBeVisible();
    for (const row of await page.locator(".project-row").all()) {
      const text = (await row.innerText()).toLowerCase();
      expect(text, "a project row repeats the active agent count").not.toContain("agent active");
    }
    const headerMeta = await page.locator(".shell-head .shell-meta").innerText();
    expect(headerMeta, "the header meta is missing the 'count · footprint' shape").toContain("·");
    expect(await page.locator(".projects-storage, .projects-storage-card").count()).toBe(0);
    expect(await page.locator(".projects-agent-spaces, .projects-agent-spaces-summary").count()).toBe(0);

    const tools = page.locator(".shell-controls");
    await expect(tools).toBeVisible();
    const projTab = tools.locator('[role="tab"]:has-text("Projects")');
    const spacesTab = tools.locator('[role="tab"]:has-text("Agent spaces")');
    const registerFilter = tools.locator('[data-action="projects-filter"]');
    const desktop = (await page.viewportSize()).width >= 1100;
    if (desktop) {
      expect(await projTab.count(), "the desktop register still draws the segmented switcher").toBe(0);
      expect(await spacesTab.count(), "the desktop register still draws the segmented switcher").toBe(0);
      expect(await registerFilter.count(), "the desktop register is missing its filter field").toBe(1);
    } else {
      expect(await projTab.count(), "the tools row is missing the Projects tab").toBe(1);
      expect(await spacesTab.count(), "the tools row is missing the Agent spaces tab").toBe(1);
      await spacesTab.first().click();
      await expect(page.locator(`.project-row:visible[data-id="${spaceId}"]`), "Agent spaces did not show the space").toHaveCount(1);
      await projTab.first().click();
      await expect(page.locator(`.project-row:visible[data-id="${spaceId}"]`), "Projects unexpectedly shows agent spaces").toHaveCount(0);
    }
  });

  test("the connect screen has no tab bar, an in-field eye, and an error", async ({ hub, page, watch }) => {
    await goto(page, "#/connect");
    await expect(page.locator("main h1").first()).toBeVisible();
    await expect(page.locator(".tabbar"), "connect shows a tab bar").toBeHidden();
    const tools = page.locator(".shell-controls");
    await expect(tools).toBeVisible();
    expect((await tools.innerText()).toLowerCase(), "the connect tools row does not read 'Not connected'").toContain(
      "not connected",
    );
    expect(await page.locator(".connect.card, main .card:has(.connect-field)").count(), "connect is wrapped in a card").toBe(0);
    const field = page.locator(".connect-field");
    await expect(field).toBeVisible();
    const eye = page.locator('[aria-label="Show token"], .connect-eye-btn');
    await expect(eye.first()).toBeVisible();
    expect(
      await page.evaluate(() => {
        const button = document.querySelector('[aria-label="Show token"], .connect-eye-btn');
        const field = document.querySelector(".connect-field");
        if (!button || !field) return false;
        const container = field.closest(".connect-input-box, .connect-field-wrap");
        return container && container.contains(button);
      }),
      "the show-token control is not inside the field box",
    ).toBe(true);
    expect(await field.getAttribute("type"), "the connect field does not start masked").toBe("password");
    await eye.first().click();
    expect(await field.getAttribute("type"), "the eye did not reveal the token").toBe("text");
    await eye.first().click();
    expect(await field.getAttribute("type"), "the eye did not mask the token again").toBe("password");

    watch.ignore(/answered 401|status of 401|401/);
    await field.fill("wrong-token-xyz");
    await page.locator('button[type="submit"]').click();
    const error = page.locator(".connect-error:not([hidden])");
    await expect(error.first(), "no error appears under the field on an invalid token").toBeVisible({ timeout: 8000 });
    expect(
      await page.evaluate(() => document.activeElement === document.querySelector(".connect-field")),
      "focus did not return to the connect field after an error",
    ).toBe(true);
    // Put the working token back for the checks that follow.
    await page.evaluate((token) => window.localStorage.setItem("hub.token", token), hub.token);
  });
});

test.describe("settings and storage", () => {
  test("Settings has one display path and a copy control with the full path", async ({ hub, page }) => {
    await goto(page, "#/settings", "Settings");
    expect(await page.locator(".shell-controls").count(), "Settings has a tools row").toBe(0);
    const pathNodes = page.locator(".settings-data-path, .settings-footer");
    await expect(pathNodes).toHaveCount(1);
    const text = (await pathNodes.innerText()).trim();
    expect(text, `the settings path is not the last two segments: ${JSON.stringify(text)}`).toMatch(/^…\/[^/]+\/[^/]+$/);
    const copy = page.locator('.settings-copy-path-btn, button[data-action="copy-path"]');
    await expect(copy).toHaveCount(1);
    const full = (await copy.getAttribute("data-path")) || "";
    expect(full.startsWith("/") && !full.startsWith("…"), `the copy control does not hold the full path: ${JSON.stringify(full)}`).toBe(true);
  });

  test("single-key shortcuts are hidden under a coarse pointer and present under a fine one", async ({ hub, browser }) => {
    const probe = async (hasTouch, width) => {
      const context = await browser.newContext({ viewport: { width, height: 844 }, hasTouch });
      await context.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
      const page = await context.newPage();
      try {
        await page.goto(`${hub.baseUrl}/#/settings`, { waitUntil: "load" });
        await expect(page.locator("main h1").first()).toBeVisible();
        return await page.evaluate(() => {
          const el = document.querySelector('#shortcuts, [data-action="toggle-shortcuts"], .settings-row-shortcuts');
          if (!el) return false;
          const style = window.getComputedStyle(el);
          return style.display !== "none" && style.visibility !== "hidden" && el.offsetWidth > 0;
        });
      } finally {
        await context.close();
      }
    };
    expect(await probe(true, 390), "the shortcuts switch is visible under a coarse pointer").toBe(false);
    expect(await probe(false, 1100), "the shortcuts switch is hidden under a fine pointer").toBe(true);
  });

  test("Storage has a four-segment bar and exactly one helper line", async ({ hub, page }) => {
    await goto(page, "#/storage", "Storage");
    const headerMeta = await page.evaluate(() => document.querySelector(".shell-head .shell-meta")?.textContent || "");
    expect(headerMeta, "the storage header meta contains a path").not.toMatch(/[/\\]/);
    const bar = page.locator('.storage-summary [role="img"], .storage-bar[role="img"]');
    await expect(bar).toHaveCount(1);
    await expect(bar.locator(".storage-bar-seg, .storage-seg")).toHaveCount(4);
    const knowledge = bar.locator('[data-kind="knowledge"]');
    await expect(knowledge, "the storage bar is missing the knowledge segment").toHaveCount(1);
    expect(
      await knowledge.evaluate((el) => window.getComputedStyle(el).backgroundColor),
      "the knowledge segment has no background colour",
    ).toBeTruthy();
    const helper = page.locator(".storage-helper-line, .storage-shared, .storage-scale");
    await expect(helper).toHaveCount(1);
    await expect(helper.first()).toContainText("of events is the shared hub database, not in any row below");
  });

  test("Storage rows list only non-zero kinds", async ({ hub, page }) => {
    await goto(page, "#/storage", "Storage");
    await expect(page.locator(".storage-project-meta, .storage-row .storage-detail").first()).toBeVisible();
    expect(
      await page.locator(".storage-project-row .storage-prune, .storage-row .storage-prune").count(),
      "storage project rows carry a per-row Prune button",
    ).toBe(0);
    const lines = await page.locator(".storage-project-meta, .storage-row .storage-detail").allInnerTexts();
    let foundEventsOnly = false;
    for (const text of lines) {
      expect(text, "a project row lists a zero-byte kind").not.toMatch(/ 0 (B|KB)/);
      if (text.trim() === "events only") foundEventsOnly = true;
    }
    expect(foundEventsOnly, "no project row with 'events only' was found").toBe(true);
  });

  test("the admin token never reaches the screen", async ({ hub, page }) => {
    await goto(page, "#/access", "Agents and tokens");
    await expect(page.locator("main")).toBeVisible();
    const prefix = hub.token.slice(0, 6);
    expect(await page.locator("main").innerText(), "the admin token prefix appears in rendered text").not.toContain(prefix);
    expect(
      await page.evaluate((needle) => {
        return [...document.querySelectorAll("*")].some((el) =>
          [...el.attributes].some((attr) => attr.value.includes(needle)),
        );
      }, prefix),
      "an element carries the admin token prefix in an attribute",
    ).toBe(false);
  });
});
