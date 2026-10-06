// The behavioural invariants that do not depend on screen composition: hidden
// means hidden, request counts, text staying text, race conditions, and the
// problem a refusal carries. This is the first slice of the old
// `.agents/scripts/invariants.py` moved onto the standard runner.
//
// Each check asserts a rendered value or a behaviour, never a substring of a
// source file, and every wait is a web-first assertion or a wait on a page
// condition rather than a fixed number of milliseconds. The browser's own
// errors are held by the shared `watch` fixture, so a check that expects a
// refusal names it with `watch.ignore` instead of disarming a global flag.

import {
  LONG_SUMMARY,
  api,
  expect,
  goto,
  heading,
  homeEvent,
  homeTitle,
  longSummaryEvent,
  oneOffEvent,
  oneOffQuestion,
  quietHomePayload,
  searchParams,
  settle,
  stillShowing,
  test,
} from "./invariants.mjs";

test.describe("hidden means hidden", () => {
  // Anything carrying the hidden attribute and still occupying the screen is a
  // defect on every screen: an author `display` beat the browser's own rule.
  test("nothing the app has hidden is still on screen", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    for (const route of ["#/home", "#/inbox", "#/projects", "#/search", "#/storage", "#/settings"]) {
      await goto(page, route);
      expect(
        await settle(page, "!!document.querySelector('main h1')"),
        `${route} did not settle for the hidden sweep`,
      ).toBe(true);
      expect(
        await stillShowing(page),
        `${route} draws elements it has marked hidden`,
      ).toEqual([]);
    }
  });
});

test.describe("request counts", () => {
  // Home holds the payload the badge wants, so it is one request, not two.
  test("Home asks for its payload once", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    let homeRequests = 0;
    page.on("request", (request) => {
      if (request.url().endsWith("/api/v1/home")) homeRequests += 1;
    });
    await goto(page, "#/storage", "Storage");
    homeRequests = 0;
    await goto(page, "#/home");
    await expect(page.locator(".home-pad").first()).toBeVisible();
    expect(homeRequests, "painting Home asked for it more than once").toBe(1);
  });

  // Nothing waiting and nothing new is the quiet state, not an empty list.
  test("a quiet Home is one request and its own shape", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    const seen = [homeEvent(1, 12), homeEvent(2, 40)];
    const payload = quietHomePayload({ recent: seen, last_event_at: seen[0].created_at });
    const home = /\/api\/v1\/home$/;
    await page.route(home, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(payload) }),
    );
    try {
      await goto(page, "#/settings", "Settings");
      const calls = [];
      page.on("request", (request) => {
        if (request.url().includes("/api/v1/") && !request.url().includes("/api/v1/stream")) {
          calls.push(`${request.method()} ${new URL(request.url()).pathname}`);
        }
      });
      await goto(page, "#/home");
      await expect(page.locator(".home-pad").first()).toBeVisible();
      expect(calls, "painting a quiet Home made more than the one request").toEqual(["GET /api/v1/home"]);
    } finally {
      await page.unroute(home).catch(() => {});
    }
  });

  // Storage draws from one request, not one per project. It used to ask
  // `/stats` for every project before it could append the table.
  test("Storage asks once, not once per project", async ({ hub, browser }) => {
    const context = await browser.newContext({ viewport: { width: 1100, height: 844 }, colorScheme: "light" });
    await context.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
    const page = await context.newPage();
    const seen = [];
    page.on("request", (request) => seen.push(request.url()));
    try {
      await page.goto(`${hub.baseUrl}/#/storage`, { waitUntil: "load" });
      expect(
        await settle(page, "!!document.querySelector('main h1')"),
        "storage never painted for the request count",
      ).toBe(true);
      await expect(page.locator(".storage-summary, .storage-row").first()).toBeVisible();
      const stats = seen.filter((url) => url.includes("/stats"));
      expect(stats, "storage made per-project stats requests before drawing").toEqual([]);
    } finally {
      await context.close();
    }
  });
});

