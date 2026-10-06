// The shared ground for the behavioural invariant checks: the fixture the
// seeded hub carries, a way to speak to the hub's HTTP and MCP surfaces from
// the test process, and the few page probes the checks ask of a rendered
// screen. It is the port of the helpers in the old invariants.py, so the
// checks themselves stay about behaviour rather than about plumbing.
//
// The `watch` fixture is what the old script's Watch class was: it holds the
// browser to no console error and no refused request while a check runs, and a
// check that expects a refusal names it with `watch.ignore` rather than
// disarming a global flag. The assertion runs when the test ends, so a check
// that forgets to whitelist an expected refusal fails here.

import { expect } from "@playwright/test";
import { test as app } from "./app.mjs";
import { readHub } from "./hub.mjs";

export { expect };

export const test = app.extend({
  // The seeded hub for this project, with the app already loaded. The base
  // fixture only writes the token; a check that starts with `goto` would
  // otherwise set a hash on `about:blank` and never reach the app. Every check
  // here starts from the app's own address and moves by hash from there.
  hub: async ({ page }, use, testInfo) => {
    const hub = readHub(testInfo.project.name);
    await page.addInitScript((token) => window.localStorage.setItem("hub.token", token), hub.token);
    await page.goto(`${hub.baseUrl}/`);
    await use(hub);
  },
  // What the browser reported while the check ran, asserted when it ends.
  watch: [
    async ({ page }, use) => {
      const failures = [];
      const ignored = [];
      let armed = true;
      const note = (text) => {
        if (armed && !ignored.some((pattern) => pattern.test(text))) failures.push(text);
      };
      page.on("console", (message) => {
        if (message.type() !== "error") return;
        // A refused fetch logs a bare "Failed to load resource" with no URL, so
        // the console message's own location is what a check can whitelist.
        const where = message.location()?.url || "";
        note(`console error: ${message.text()}${where ? ` (${where})` : ""}`);
      });
      page.on("response", (response) => {
        if (response.status() >= 400) {
          note(`${response.request().method()} ${response.url()} answered ${response.status()}`);
        }
      });
      const watch = {
        failures,
        // A refusal this check is about rather than a defect: the URL is
        // matched against the path the hub answered.
        ignore(pattern) {
          ignored.push(pattern instanceof RegExp ? pattern : new RegExp(pattern));
        },
        // A check that drives a refusal of its own hands the browser back to
        // the watch after it, the way the old script re-armed the flag.
        disarm() {
          armed = false;
          return () => {
            armed = true;
          };
        },
      };
      await use(watch);
      armed = true;
      expect(failures, "the browser reported problems while the check ran").toEqual([]);
    },
    { auto: true },
  ],
});

// The days the Home greeting names, by the same order the browser's own clock
// uses: Sunday is 0.
const HOME_DAYS = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
];

function homePart(now) {
  if (now.getHours() < 5 || now.getHours() >= 21) return "night";
  if (now.getHours() < 12) return "morning";
  return now.getHours() < 17 ? "afternoon" : "evening";
}

// The greeting Home paints. Before 05:00 the hub still calls it yesterday, so
// the day name follows the hub's own rule rather than the wall clock's.
export function homeTitle() {
  const now = new Date();
  const day = now.getHours() < 5 ? new Date(now.getTime() - 86_400_000) : now;
  return `${HOME_DAYS[day.getDay()]} ${homePart(now)}`;
}

// Wait on a condition in the page rather than on a fixed sleep. It returns
// whether the condition held, so a check that needs to say why can.
//
// The expression is compiled into a function here rather than handed to
// `waitForFunction` as a string: the hub serves the app under a CSP with no
// `unsafe-eval`, and the string form is evaluated with the page's own Function
// constructor, which that policy refuses. A real function is serialized over
// the debugger protocol instead, so the policy never sees it.
export async function settle(page, expression, timeout = 8000) {
  const condition = new Function(`return (${expression});`);
  try {
    await page.waitForFunction(condition, null, { timeout });
    return true;
  } catch {
    return false;
  }
}

export function heading(page) {
  return page.evaluate(() => {
    const h = document.querySelector("main h1");
    return h ? h.textContent.trim() : "";
  });
}

// Move the hash and wait for the screen's heading, the way the old script's
// goto did. A route whose heading is not the screen's own (the project stage
// and the viewer) passes no title and the check waits for what it needs.
export async function goto(page, hash, title) {
  await page.evaluate((value) => {
    location.hash = value;
  }, hash);
  if (!title) return;
  await page
    .waitForFunction(
      (want) => {
        const h = document.querySelector("main h1");
        return !!h && h.textContent.trim() === want;
      },
      title,
      { timeout: 5000 },
    )
    .catch(() => {});
}

