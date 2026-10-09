// The project-owned screens: the tools row, session detail and its brain, the
// artifact list and viewer, the feed's status pill, the wiki tree and reader,
// the section switcher and the search scope chips. This is the slice of the old
// invariants.py whose subject is a screen inside a project.
//
// One old check is dropped rather than ported: `capture_b7_screenshots` wrote
// PNGs of the session document and a resolved quote into `target/tmp` for a
// person to look at. It asserted nothing, so ADR 0023 has no tool for it: a
// visual check is a `toHaveScreenshot` baseline, and this was a capture.

import { api, expect, goto, newSession, oneOffQuestion, settle, structured, test, toolCall } from "./invariants.mjs";

const projectRoute = (hub, segment) => `#/projects/${encodeURIComponent(hub.projectId)}/${segment}`;

test.describe("project tools and sessions", () => {
  test("the project tools row carries the segmented tabs and a 44px filter button", async ({ hub, page }) => {
    await goto(page, projectRoute(hub, "feed"), "Feed");
    const tools = page.locator(".project-tools-mobile, .project-tools-row");
    await expect(tools).toBeVisible();
    for (const label of ["Feed", "Artifacts", "Sessions"]) {
      await expect(tools.locator(`[role="tab"]:has-text("${label}")`), `missing ${label} tab`).toHaveCount(1);
    }
    const filter = tools.locator('.project-filter-btn, [aria-label*="Filter and group"]');
    await expect(filter.first()).toBeVisible();
    const box = await filter.first().boundingBox();
    expect(box.width >= 44 && box.height >= 44, `the filter button is ${box.width}x${box.height}, not 44x44`).toBe(true);
  });

  test("session detail renders exactly one title", async ({ hub, page }) => {
    await goto(page, `#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(hub.sessionId)}`, hub.fixture.sessionName);
    expect(await page.locator(".shell-head .shell-title-line").count(), "session detail has no bar title").toBeGreaterThan(0);
    expect(
      await page.locator(".session-detail-view h1, .session-detail-header h1, .session-detail-title-block h1").count(),
      "session detail renders a second title inside the detail view",
    ).toBe(0);
  });

  test("Reassign moves the session owner", async ({ hub, page }) => {
    const target = "reassign-probe";
    await api(hub, "POST", "/api/v1/agents", { id: target, display_name: "Reassign probe" });
    const session = await newSession(hub);
    const started = await toolCall(hub, session, "session_start", {
      project_id: hub.projectId,
      session_name: "reassign-check",
    });
    const sessionId = structured(started).session_id || "";
    expect(sessionId, "the reassign check could not start a session to move").toBeTruthy();

    await goto(page, `#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(sessionId)}`);
    await expect(page.locator('[data-action="reassign-open"]')).toBeVisible();
    const ownerBefore = await page.locator(".session-detail-header .meta[data-owner]").getAttribute("data-owner");
    expect(ownerBefore, "the probe session already belongs to the target agent").not.toBe(target);
    await page.click('[data-action="reassign-open"]');
    const item = `[data-action="reassign"][data-agent="${target}"]`;
    await expect(page.locator(item), "the Reassign control offered no agent to move to").toBeVisible({ timeout: 5000 });
    await page.click(item);
    const dialog = page.locator("dialog.dialog[open]");
    await expect(dialog, "choosing an agent opened no confirmation dialog").toBeVisible({ timeout: 5000 });
    expect(await dialog.innerText(), "the dialog does not name the agent it moves to").toContain(target);
    await dialog.locator(".dialog-commit").click();
    await expect(page.locator(".session-detail-header .meta[data-owner]")).toHaveAttribute("data-owner", target, {
      timeout: 8000,
    });
  });

  test("a kv key is read in the aside and not in the stage", async ({ hub, page }) => {
    await goto(
      page,
      `#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(hub.sessionId)}&file=%2Fkv%2Flast-run`,
      hub.fixture.sessionName,
    );
    // The aside is the surface the fine pointer reads, which is the guarantee:
    // the CSS keeps it out of the phone layout, so the attribute is what says
    // the app chose the aside rather than the coarse-pointer sheet.
    const aside = page.locator(".shell-aside");
    await expect(aside).toHaveCount(1);
    expect(await aside.getAttribute("hidden"), "the aside is hidden for a kv key").toBeNull();
    await expect(aside).toContainText(hub.fixture.brainValue);
    await expect(aside.locator('[data-action="copy-kv-value"]')).toHaveCount(1);
    await expect(page.locator(".shell-stage"), "the stage unexpectedly carries the kv value").not.toContainText(
      hub.fixture.brainValue,
    );
    const leaf = page.locator('.tree-item[data-path="/kv/last-run"]');
    if (await leaf.count()) {
      expect(await leaf.locator(".tree-file-chev, .tree-chev").count(), "a kv tree row carries a chevron").toBe(0);
    }
  });

  test("an fs file opens rendered in the stage, with provenance and a back control", async ({ hub, page }) => {
    await goto(
      page,
      `#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(hub.sessionId)}&file=%2Ffs%2Fcontext.md`,
      "context.md",
    );
    const stage = page.locator(".shell-stage");
    await expect(stage.locator(".session-doc-content h1, h1:has-text('notes')")).toHaveCount(1);
    const text = await stage.innerText();
    expect(text, "the stage does not show the brain provenance line").toContain("session brain");
    expect(text, "the stage does not show the read-only provenance").toContain("read-only");
    await expect(stage.locator(".shell-back")).toHaveCount(1);
    const aside = page.locator(".shell-aside");
    if ((await aside.count()) && !(await aside.getAttribute("hidden"))) {
      expect(await aside.innerText(), "the fs file is rendered in the aside instead of the stage").not.toContain("notes");
    }
    expect(
      await stage.locator('[data-action="comments-toggle"], [data-action="stage-version"], [data-action="share"]').count(),
      "the fs file stage has comments, version or share controls",
    ).toBe(0);
  });

  test("a missing entry and a directory path do not throw and say so in words", async ({ hub, page, watch }) => {
    // A missing entry is a 404 and a directory is a 409: both are the answers
    // this check is about rather than defects.
    watch.ignore(/brain\/entry/);
    await page.evaluate((route) => {
      location.hash = route;
    }, `#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(hub.sessionId)}&file=%2Fkv%2Fmissing-key`);
    const aside = page.locator(".shell-aside");
    await expect(aside).toHaveCount(1);
    expect(await aside.getAttribute("hidden"), "the aside is hidden for a non-existent path").toBeNull();
    await expect(aside, "the aside does not state the missing entry in words").toContainText(/not found|missing/i);
    await page.evaluate((route) => {
      location.hash = route;
    }, `#/projects/${encodeURIComponent(hub.projectId)}/sessions?id=${encodeURIComponent(hub.sessionId)}&file=%2Ffs%2Fnotes`);
    await expect(aside).toHaveCount(1);
    expect(await aside.getAttribute("hidden"), "the aside is hidden for a directory path").toBeNull();
    await expect(aside, "the aside does not state the directory in words").toContainText(/directory|folder/i);
  });
});