test.describe("text stays text", () => {
  // One shared helper escapes every screen, so its loss must not pass quietly.
  test("an agent's markup reaches Home as text", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    await goto(page, "#/storage", "Storage");
    await goto(page, "#/home");
    await expect(page.locator("main")).toContainText(hub.fixture.markup);
    expect(
      await page.evaluate(() => !!document.getElementById("pwned")),
      "an agent's markup became an element",
    ).toBe(false);
  });

  // Esc closes the card, never a half-written answer, and never another screen.
  test("Esc closes an inbox card without losing a draft", async ({ hub, page }) => {
    const subject = "escape check question";
    const item = await oneOffQuestion(hub, hub.projectId, subject);
    expect(item, "the seeded question never reached the inbox").toBeTruthy();
    const row = `main .inbox-item[data-id="${item}"]`;
    const field = "main textarea, main .composer-field";
    const draft = "half an answer";
    try {
      await goto(page, "#/settings", "Settings");
      await goto(page, "#/inbox", "Inbox");
      await expect(page.locator(row)).toBeVisible();
      await page.locator(`${row} a`).click();
      await expect(page.locator(field).first()).toBeVisible();
      await page.locator(field).first().fill(draft);
      await page.locator(field).first().press("Escape");
      expect(await page.locator(field).first().inputValue()).toBe(draft);
      await page.locator(field).first().press("Tab");
      await page.keyboard.press("Escape");
      expect(await page.locator(field).first().inputValue()).toBe(draft);
      await page.locator(field).first().fill("");
      await page.locator(field).first().press("Escape");
      await expect(page.locator(field)).toHaveCount(0);
    } finally {
      await api(hub, "POST", `/api/v1/questions/${item}/answer`, { body: "closed by the check" }).catch(() => {});
      await goto(page, "#/inbox", "Inbox");
    }
  });

  // Neither what an agent wrote nor what the reader typed becomes markup. The
  // highlight is the one place a snippet is cut up and reassembled.
  test("a hostile search query and a hostile snippet stay text", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    await goto(page, "#/search", "Search");
    await expect(page.locator("#q")).toBeVisible();
    const probe = () =>
      page.evaluate(() => ({
        made:
          !!document.getElementById("pwned") ||
          !!window.__searchPwned ||
          !!document.querySelector("main img, main b"),
        error: !!document.querySelector("main .error"),
        field: !!document.getElementById("q"),
        text: (document.querySelector("main") || {}).textContent || "",
      }));
    for (const query of hub.fixture.searchHostile) {
      await page.fill("#q", "");
      await page.fill("#q", query);
      await expect
        .poll(async () => (await probe()).text.includes(hub.fixture.markup), { timeout: 5000 })
        .toBe(true);
      const found = await probe();
      expect(found.made, `markup in the query ${JSON.stringify(query)} became an element`).toBe(false);
      expect(found.error || !found.field, "the query broke the screen").toBe(false);
      expect(await page.inputValue("#q"), "the field does not show the query as typed").toBe(query);
    }
  });

  // A refused request keeps the problem's status and code, not only its words.
  test("a refused request keeps its status and code", async ({ hub, page, watch }) => {
    await page.goto(`${hub.baseUrl}/`);
    watch.ignore(/no-such-session/);
    const seen = await page.evaluate(() =>
      import("/api.mjs")
        .then((module) => module.api("/api/v1/sessions/no-such-session"))
        .then(
          () => null,
          (error) => ({ status: error.status, code: error.code, message: error.message }),
        ),
    );
    expect(seen, "an unknown session did not refuse").toBeTruthy();
    expect(seen.status, "the refusal status").toBe(404);
    expect(seen.code, "the refusal code").toBeTruthy();
  });

  // Markdown rendering and resolved comment quotes must sanitize untrusted HTML
  // and dangerous URL schemes.
  test("markdown and a resolved quote sanitize untrusted HTML", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    const result = await page.evaluate(async () => {
      const { renderMarkdown } = await import("/sessions.mjs");
      const malicious =
        '# Notes\n<script id="pwned-script">alert(1)</script>' +
        '<iframe src="https://example.com" id="pwned-iframe"></iframe>' +
        '<form id="pwned-form"></form>' +
        '<a href="javascript:alert(1)" id="pwned-js-link">click</a>' +
        '<a href="data:text/html,evil" id="pwned-data-link">data</a>' +
        '<a href="https://example.com" id="safe-link">safe</a>';
      const rendered = await renderMarkdown(malicious);
      const container = document.createElement("div");
      container.innerHTML = rendered;

      const { renderDesktopCard } = await import("/comments.mjs");
      const card = renderDesktopCard({
        id: 9999,
        done: true,
        body: "quote test",
        author: "agent",
        created_at: new Date().toISOString(),
        anchor: { mode: "text", quote: '<b id="pwned-quote">malicious</b>' },
      });
      const quoteElement = card.querySelector("#pwned-quote");
      const quoteText = card.querySelector(".hub-card-resolved-text")?.textContent || "";
      const jsLink = container.querySelector("#pwned-js-link, a[href^='javascript:']");
      const dataLink = container.querySelector("#pwned-data-link, a[href^='data:']");
      const safeLink = container.querySelector("#safe-link");
      return {
        hasScript: !!container.querySelector("#pwned-script, script"),
        hasIframe: !!container.querySelector("#pwned-iframe, iframe"),
        hasForm: !!container.querySelector("#pwned-form, form"),
        hasJsScheme: !!jsLink && (jsLink.getAttribute("href") || "").toLowerCase().startsWith("javascript:"),
        hasDataScheme: !!dataLink && (dataLink.getAttribute("href") || "").toLowerCase().startsWith("data:"),
        hasSafeLink: !!safeLink && safeLink.getAttribute("href") === "https://example.com",
        hasInjectedQuoteElement: !!quoteElement,
        quoteTextContainsQuote: quoteText.includes("malicious"),
      };
    });
    expect(result.hasScript, "renderMarkdown leaves <script> in rendered DOM").toBe(false);
    expect(result.hasIframe, "renderMarkdown leaves <iframe> in rendered DOM").toBe(false);
    expect(result.hasForm, "renderMarkdown leaves <form> in rendered DOM").toBe(false);
    expect(result.hasJsScheme, "renderMarkdown permits javascript: link scheme").toBe(false);
    expect(result.hasDataScheme, "renderMarkdown permits data: link scheme").toBe(false);
    expect(result.hasInjectedQuoteElement, "a resolved comment quote became an element").toBe(false);
    expect(result.quoteTextContainsQuote, "the resolved quote text is missing").toBe(true);
  });
});

