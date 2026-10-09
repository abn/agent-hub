// The app behind a path-stripping reverse proxy, on Playwright Test. Every other
// check drives the hub at the origin root, where an absolute path from the root
// and a path relative to the document land on the same URL. A production
// deployment fronts the hub with a proxy mounted on a path that strips the prefix
// before forwarding, so a client-side reference written from the origin root
// 404s there and every other check still passes.
//
// It is the port of prefix-smoke.py: a small proxy that strips /hub, and a real
// browser driven at the proxy. It runs in its own project (prefix) against its
// own seeded hub, because it registers a service worker and connects.

import { execFileSync } from "node:child_process";
import http from "node:http";
import { expect } from "@playwright/test";
import { test as base } from "./app.mjs";

const PREFIX = "/hub";
// Render-blocking, so a browser always requests them on every load: a reliable
// signal for the boot check.
const CORE_ASSETS = ["app.js", "app.css", "tokens.css"];

// A reverse proxy mounted on PREFIX that strips it and forwards to the hub,
// preserving the client-facing host as a real proxy does. It refuses anything
// not under the prefix, and forwards "/hub" and "/hub/" as-is, unredirected:
// that trailing-slash behaviour is half of what this check holds the app to.
function startProxy(upstreamPort) {
  const server = http.createServer((req, res) => {
    const path = req.url || "/";
    let forward;
    if (path === PREFIX) forward = "/";
    else if (path.startsWith(`${PREFIX}/`)) forward = path.slice(PREFIX.length);
    else {
      res.writeHead(404, { "content-type": "text/plain" });
      res.end("not under the proxied prefix");
      return;
    }
    const headers = { ...req.headers };
    delete headers.host;
    delete headers.connection;
    headers["x-forwarded-host"] = req.headers.host || "";
    const upstream = http.request(
      { host: "127.0.0.1", port: upstreamPort, path: forward, method: req.method, headers },
      (up) => {
        const out = { ...up.headers };
        delete out.connection;
        delete out["transfer-encoding"];
        res.writeHead(up.statusCode || 502, out);
        up.pipe(res);
      },
    );
    upstream.on("error", () => {
      res.writeHead(502, { "content-type": "text/plain" });
      res.end("upstream unavailable");
    });
    req.pipe(upstream);
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve({ server, port: server.address().port }));
  });
}

// Poll a condition from outside the page. The shell's CSP has no unsafe-eval,
// which trips page.waitForFunction's injected evaluator, so the poll is explicit.
async function settle(page, expression, timeout = 8000) {
  const deadline = Date.now() + timeout;
  for (;;) {
    if (await page.evaluate(`!!(${expression})`)) return true;
    if (Date.now() >= deadline) return false;
    await page.waitForTimeout(100);
  }
}

const assetName = (url) => url.split("/").pop().split("?")[0];

// Publish an HTML-kind artifact, so the server-rendered /artifacts/{id}/frame
// route is exercised too, not only the client-rendered markdown path. The CLI is
// the same one-shot interface a hook uses.
function publishHtml(hub, project) {
  const bin = process.env.HUB_BIN || "target/debug/agent-hub";
  const out = execFileSync(
    bin,
    [
      "call",
      "artifact_publish",
      JSON.stringify({
        project_id: project,
        title: "Prefix check page",
        kind: "html",
        content: "<p>prefix smoke</p>",
      }),
    ],
    { env: { ...process.env, HUB_URL: hub.baseUrl, HUB_TOKEN: hub.token } },
  ).toString();
  return JSON.parse(out).artifact_id;
}

const test = base.extend({
  // A prefix-stripping proxy in front of this project's hub, for the life of the
  // test.
  prefix: async ({ hub }, use) => {
    const upstream = Number(new URL(hub.baseUrl).port);
    const { server, port } = await startProxy(upstream);
    await use(`http://127.0.0.1:${port}`);
    // Keep-alive connections keep server.close from resolving; drop them first.
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  },
});