test.describe("artifacts", () => {
  test("grouping by agent uses the actor with an honest fallback", async ({ hub, page }) => {
    await goto(page, projectRoute(hub, "artifacts"), "Artifacts");
    const filter = page.locator('[data-action="project-filter-toggle"]');
    if ((await filter.count()) && (await filter.first().isVisible())) {
      await filter.first().click();
    }
    const group = page.locator("[data-group-toggle]");
    await expect(group, "the artifacts screen has no group menu toggle").toHaveCount(1);
    await group.click();
    const option = page.locator('[data-action="artifact-group"][data-group="agent"]');
    await expect(option, "the group menu does not offer Agent").toHaveCount(1);
    await option.click();
    const headers = page.locator(".shell-group-label, .section-label");
    await expect(headers.first()).toBeVisible();
    const texts = await headers.allInnerTexts();
    expect(texts.length, "no group headers after grouping by agent").toBeGreaterThan(0);
    expect(texts.filter((text) => text.trim() === "" || text.trim().startsWith("·")), "a blank group header").toEqual([]);
  });

  test("an artifact row shows the thread count excluding deleted comments", async ({ hub, page }) => {
    const artifact = hub.artifactId;
    const comment = async (body) =>
      (await api(hub, "POST", `/api/v1/artifacts/${artifact}/comments`, { author: "human", body })).json();
    const reload = async () => {
      await goto(page, "#/projects");
      await goto(page, projectRoute(hub, "artifacts"), "Artifacts");
      return page.locator(`.artifact-row[data-id="${artifact}"]`);
    };
    await comment("first comment");
    let row = await reload();
    await expect(row, "the artifact row is not on screen").toBeVisible();
    await expect(row, "the row does not show one comment").toContainText("1 comment");
    const second = await comment("second comment");
    row = await reload();
    await expect(row, "the row does not show two comments").toContainText("2 comments");
    if (second && second.id) {
      await api(hub, "DELETE", `/api/v1/artifacts/${artifact}/comments/${second.id}`);
    }
    row = await reload();
    await expect(row, "the row does not show one comment after the delete").toContainText("1 comment");
  });
});