test.describe("races", () => {
  // The router dispatches known routes, handles unknown routes, and tracks
  // history.
  test("the router falls back and tracks history", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    await page.evaluate(() => {
      location.hash = "#/nonexistent-route-xyz";
    });
    // The desktop Home header names the screen ("Home"); on a phone it is the
    // greeting. Either is the Home screen, so the fallback accepts both.
    await expect(page.locator(".home-pad")).toBeVisible({ timeout: 8000 });
    await goto(page, "#/inbox", "Inbox");
    await goto(page, "#/storage", "Storage");
    await page.goBack();
    expect(
      await settle(
        page,
        "location.hash.startsWith('#/inbox') && document.querySelector('main h1')?.textContent === 'Inbox'",
      ),
      "router history Back did not return to Inbox",
    ).toBe(true);
    await page.goForward();
    expect(
      await settle(
        page,
        "location.hash.startsWith('#/storage') && document.querySelector('main h1')?.textContent === 'Storage'",
      ),
      "router history Forward did not return to Storage",
    ).toBe(true);
  });

  // The screen you left must not paint over the screen you are on. One request
  // of each screen is held in the page, the reader moves on, the next screen
  // paints, and only then is the held answer let go. The list is checked
  // against the router's own registered table, so a new screen fails here until
  // it is held.
  test("the screen you left does not paint over the screen you are on", async ({ hub, page }) => {
    const project = hub.projectId;
    const session = hub.sessionId;
    const artifact = hub.artifactId;
    const at = `#/projects/${encodeURIComponent(project)}`;
    const base = `/api/v1/projects/${encodeURIComponent(project)}`;
    const holds = {
      home: [["home", "#/home", "/api/v1/home"]],
      inbox: [["inbox", "#/inbox", "/api/v1/inbox?status=action"]],
      projects: [
        ["project feed", `${at}/feed`, `${base}/feed?`],
        ["project artifacts", `${at}/artifacts`, `${base}/artifacts`],
        ["project sessions", `${at}/sessions`, "/api/v1/sessions?project="],
        ["project settings", `${at}/settings`, base],
      ],
      feed: [["the bare feed address", "#/feed", "/api/v1/projects"]],
      sessions: [["the bare sessions address", "#/sessions", "/api/v1/projects"]],
      artifacts: [
        ["the bare artifacts address", "#/artifacts", "/api/v1/projects"],
        [
          "artifact viewer",
          `#/artifacts/${encodeURIComponent(artifact)}`,
          `/api/v1/artifacts/${encodeURIComponent(artifact)}/versions`,
        ],
      ],
      session: [
        [
          "session detail",
          `#/session?project=${encodeURIComponent(project)}&id=${encodeURIComponent(session)}`,
          `/api/v1/sessions/${encodeURIComponent(session)}/brain?path=%2Ffs`,
        ],
      ],
      search: [["search", `#/search?q=${encodeURIComponent(hub.fixture.searchTerm)}`, "/api/v1/search?"]],
      storage: [["storage", "#/storage", "/api/v1/storage"]],
      settings: [["settings", "#/settings", "/api/v1/agents"]],
      access: [["access", "#/access", "/api/v1/agents"]],
      more: [["more", "#/more", "/api/v1/storage"]],
      connect: [["connect", "#/connect?next=%2Fstorage", "/api/v1/home"]],
    };

    await page.goto(`${hub.baseUrl}/`);
    const registered = await page.evaluate(() => window.__router.screens());
    const held = Object.keys(holds);
    expect(
      registered.filter((name) => !held.includes(name)),
      "the router registers screens the render guard does not hold",
    ).toEqual([]);
    expect(
      held.filter((name) => !registered.includes(name)),
      "the render guard holds screens the router does not register",
    ).toEqual([]);

    // A real function rather than a string expression: the page's CSP refuses
    // the string form, and the argument only reaches a function.
    const holdFetch = (needle) => {
      const send = window.fetch;
      let go;
      const gate = new Promise((open) => {
        go = open;
      });
      const held = {
        asked: 0,
        answered: 0,
        release: go,
        restore: () => {
          window.fetch = send;
        },
      };
      window.__held = held;
      window.fetch = (url, options) => {
        if (!String(url).includes(needle)) return send(url, options);
        held.asked += 1;
        return gate.then(() => send(url, options)).finally(() => {
          held.answered += 1;
        });
      };
    };

    for (const [screen, entries] of Object.entries(holds)) {
      for (const [name, hash, needle] of entries) {
        const [nextHash, nextTitle] = screen === "search" ? ["#/storage", "Storage"] : ["#/search", "Search"];
        if (screen === "home") await goto(page, "#/storage", "Storage");
        else await goto(page, "#/home", homeTitle());
        await page.evaluate(holdFetch, needle);
        try {
          await page.evaluate((value) => {
            location.hash = value;
          }, hash);
          expect(
            await settle(page, "window.__held.asked > 0"),
            `leaving ${name}: the screen never asked for ${needle}, so nothing was held`,
          ).toBe(true);
          await goto(page, nextHash, nextTitle);
          expect(await heading(page), `leaving ${name}: the next screen never painted`).toBe(nextTitle);
          await page.evaluate(() => window.__held.release());
          expect(
            await settle(page, "window.__held.answered >= window.__held.asked"),
            `leaving ${name}: the held request was never answered`,
          ).toBe(true);
          // Let the late answer run its paint, then hold the route and heading.
          await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
          expect(
            await heading(page),
            `leaving ${name}: the screen left behind painted over ${nextHash}`,
          ).toBe(nextTitle);
          expect(
            await page.evaluate(() => location.hash),
            `leaving ${name}: the screen left behind took the route back`,
          ).toBe(nextHash);
        } finally {
          await page.evaluate(() => {
            if (window.__held) {
              window.__held.release();
              window.__held.restore();
            }
          });
        }
      }
    }
  });

  // An answer that arrives late must not paint over the newer query's.
  test("a late search answer does not paint over the newer query's", async ({ hub, page }) => {
    await page.goto(`${hub.baseUrl}/`);
    await goto(page, "#/search", "Search");
    await expect(page.locator("#q")).toBeVisible();
    const slow = new RegExp(`/api/v1/search\\?q=${hub.fixture.searchTerm}\\b`);
    let release;
    const gate = new Promise((open) => {
      release = open;
    });
    let asked = false;
    await page.route(slow, async (route) => {
      asked = true;
      await gate;
      await route.continue();
    });
    const late = page.waitForResponse(slow, { timeout: 10_000 });
    try {
      await page.evaluate(
        ([first, second]) => {
          const q = document.getElementById("q");
          const type = (text) => {
            q.value = text;
            q.dispatchEvent(new Event("input", { bubbles: true }));
          };
          setTimeout(() => type(first), 0);
          setTimeout(() => type(second), 250);
        },
        [hub.fixture.searchTerm, hub.fixture.searchMarkup],
      );
      // The second query's results paint while the first is still held.
      await expect(page.locator("main")).toContainText(hub.fixture.markup);
      release();
      await late;
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    } finally {
      await page.unroute(slow).catch(() => {});
    }
    expect(asked, "the first query was never asked, so nothing raced").toBe(true);
    const body = await page.evaluate(() => (document.querySelector("main") || {}).textContent || "");
    expect(body, "the older query's late answer painted over the newer one").not.toContain(hub.fixture.finished);
    expect(body, "the newer query's results are not on screen").toContain(hub.fixture.markup);
    expect((await searchParams(page)).q, "the route does not name the newer query").toBe(hub.fixture.searchMarkup);

    // A letter typed on the way out: its timer fires after the screen has gone,
    // and the route by then is the next screen's to hold.
    await page.evaluate(() => {
      const send = window.fetch;
      window.__sendNow = () => {
        window.fetch = send;
      };
      window.fetch = (url, options) =>
        String(url).includes("/api/v1/storage")
          ? new Promise((go) => setTimeout(go, 500)).then(() => send(url, options))
          : send(url, options);
    });
    await page.evaluate((text) => {
      const q = document.getElementById("q");
      q.value = text;
      q.dispatchEvent(new Event("input", { bubbles: true }));
      location.hash = "#/storage";
    }, hub.fixture.searchTerm);
    expect(
      await settle(page, "(document.querySelector('main h1') || {}).textContent === 'Storage'"),
    ).toBe(true);
    await page.evaluate(() => window.__sendNow && window.__sendNow());
    expect(await page.evaluate(() => location.hash), "the screen left behind took the route back").toBe("#/storage");
    expect(await heading(page)).toBe("Storage");
  });
});