test("the app works when served behind a prefix-stripping proxy", async ({ hub, page, prefix }) => {
  const failures = [];
  const entries = [];
  page.on("response", (r) => entries.push([r.url(), r.status()]));
  page.on("pageerror", (e) => {
    // A sandboxed artifact frame reads localStorage and throws by design; the
    // viewer's opaque origin is its own rule, not a fault of the prefix.
    const said = String(e);
    if (!said.includes("sandboxed")) failures.push(`uncaught page error: ${said}`);
  });

  // Entering at the bare prefix must normalise to the trailing slash, and the
  // shell must actually boot: `main` is only filled by the router after app.js
  // runs, unlike the static markup which paints regardless.
  const boot = async (path) => {
    const start = entries.length;
    await page.goto(prefix + path, { waitUntil: "load" });
    await settle(page, `location.pathname === "${PREFIX}/"`, 5000);
    if (!page.url().endsWith(`${PREFIX}/`)) {
      failures.push(`entering at ${path} left the address at ${page.url()}`);
    }
    if (!(await settle(page, "document.querySelector('main') && document.querySelector('main').children.length > 0"))) {
      failures.push(`entering at ${path}: the app never booted (main stayed empty)`);
      return false;
    }
    await page.waitForTimeout(300);
    const ok = new Set(entries.slice(start).filter(([, s]) => s === 200).map(([u]) => assetName(u)));
    const missing = CORE_ASSETS.filter((name) => !ok.has(name));
    if (missing.length) failures.push(`entering at ${path}: assets never loaded with 200: ${missing}`);
    return true;
  };

  if (!(await boot(PREFIX))) {
    expect(failures).toEqual([]);
    return;
  }

  // Connect under the prefix, then an authenticated call must succeed.
  const field = "main .connect .connect-field";
  if (!(await settle(page, `document.querySelector('${field}')`))) {
    await page.evaluate("location.hash = '#/connect'");
    await settle(page, `document.querySelector('${field}')`);
  }
  await page.fill(field, hub.token);
  await page.click("main .connect button[type='submit']");
  if (!(await settle(page, `localStorage.getItem('hub.token') === ${JSON.stringify(hub.token)}`))) {
    failures.push("the connect screen did not store the token under the prefix");
  }
  const apiStatus = await page.evaluate(
    (token) =>
      fetch("api/v1/home", { headers: { Authorization: `Bearer ${token}` } }).then((r) => r.status),
    hub.token,
  );
  if (apiStatus !== 200) failures.push(`an authenticated call to api/v1/home answered ${apiStatus}`);

  // The service worker registers under the prefix, controls the page, leaves API
  // calls to the network, and never caches an API response.
  const ready = await page.evaluate(() =>
    Promise.race([
      navigator.serviceWorker.ready.then((reg) => ({ ok: true, scope: reg.scope })),
      new Promise((resolve) => setTimeout(() => resolve({ ok: false }), 8000)),
    ]),
  );
  if (!ready.ok) {
    failures.push("the service worker never became ready under the prefix");
  } else if (!ready.scope.endsWith(`${PREFIX}/`)) {
    failures.push(`the service worker registered at scope ${ready.scope}, not under ${PREFIX}/`);
  } else if (!(await settle(page, "!!navigator.serviceWorker.controller", 8000))) {
    failures.push("the service worker never took control of the page under the prefix");
  } else {
    const [response] = await Promise.all([
      page.waitForResponse((r) => r.url().includes("api/v1/home")),
      page.evaluate(
        (token) =>
          fetch("api/v1/home", { headers: { Authorization: `Bearer ${token}` } }).then((r) => r.status),
        hub.token,
      ),
    ]);
    if (response.fromServiceWorker()) {
      failures.push("a request to api/v1/home was answered by the service worker instead of the network");
    }
    const cached = await page.evaluate(async () => {
      const names = await caches.keys();
      const shell = names.find((n) => n.startsWith("agent-hub-shell-"));
      if (!shell) return { found: false, apiKeys: [] };
      const cache = await caches.open(shell);
      const keys = await cache.keys();
      return {
        found: true,
        apiKeys: keys.map((r) => new URL(r.url).pathname).filter((p) => p.includes("/api/")),
      };
    });
    if (!cached.found) failures.push("no offline shell cache was opened, so the worker never installed");
    else if (cached.apiKeys.length) failures.push(`the worker cached API responses: ${cached.apiKeys}`);
  }

  // Both artifact frame paths load their loader script through the prefix.
  const htmlId = publishHtml(hub, hub.projectId);
  const frame = async (id, wanted, where) => {
    const start = entries.length;
    await page.evaluate(`location.hash = '#/artifacts/${encodeURIComponent(id)}?project=${encodeURIComponent(hub.projectId)}'`);
    // The stage of the previous artifact is still on screen until the new one
    // paints, so the wait is for this artifact's own frame.
    const painted = `document.querySelector('main .shell.has-selection[data-segment="artifacts"] #hub-frame[src*="${id}"]')`;
    if (!(await settle(page, painted))) {
      failures.push(`the ${where} artifact viewer never painted under the prefix`);
    }
    await page.waitForTimeout(700);
    const bad = entries.slice(start).filter(([, s]) => s >= 400).map(([u, s]) => `${s} ${u}`);
    if (bad.length) failures.push(`loading the ${where} artifact under the prefix 404d: ${bad.join("; ")}`);
    const names = new Set(entries.slice(start).filter(([, s]) => s === 200).map(([u]) => assetName(u)));
    const missing = wanted.filter((name) => !names.has(name));
    if (missing.length) failures.push(`loading the ${where} artifact never fetched ${missing}`);
  };
  await frame(hub.artifactId, ["frame-loader.js"], "markdown");
  await frame(htmlId, ["frame-loader.js", "artifact-viewer.mjs", "tokens.css", "marked.js"], "html");

  // The raw fetch carries the prefix, rather than being built from the origin
  // root.
  const rawStart = entries.length;
  await page.evaluate(`location.hash = '#/artifacts/${encodeURIComponent(hub.artifactId)}?project=${encodeURIComponent(hub.projectId)}'`);
  if (await settle(page, `document.querySelector('.shell-controls [data-action="copy-raw"][data-id="${hub.artifactId}"]')`)) {
    await page.getByRole("button", { name: "Copy raw" }).click();
    await page.waitForTimeout(1500);
    const raw = entries.slice(rawStart).filter(([u]) => u.includes("/raw"));
    if (!raw.length) failures.push("the raw fetch never reached the network under the prefix");
    for (const [url, status] of raw) {
      if (status !== 200) failures.push(`the raw fetch answered ${status}: ${url}`);
      if (!url.includes(PREFIX)) failures.push(`the raw fetch dropped the prefix: ${url}`);
    }
  } else {
    failures.push("the artifact viewer offered no raw-copy control under the prefix");
  }

  // The share link the hub returns resolves under the prefix, and its page
  // loads its frame and assets.
  const share = await page.request.post(
    `${hub.baseUrl}/api/v1/artifacts/${encodeURIComponent(htmlId)}/share`,
    { headers: { Authorization: `Bearer ${hub.token}` } },
  );
  const shareData = await share.json();
  const resolved = new URL(shareData.url, `${prefix}${PREFIX}/`).href;
  if (!resolved.includes(PREFIX)) failures.push(`the share link drops the prefix: ${shareData.url}`);
  const shareStart = entries.length;
  await page.goto(`${prefix}${PREFIX}/s/${shareData.token}`, { waitUntil: "load" });
  if (!(await settle(page, "document.querySelector('main iframe')", 8000))) {
    failures.push("the share link page embedded no frame under the prefix");
  }
  await page.waitForTimeout(600);
  const shareBad = entries.slice(shareStart).filter(([, s]) => s >= 400).map(([u, s]) => `${s} ${u}`);
  if (shareBad.length) failures.push(`loading the share link page 404d: ${shareBad.join("; ")}`);

  expect(failures).toEqual([]);
});

test("entering with the trailing slash already present boots too", async ({ page, prefix }) => {
  const entries = [];
  page.on("response", (r) => entries.push([r.url(), r.status()]));
  await page.goto(`${prefix}${PREFIX}/`, { waitUntil: "load" });
  const booted = await settle(page, "document.querySelector('main') && document.querySelector('main').children.length > 0");
  expect(booted, "the app never booted at the trailing-slash address").toBe(true);
  const ok = new Set(entries.filter(([, s]) => s === 200).map(([u]) => assetName(u)));
  expect(CORE_ASSETS.filter((name) => !ok.has(name)), "core assets never loaded").toEqual([]);
});