test.describe("inbox and search on a phone", () => {
  test("the inbox tools row, flat rows, and the unread mark", async ({ hub, page }) => {
    const session = await newSession(hub);
    await toolCall(hub, session, "signal_append", {
      project_id: hub.projectId,
      kind: "finished",
      summary: "phone unread row check",
    });
    await goto(page, "#/inbox", "Inbox");
    const tools = page.locator(".shell-controls");
    await expect(tools.locator("[data-index-filter]")).toHaveCount(1);
    await expect(tools.locator('.chip:has-text("Unread")')).toHaveCount(1);
    expect(await tools.locator(".inbox-sync").count(), "the inbox tools row carries a sync line").toBe(0);
    expect(await page.locator(".inbox-rows.card, .inbox-group.card, .shell-index .card").count(), "the inbox list is a boxed card").toBe(0);

    const unread = page.locator(".inbox-row.is-unread").first();
    await expect(unread, "no unread row was found in the inbox").toBeVisible();
    await expect(unread.locator(".dot-unread"), "the unread row has no unread dot").toHaveCount(1);
    const weight = await unread.locator(".title").evaluate((el) => window.getComputedStyle(el).fontWeight);
    expect(["600", "bold"], `the unread title weight is ${weight}`).toContain(weight);
  });

  test("the search scope chips, the hit mark, and flat rows", async ({ hub, page }) => {
    await goto(page, `#/search?q=${encodeURIComponent(hub.fixture.searchTerm)}`, "Search");
    for (const width of [390, 360]) {
      await page.setViewportSize({ width, height: 800 });
      const wrapped = await page.evaluate(() => {
        const row = document.querySelector(".shell-controls");
        if (!row) return [];
        const bad = [];
        row.querySelectorAll("span, button, a, div, input").forEach((el) => {
          if (el.children.length === 0 && el.textContent.trim() && el.getClientRects().length > 1) bad.push(el.textContent.trim());
        });
        return bad;
      });
      expect(wrapped, `the search tools row wraps text at ${width}px`).toEqual([]);
    }
    await page.setViewportSize({ width: 390, height: 844 });
    const row = page.locator(".search-row").first();
    await expect(row, "no search row was found").toBeVisible();
    const mark = row.locator(".title mark");
    await expect(mark, "the search match is not inside a mark in the title").toHaveCount(1);
    expect((await mark.first().innerText()).toLowerCase(), "the mark does not contain the search term").toContain(
      hub.fixture.searchTerm.toLowerCase(),
    );
    const rowText = (await row.innerText()).toLowerCase();
    expect(
      rowText.split(hub.fixture.searchTerm.toLowerCase()).length - 1,
      "the search hit renders the term more than once",
    ).toBe(1);
    expect(await row.locator(".search-snippet").count(), "the search hit carries a snippet echoing the title").toBe(0);
    expect(await page.locator(".search-group.card, .search-results .card, .shell-index .card").count(), "the search list is a boxed card").toBe(0);
  });

  test("the feed status pill stays on one line", async ({ hub, page }) => {
    const event = await oneOffQuestion(hub, hub.projectId, "Rotate the staging certificate on the edge host");
    expect(event, "the seeded question never reached the feed").toBeTruthy();
    const row = `main .feed-row[data-id="${event}"]`;
    const pillRows = () =>
      page.evaluate(() => {
        const r2 = (n) => Math.round(n * 100) / 100;
        const lineBoxes = (el) => {
          const range = document.createRange();
          let lines = 0;
          for (const node of el.childNodes) {
            if (node.nodeType !== 3 || !node.textContent.trim()) continue;
            range.selectNodeContents(node);
            lines += range.getClientRects().length;
          }
          return lines;
        };
        return [...document.querySelectorAll(".feed-row")]
          .filter((row) => row.querySelector(".pill-status"))
          .map((row) => {
            const pill = row.querySelector(".pill-status");
            const title = row.querySelector(".title");
            const line = pill.parentElement;
            return {
              id: row.dataset.id || "",
              title: (title.textContent || "").replace(/\s+/g, " ").trim(),
              pillText: (pill.textContent || "").replace(/\s+/g, " ").trim(),
              height: r2(pill.getBoundingClientRect().height),
              clientHeight: pill.clientHeight,
              scrollHeight: pill.scrollHeight,
              textLines: lineBoxes(pill),
              scrollWidth: pill.scrollWidth,
              clientWidth: pill.clientWidth,
              whiteSpace: getComputedStyle(pill).whiteSpace,
              titleScroll: title.scrollWidth,
              titleClient: title.clientWidth,
              titleTextOverflow: getComputedStyle(title).textOverflow,
              lineOverflow: r2(line.scrollWidth - line.clientWidth),
              titleBottom: r2(title.getBoundingClientRect().bottom),
              pillTop: r2(pill.getBoundingClientRect().top),
            };
          });
      });
    try {
      for (const [width, height] of [
        [1440, 900],
        [390, 844],
      ]) {
        await page.setViewportSize({ width, height });
        await goto(page, "#/settings", "Settings");
        await goto(page, projectRoute(hub, "feed"));
        await expect(page.locator(row), `the seeded row is not in the feed at ${width}px`).toBeVisible({ timeout: 8000 });
        for (const measured of await pillRows()) {
          const what = `at ${width}px the row ${JSON.stringify(measured.title)} carries a pill ${JSON.stringify(measured.pillText)} that`;
          expect(measured.textLines, `${what} runs on ${measured.textLines} lines`).toBe(1);
          expect(measured.scrollHeight <= measured.clientHeight, `${what} spills out of its box`).toBe(true);
          expect(Math.round(measured.height), `${what} is taller than its single-line box`).toBe(Math.round(measured.clientHeight));
          expect(measured.scrollWidth <= measured.clientWidth, `${what} overflows its own width`).toBe(true);
          expect(measured.lineOverflow <= 0.5, `${what} pushed its line out`).toBe(true);
        }
        const theRow = (await pillRows()).find((entry) => entry.id === event);
        expect(theRow, `at ${width}px the seeded row draws no waiting pill`).toBeTruthy();
        expect(
          theRow.pillTop >= theRow.titleBottom - 0.5,
          `at ${width}px the row's pill shares the title's line`,
        ).toBe(true);
      }
    } finally {
      await api(hub, "POST", `/api/v1/questions/${event}/answer`, { body: "answered by the check" }).catch(() => {});
      await page.setViewportSize({ width: 390, height: 844 });
    }
  });

  test("a wiki search hit reads its display path", async ({ hub, page }) => {
    const term = "marmoset";
    const base = `/api/v1/projects/${encodeURIComponent(hub.projectId)}/kb/pages`;
    await api(hub, "PUT", `${base}/runbooks/${term}-untitled.md`, {
      content: `---\ntype: note\n---\n# Field\n\nA ${term} page with no title of its own.\n`,
    });
    await api(hub, "PUT", `${base}/runbooks/${term}-titled.md`, {
      content: `---\ntitle: Deploy the ${term}\ntype: note\n---\n# Field\n\nA ${term} page.\n`,
    });
    const searchRows = () =>
      page.evaluate(() =>
        [...document.querySelectorAll(".search-row")].map((row) => {
          const link = row.querySelector(".search-link");
          return {
            title: ((row.querySelector(".title") || {}).textContent || "").trim(),
            href: link ? link.getAttribute("href") : null,
          };
        }),
      );
    try {
      for (const [width, height] of [
        [1440, 900],
        [390, 844],
      ]) {
        await page.setViewportSize({ width, height });
        await goto(page, "#/settings", "Settings");
        await goto(page, `#/search?q=${term}`, "Search");
        await expect(page.locator(".search-row").first(), `the search for ${term} drew no row at ${width}px`).toBeVisible({
          timeout: 8000,
        });
        const rows = await searchRows();
        const titles = rows.map((row) => row.title);
        for (const title of titles) {
          expect(!title.includes("/fs/") && !title.startsWith("/"), `a wiki hit shows its stored path ${JSON.stringify(title)}`).toBe(true);
        }
        expect(titles, `no hit reads the display path runbooks/${term}-untitled.md`).toContain(`runbooks/${term}-untitled.md`);
        expect(titles, `a wiki hit with a title of its own lost it`).toContain(`Deploy the ${term}`);
        if (width === 390) continue;
        const target = `runbooks/${term}-untitled.md`;
        await page.locator(".search-row", { hasText: target }).first().click();
        const preview = await page.evaluate(() => ({
          path: ((document.querySelector(".search-stage-path") || {}).textContent || "").trim(),
          title: ((document.querySelector(".search-preview-title") || {}).textContent || "").trim(),
        }));
        expect(preview.path, "the search preview path shows the stored path").not.toContain("/fs/");
        expect(preview.title, "the search preview title is not the display path").toBe(target);
      }
    } finally {
      await page.setViewportSize({ width: 390, height: 844 });
    }
  });

  test("the open inbox card's content sits on the 16px gutter", async ({ hub, page }) => {
    const question = await oneOffQuestion(hub, hub.projectId, "gutter check question");
    expect(question, "the gutter probe question never reached the inbox").toBeTruthy();
    try {
      await goto(page, `#/inbox?open=${encodeURIComponent(question)}`, "Inbox");
      await expect(page.locator(".inbox-detail #inbox-detail-title")).toBeVisible({ timeout: 8000 });
      const probes = await page.evaluate(() => {
        const textX = (el) => {
          const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
          const tn = walker.nextNode();
          if (tn) {
            const range = document.createRange();
            range.selectNodeContents(tn);
            const r = range.getBoundingClientRect();
            if (r.width > 0) return r.left;
          }
          return el.getBoundingClientRect().left;
        };
        const probe = (sel) => {
          const el = document.querySelector(sel);
          if (!el || !el.getClientRects().length) return { sel, missing: true };
          return { sel, x: Math.round(textX(el) * 100) / 100 };
        };
        return [".inbox-back", "#inbox-detail-title", ".inbox-detail-meta"].map(probe);
      });
      for (const probe of probes) {
        expect(probe.missing, `inbox card: ${probe.sel} is not on screen`).toBeFalsy();
        expect(probe.x, `inbox card: ${probe.sel} is not on the 16px gutter`).toBe(16);
      }
    } finally {
      await api(hub, "POST", `/api/v1/questions/${question}/answer`, { body: "answered by the check" }).catch(() => {});
      await goto(page, "#/inbox", "Inbox");
    }
  });
});