// Anything carrying the hidden attribute and still occupying the screen. An
// author `display` beats the browser's own `[hidden] { display: none }`, so a
// control the script believes it has hidden stays where it was. Asked of the
// rendered page, because the markup says hidden in every one of those cases.
export function stillShowing(page) {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll("[hidden]"))
      .filter((el) => el.getClientRects().length > 0)
      .map((el) =>
        `${el.tagName.toLowerCase()}.${(el.className || "").toString().trim().split(/\s+/).slice(0, 2).join(".")}`,
      )
      .slice(0, 6),
  );
}

export function searchParams(page) {
  return page.evaluate(() =>
    Object.fromEntries(new URLSearchParams(location.hash.split("?")[1] || "")),
  );
}

export function gateOf(page) {
  return page.frameLocator("main iframe");
}

// The hub's own HTTP surface, with the admin token the descriptor carries.
export async function api(hub, method, path, body) {
  const response = await fetch(`${hub.baseUrl}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${hub.token}`,
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  return { status: response.status, text, json: () => JSON.parse(text) };
}

// One MCP session against the hub, held as a mutable object so the assigned
// session id is carried by every later call.
export async function mcp(hub, session, payload) {
  const headers = {
    Authorization: `Bearer ${hub.token}`,
    "Content-Type": "application/json",
    Accept: "application/json, text/event-stream",
  };
  if (session.id) headers["mcp-session-id"] = session.id;
  const response = await fetch(`${hub.baseUrl}/mcp`, {
    method: "POST",
    headers,
    body: JSON.stringify(payload),
  });
  const assigned = response.headers.get("mcp-session-id");
  if (assigned && !session.id) session.id = assigned;
  const raw = await response.text();
  for (const line of raw.split("\n")) {
    if (line.startsWith("data:")) {
      const value = line.slice(5).trim();
      if (value) return JSON.parse(value);
    }
  }
  return {};
}

export async function newSession(hub) {
  const session = { id: "" };
  await mcp(hub, session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "checks", version: "0.0.0" },
    },
  });
  await mcp(hub, session, { jsonrpc: "2.0", method: "notifications/initialized" });
  return session;
}

export async function toolCall(hub, session, name, args) {
  return mcp(hub, session, {
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name, arguments: args },
  });
}

export const structured = (answer) => (answer?.result?.structuredContent) || {};

// One feed event of the given kind, and its id, so a check can address it.
export async function oneOffEvent(hub, project, kind, summary) {
  const session = await newSession(hub);
  await toolCall(hub, session, "signal_append", { project_id: project, kind, summary });
  const feed = (await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(project)}/feed?limit=5`)).json();
  return (feed.events.find((event) => event.summary === summary) || {}).id || "";
}

// One open question, and its inbox event id.
export async function oneOffQuestion(hub, project, subject) {
  const session = await newSession(hub);
  await toolCall(hub, session, "question_post", { project_id: project, subject });
  for (const status of ["action", "waiting"]) {
    const held = (await api(hub, "GET", `/api/v1/inbox?status=${status}&limit=500`)).json();
    const item = held.items.find((entry) => entry.summary === subject);
    if (item) return item.event_id;
  }
  return "";
}

// One finished event with a message-length summary, and its id.
export async function longSummaryEvent(hub, project, summary) {
  const session = await newSession(hub);
  await toolCall(hub, session, "signal_append", {
    project_id: project,
    kind: "finished",
    summary,
  });
  const feed = (await api(hub, "GET", `/api/v1/projects/${encodeURIComponent(project)}/feed?limit=8`)).json();
  const found = feed.events.find((event) => event.summary === summary);
  return found ? found.id : "";
}

// One feed event as the Home response carries it, used by the quiet-state
// check that answers the Home request itself.
export function homeEvent(index, minutes, fields = {}) {
  return {
    id: `01HOME${String(index).padStart(20, "0")}`,
    project_id: "homelab",
    kind: "finished",
    actor: "backup-agent",
    summary: `home event ${index}`,
    payload: null,
    thread_id: null,
    needs_action: false,
    created_at: new Date(Date.now() - minutes * 60_000).toISOString(),
    inbox_status: null,
    ...fields,
  };
}

// A Home response with nothing waiting and nothing new, plus overrides. The
// quiet state needs every inbox item read and every feed cursor at its head,
// which no other check wants done to the seeded hub, so this answers the one
// Home request instead. The byte figures are the harness's own quiet payload.
export function quietHomePayload(fields = {}) {
  return {
    unread: 0,
    waiting: 0,
    agents_active: 2,
    last_event_at: null,
    recent: [],
    unseen: [],
    storage: {
      used_bytes: 6_012_954_214,
      capacity_bytes: 34_359_738_368,
      free_bytes: 28_346_784_154,
    },
    prunable: { sessions: 0, bytes: 0 },
    ...fields,
  };
}

// A summary over this length, or one holding a newline, is a message and not a
// subject: the item titles it with its leading sentence and reads the rest as
// body prose.
export const LONG_SUMMARY =
  "The nightly run is green. 42 checks passed, the loader rewrite is behind the flag, " +
  "and the artifact viewer now reserves its 52px chrome like every other pane.";