test.describe("summary and message", () => {
  // A long summary is body prose, not one long heading, on both stages. A
  // short subject keeps today's title treatment.
  test("a long summary is body prose and a short subject stays a title", async ({ hub, page }) => {
    const previous = page.viewportSize();
    await page.setViewportSize({ width: 1440, height: 900 });
    const eventId = await longSummaryEvent(hub, hub.projectId, LONG_SUMMARY);
    expect(eventId, "the long-summary event never reached the feed").toBeTruthy();
    const readSurface = () =>
      page.evaluate(() => {
        const read = (sel) => {
          const el = document.querySelector(sel);
          if (!el || !el.getClientRects().length) return { sel, missing: true };
          const cs = getComputedStyle(el);
          const lineHeight = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.3;
          return {
            sel,
            tag: el.tagName.toLowerCase(),
            size: cs.fontSize,
            weight: cs.fontWeight,
            lines: Math.round(el.getBoundingClientRect().height / lineHeight),
            chars: el.textContent.trim().length,
            heading: el.tagName.toLowerCase().startsWith("h"),
          };
        };
        return [
          read("#inbox-detail-title"),
          read(".inbox-detail-message"),
          read(".feed-stage-title"),
          read(".feed-stage-message"),
        ];
      });

    for (const [route, titleSel, messageSel, label] of [
      [`#/inbox?open=${encodeURIComponent(eventId)}`, "#inbox-detail-title", ".inbox-detail-message", "inbox detail"],
      [
        `#/projects/${encodeURIComponent(hub.projectId)}/feed?event=${encodeURIComponent(eventId)}`,
        ".feed-stage-title",
        ".feed-stage-message",
        "feed stage",
      ],
    ]) {
      await goto(page, route);
      expect(
        await settle(page, `!!document.querySelector(${JSON.stringify(titleSel)})`),
        `${label}: the event did not open`,
      ).toBe(true);
      const rows = Object.fromEntries((await readSurface()).map((row) => [row.sel, row]));
      const title = rows[titleSel] || {};
      const message = rows[messageSel] || {};
      expect(title.heading, `${label}: the accessible title is not a heading`).toBe(true);
      expect(
        title.chars,
        `${label}: the heading carries the whole ${title.chars}-character summary`,
      ).toBeLessThan(LONG_SUMMARY.length);
      expect(message.missing, `${label}: a long summary has no message paragraph`).toBeFalsy();
      expect(
        message.size === "15px" && (message.weight === "400" || message.weight === "normal"),
        `${label}: the message is ${message.size}/${message.weight}, not body prose at 15px normal`,
      ).toBe(true);
      expect(
        message.chars >= Math.floor(LONG_SUMMARY.length / 2),
        `${label}: the message keeps too little of the summary`,
      ).toBe(true);
      expect(title.chars + message.chars, `${label}: the split drops characters`).toBeGreaterThanOrEqual(
        LONG_SUMMARY.length,
      );
    }

    const feed = (await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(hub.projectId)}/feed?limit=20`)).json();
    const shortId = (feed.events.find((event) => event.summary === hub.fixture.question) || {}).id || "";
    expect(shortId, "the seeded question subject never reached the feed").toBeTruthy();
    for (const [route, titleSel, messageSel, label] of [
      [`#/inbox?open=${encodeURIComponent(shortId)}`, "#inbox-detail-title", ".inbox-detail-message", "inbox detail"],
      [
        `#/projects/${encodeURIComponent(hub.projectId)}/feed?event=${encodeURIComponent(shortId)}`,
        ".feed-stage-title",
        ".feed-stage-message",
        "feed stage",
      ],
    ]) {
      await goto(page, route);
      expect(
        await settle(page, `!!document.querySelector(${JSON.stringify(titleSel)})`),
        `${label}: the short subject did not open`,
      ).toBe(true);
      const rows = Object.fromEntries((await readSurface()).map((row) => [row.sel, row]));
      const title = rows[titleSel] || {};
      const message = rows[messageSel] || {};
      await expect(page.locator(titleSel)).toContainText(hub.fixture.question);
      expect(
        ["17px", "22px"].includes(title.size) && (title.weight === "600" || title.weight === "bold"),
        `${label}: a short subject lost its item-title treatment`,
      ).toBe(true);
      expect(message.missing, `${label}: a short subject was split into a message paragraph`).toBeTruthy();
    }
    if (previous) await page.setViewportSize(previous);
  });
});