test.describe("artifact viewer chrome", () => {
  test("the version sheet's 12px text clears 4.5:1 in both themes", async ({ hub, page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    const paintedContrast = (selector) =>
      page.evaluate((sel) => {
        const ch = (v) => {
          const s = v / 255;
          return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
        };
        const lum = (c) => 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2]);
        const parse = (value) => {
          const m = String(value).match(/rgba?\(([^)]+)\)/);
          if (!m) return null;
          const p = m[1].split(",").map((v) => parseFloat(v));
          return [p[0], p[1], p[2], p[3] === undefined ? 1 : p[3]];
        };
        const over = (fg, bg) => fg.slice(0, 3).map((c, i) => c * fg[3] + bg[i] * (1 - fg[3]));
        const el = document.querySelector(sel);
        if (!el || !el.getClientRects().length) return { selector: sel, missing: true };
        let ground = null;
        let node = el;
        while (node && node !== document.documentElement) {
          const c = parse(getComputedStyle(node).backgroundColor);
          if (c && c[3] > 0) {
            if (ground === null) ground = c.slice(0, 3);
            else if (c[3] < 1) ground = over(c, ground);
            if (c[3] === 1) break;
          }
          node = node.parentElement;
        }
        if (ground === null) ground = [255, 255, 255];
        const fg = over(parse(getComputedStyle(el).color), ground);
        const hi = Math.max(lum(fg), lum(ground));
        const lo = Math.min(lum(fg), lum(ground));
        return { selector: sel, size: getComputedStyle(el).fontSize, ratio: Math.round(((hi + 0.05) / (lo + 0.05)) * 100) / 100 };
      }, selector);
    await goto(page, `#/artifacts/${encodeURIComponent(hub.artifactId)}?project=${encodeURIComponent(hub.projectId)}`);
    await expect(page.locator('[data-action="version-toggle"]')).toBeVisible({ timeout: 8000 });
    await page.click('[data-action="version-toggle"]');
    await expect(page.locator(".hub-version-row.current")).toBeVisible();
    for (const theme of ["dark", "light"]) {
      await page.evaluate((value) => document.documentElement.setAttribute("data-theme", value), theme);
      for (const selector of [
        ".hub-version-row.current .hub-version-secondary",
        ".hub-version-row.current .hub-version-size",
        ".hub-version-sheet-footer",
      ]) {
        const got = await paintedContrast(selector);
        expect(got.missing, `version sheet ${theme}: ${selector} is not on screen`).toBeFalsy();
        expect(got.ratio, `version sheet ${theme}: ${selector} is ${got.ratio}:1 at ${got.size}`).toBeGreaterThanOrEqual(4.5);
      }
    }
    await page.evaluate(() => {
      const b = document.querySelector("#hub-version-backdrop");
      if (b) b.click();
    });
    await page.setViewportSize({ width: 390, height: 844 });
  });

  test("the artifact's stage header is a 52px band with one control height", async ({ hub, page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, `#/artifacts/${encodeURIComponent(hub.artifactId)}?project=${encodeURIComponent(hub.projectId)}`);
    await expect(page.locator('.shell.has-selection[data-segment="artifacts"] #hub-frame')).toBeVisible({ timeout: 8000 });
    const got = await page.evaluate(() => {
      const head = document.querySelector(".shell-stage > .shell-head");
      if (!head || !head.getClientRects().length) return { missing: true };
      const heights = [...head.querySelectorAll("button")]
        .filter((el) => el.getClientRects().length)
        .map((el) => ({ cls: el.className.split(" ")[0], h: Math.round(el.getBoundingClientRect().height) }));
      const add = document.querySelector('.shell-aside [aria-label="New thread"]');
      return {
        band: Math.round(head.getBoundingClientRect().height),
        heights,
        thread:
          add && add.getClientRects().length
            ? { w: Math.round(add.getBoundingClientRect().width), h: Math.round(add.getBoundingClientRect().height) }
            : null,
      };
    });
    expect(got.missing, "the artifact's stage header is not on screen").toBeFalsy();
    expect(got.band, `the stage header is ${got.band}px, not 52`).toBe(52);
    expect([...new Set(got.heights.map((h) => h.h))].length, `the header mixes control heights: ${JSON.stringify(got.heights)}`).toBeLessThanOrEqual(1);
    expect(got.thread, "the comments aside has no thread control on screen").toBeTruthy();
    expect(got.thread.w === got.thread.h && got.thread.w === 32, `the thread control is ${got.thread.w}x${got.thread.h}, not 32x32`).toBe(true);
    await page.setViewportSize({ width: 390, height: 844 });
  });
});

test.describe("wiki", () => {
  test("the tree, the reader, the editor, comments, changes and lint", async ({ hub, page }) => {
    const project = hub.projectId;
    const content = "---\ntype: runbook\nstatus: standard\n---\n# Deploy\n\nRun the release from the checkout.\n";
    await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(project)}/kb/pages/runbooks/deploy.md`, { content });
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, projectRoute(hub, "wiki"));
    await expect(page.locator(".wiki-row").first()).toBeVisible();

    await goto(page, projectRoute(hub, "wiki") + "?page=runbooks%2Fdeploy.md");
    await expect(page.locator(".wiki-page")).toBeVisible();
    const body = await page.locator(".wiki-page").innerText();
    expect(body, "the reader did not keep the frontmatter out of the body").toContain("Deploy");
    expect(body, "the reader kept the frontmatter in the body").not.toContain("type:");

    await goto(page, projectRoute(hub, "wiki") + "?page=runbooks%2Fdeploy.md&edit=1");
    await expect(page.locator("#wiki-content")).toBeVisible();
    await page.fill("#wiki-content", "---\ntype: runbook\nstatus: standard\n---\n# Deploy\n\nEdited by the check.\n");
    await page.click('[data-action="wiki-save"]');
    await expect(page.locator(".wiki-page")).toContainText("Edited by the check.", { timeout: 8000 });

    const comments = page.locator(".wiki-comments");
    await comments.getByRole("textbox", { name: "Comment" }).fill("A comment from the check.");
    await comments.getByRole("button", { name: "Post", exact: true }).click();
    await expect(comments).toContainText("A comment from the check.", { timeout: 8000 });
    await expect(page.locator('[data-action="wiki-comment-resolve"]')).toHaveCount(1);
    await expect(page.locator('[data-action="wiki-review"]')).toHaveCount(1);
    await page.click('[data-action="wiki-review"]');

    await goto(page, projectRoute(hub, "wiki") + "?view=changes");
    await expect(page.locator(".wiki-change").first()).toBeVisible({ timeout: 8000 });
    await goto(page, projectRoute(hub, "wiki") + "?view=lint");
    await expect(page.locator(".wiki-finding").first()).toBeVisible({ timeout: 8000 });
    await page.setViewportSize({ width: 390, height: 844 });
  });

  test("the phone wiki index shows no trail at the root and a trail inside a directory", async ({ hub, page }) => {
    const project = hub.projectId;
    await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(project)}/kb/pages/notes/field.md`, {
      content: "---\ntype: note\n---\n# Field\n\nA page under a directory.\n",
    });
    const shape = () =>
      page.evaluate(() => {
        const crumbs = [...document.querySelectorAll(".shell-index .wiki-breadcrumb")];
        return {
          crumbs: crumbs.length,
          firstCrumb: crumbs.length ? (crumbs[0].querySelector("a") || {}).textContent || "" : null,
          tree: !!document.querySelector(".shell-index .wiki-tree"),
          rows: document.querySelectorAll(".shell-index .wiki-row").length,
        };
      });
    await goto(page, "#/settings", "Settings");
    await goto(page, projectRoute(hub, "wiki"), "Wiki");
    await expect(page.locator(".shell-index .wiki-row").first()).toBeVisible({ timeout: 8000 });
    const root = await shape();
    expect(root.crumbs, "the phone wiki index draws a breadcrumb at its root").toBe(0);
    expect(root.tree && root.rows > 0, "the phone wiki index rendered no tree at its root").toBe(true);

    await goto(page, "#/settings", "Settings");
    await goto(page, projectRoute(hub, "wiki") + "?dir=notes", "Wiki");
    await expect(page.locator(".shell-index .wiki-breadcrumb").first()).toBeVisible({ timeout: 8000 });
    const inside = await shape();
    expect(inside.crumbs, "the phone wiki index inside a directory has no crumb").toBeGreaterThanOrEqual(1);
    expect(inside.firstCrumb, "the phone wiki trail does not start at Wiki").toBe("Wiki");
    expect(inside.rows, "the phone wiki directory rendered no rows").toBeGreaterThan(0);
  });

  test("a wiki directory row never addresses a page", async ({ hub, page }) => {
    const project = hub.projectId;
    await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(project)}/kb/pages/notes/field.md`, {
      content: "---\ntype: note\n---\n# Field\n\nA page under a directory.\n",
    });
    const dirRows = () =>
      page.evaluate(() => [...document.querySelectorAll(".shell-index .wiki-dir")].map((a) => a.getAttribute("href")));
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/settings", "Settings");
    await goto(page, projectRoute(hub, "wiki"), "Wiki");
    await expect(page.locator(".shell-index .wiki-dir").first()).toBeVisible({ timeout: 8000 });
    for (const href of await dirRows()) {
      expect((href || "").includes("page="), `a desktop wiki directory row addresses a page (${href})`).toBe(false);
      expect(!href || href.includes("dir="), `a desktop wiki directory row has an unexpected address (${href})`).toBe(true);
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await goto(page, "#/settings", "Settings");
    await goto(page, projectRoute(hub, "wiki"), "Wiki");
    if (await page.locator(".shell-index .wiki-dir").count()) {
      for (const href of await dirRows()) {
        expect((href || "").includes("page="), `a phone wiki directory row addresses a page (${href})`).toBe(false);
        expect(href && href.includes("dir="), `a phone wiki directory row has no directory address (${href})`).toBe(true);
      }
    }
  });

  test("a wiki tree row's meta line stays inside the index pane", async ({ hub, page }) => {
    const project = hub.projectId;
    await api(hub, "PUT", `/api/v1/projects/${encodeURIComponent(project)}/kb/pages/notes/agent.md`, {
      content: "---\ntype: Overview\nstatus: draft\ntitle: Agent notes\n---\n# Agent notes\n\nA page the check reads.\n",
    });
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/settings", "Settings");
    await goto(page, projectRoute(hub, "wiki"), "Wiki");
    await expect(page.locator(".shell-index .wiki-row").first()).toBeVisible({ timeout: 8000 });
    const got = await page.evaluate(() => {
      const pane = document.querySelector(".shell-index");
      if (!pane) return { error: "the wiki index drew no pane" };
      const paneBox = pane.getBoundingClientRect();
      return {
        paneRight: Math.round(paneBox.right * 10) / 10,
        rows: [...pane.querySelectorAll(".wiki-row")].map((row) => {
          const box = row.getBoundingClientRect();
          const meta = row.querySelector("span.mono");
          const metaBox = meta ? meta.getBoundingClientRect() : null;
          return {
            name: (row.querySelector("span") || {}).textContent?.trim().slice(0, 30) || "",
            scrollWidth: row.scrollWidth,
            clientWidth: row.clientWidth,
            right: Math.round(box.right * 10) / 10,
            pastPane: box.right > paneBox.right + 0.5,
            metaRight: metaBox ? Math.round(metaBox.right * 10) / 10 : null,
            metaOverflows: meta ? meta.scrollWidth > meta.clientWidth : false,
            metaText: meta ? (meta.textContent || "").trim() : null,
          };
        }),
      };
    });
    expect(got.error, got.error).toBeUndefined();
    let metaSeen = false;
    for (const row of got.rows) {
      expect(row.scrollWidth <= row.clientWidth, `a wiki tree row scrolls horizontally (${row.name})`).toBe(true);
      expect(row.pastPane, `a wiki tree row runs past the pane's edge (${row.name})`).toBe(false);
      expect(row.metaRight === null || row.metaRight <= got.paneRight + 0.5, `a wiki meta line runs past the pane (${row.metaText})`).toBe(true);
      if (row.metaText && row.metaText.includes("·") && row.metaOverflows) metaSeen = true;
    }
    expect(metaSeen, "no wiki meta line was long enough to need an ellipsis").toBe(true);
    await page.setViewportSize({ width: 390, height: 844 });
  });
});

test.describe("index pane geometry", () => {
  test("the section switcher fits the index pane it is in", async ({ hub, page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/settings", "Settings");
    await goto(page, projectRoute(hub, "wiki"), "Wiki");
    const probe = () =>
      page.evaluate(() => {
        const head = document.querySelector(".shell-head.project-seg-head");
        const seg = head && head.querySelector(".shell-seg");
        if (!seg) return { error: "the index header has no .shell-seg" };
        const box = seg.getBoundingClientRect();
        const wrap = head.querySelector(".proj-overflow-wrap");
        const shown = (el) => el.getClientRects().length > 0;
        const counts = [...seg.querySelectorAll(".shell-seg-count")];
        const tabs = [...seg.querySelectorAll("a")].map((a) => {
          const r = a.getBoundingClientRect();
          return {
            text: (a.textContent || "").trim(),
            right: Math.round(r.right * 10) / 10,
            truncated: a.scrollWidth > a.clientWidth + 1,
            pastScrollEdge: r.right > box.right + 1,
          };
        });
        return {
          paneWidth: Math.round(document.querySelector(".shell-index").getBoundingClientRect().width),
          segRight: Math.round(box.right * 10) / 10,
          scrollWidth: seg.scrollWidth,
          clientWidth: seg.clientWidth,
          overflows: seg.scrollWidth > seg.clientWidth + 1,
          overflowLeft: wrap ? Math.round(wrap.getBoundingClientRect().left * 10) / 10 : null,
          countsShown: counts.filter(shown).length,
          countsDrawn: counts.length,
          tabCount: tabs.length,
          tabs,
        };
      });
    for (const [width, wantCounts] of [
      [300, false],
      [480, true],
    ]) {
      await page.evaluate((value) => document.querySelector(".shell").style.setProperty("--w-index", `${value}px`), width);
      await expect(page.locator(".shell-head.project-seg-head .shell-seg")).toBeVisible();
      const got = await probe();
      expect(got.error, got.error).toBeUndefined();
      expect(got.paneWidth, `the index pane measured ${got.paneWidth}px, expected ${width}px`).toBe(width);
      expect(got.tabCount, "the switcher did not draw four tabs").toBe(4);
      expect(got.overflows, "the switcher clips its last tab").toBe(false);
      for (const tab of got.tabs) {
        expect(tab.truncated, `at ${width}px the ${JSON.stringify(tab.text)} tab is truncated`).toBe(false);
        expect(tab.pastScrollEdge, `at ${width}px the ${JSON.stringify(tab.text)} tab runs past the switcher edge`).toBe(false);
        expect(tab.right, `at ${width}px the ${JSON.stringify(tab.text)} tab reaches over the overflow button`).toBeLessThanOrEqual(got.overflowLeft + 0.5);
      }
      expect(got.segRight, "the switcher touches the overflow button").toBeLessThan(got.overflowLeft);
      if (wantCounts) expect(got.countsShown, `the switcher shows ${got.countsShown} of ${got.countsDrawn} counts`).toBe(got.countsDrawn);
      else expect(got.countsShown, `the switcher still shows ${got.countsShown} counts`).toBe(0);
    }
    await page.evaluate(() => document.querySelector(".shell").style.removeProperty("--w-index"));
    await page.setViewportSize({ width: 390, height: 844 });
  });

  test("every search scope chip is reachable at the default index width", async ({ hub, page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await goto(page, "#/settings", "Settings");
    await goto(page, `#/search?q=${encodeURIComponent(hub.fixture.searchTerm)}`);
    await expect(page.locator(".search-scopes button[data-scope]").first()).toBeVisible({ timeout: 8000 });
    const probe = () =>
      page.evaluate(() => {
        const pane = document.querySelector(".shell-index");
        const strip = document.querySelector(".search-scopes");
        if (!pane || !strip) return { error: "the search index drew no scope strip" };
        const paneBox = pane.getBoundingClientRect();
        const stripBox = strip.getBoundingClientRect();
        const band = strip.closest(".shell-controls");
        const bandBox = band ? band.getBoundingClientRect() : null;
        return {
          paneWidth: Math.round(paneBox.width),
          paneRight: Math.round(paneBox.right * 10) / 10,
          scrollWidth: strip.scrollWidth,
          clientWidth: strip.clientWidth,
          scrolls: strip.scrollWidth > strip.clientWidth + 1,
          stripTop: Math.round(stripBox.top * 10) / 10,
          stripBottom: Math.round(stripBox.bottom * 10) / 10,
          bandTop: bandBox ? Math.round(bandBox.top * 10) / 10 : null,
          bandBottom: bandBox ? Math.round(bandBox.bottom * 10) / 10 : null,
          chips: [...strip.querySelectorAll("button[data-scope]")].map((chip) => {
            const box = chip.getBoundingClientRect();
            const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
            return {
              label: (chip.textContent || "").trim(),
              right: Math.round(box.right * 10) / 10,
              pastPane: box.right > paneBox.right + 0.5,
              truncated: chip.scrollWidth > chip.clientWidth + 1,
              hitIsChip: !!(hit && (hit === chip || chip.contains(hit))),
              hit: hit ? `${hit.tagName.toLowerCase()}.${String(hit.className || "").split(/\s+/)[0]}` : null,
            };
          }),
        };
      });
    for (const width of [300, 260]) {
      await page.evaluate((value) => document.querySelector(".shell").style.setProperty("--w-index", `${value}px`), width);
      const got = await probe();
      expect(got.error, got.error).toBeUndefined();
      expect(got.paneWidth, `the index pane measured ${got.paneWidth}px, expected ${width}px`).toBe(width);
      for (const chip of got.chips) {
        expect(chip.truncated, `at ${width}px the ${JSON.stringify(chip.label)} chip is truncated`).toBe(false);
        if (width === 300) {
          expect(chip.pastPane, `at ${width}px the ${JSON.stringify(chip.label)} chip runs past the pane's edge`).toBe(false);
          expect(chip.hitIsChip, `at ${width}px a pointer on the ${JSON.stringify(chip.label)} chip hits ${chip.hit}`).toBe(true);
        }
        if (!chip.hitIsChip && width > 260) {
          expect(chip.hitIsChip, `at ${width}px the ${JSON.stringify(chip.label)} chip cannot be hit (${chip.hit})`).toBe(true);
        }
      }
      if (width === 300) expect(got.scrolls, "the scope strip has to scroll at the default width").toBe(false);
      if (got.bandTop !== null) {
        expect(got.stripTop >= got.bandTop - 0.5 && got.stripBottom <= got.bandBottom + 0.5, `the scope strip runs outside the control band at ${width}px`).toBe(true);
      }
    }
    await page.evaluate(() => document.querySelector(".shell").style.removeProperty("--w-index"));
    await page.setViewportSize({ width: 390, height: 844 });
  });
});
