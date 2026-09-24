// The shell around the screens: the desktop app rail's node line, project list,
// unread badge mirroring, and sync indicator.

import { api } from "./api.mjs";
import { esc } from "./dom.mjs";
import { prefs } from "./prefs.mjs";
import { relative } from "./time.mjs";

const nodeLine = document.getElementById("top-node");
const railProjects = document.getElementById("rail-projects");

// The sync state, as §13 defines it.
//
// Shown only when it is not healthy, on both widths and in no reserved space:
// a line that is always there is a line nobody reads. Three states.
//
//   healthy  nothing is shown anywhere.
//   stale    no successful response for five minutes while the page is visible.
//   failed   two consecutive failed requests (a network error, a ten second
//            timeout, or a 5xx), or the browser reporting itself offline.
//
// A 401 is neither: it means the token is gone, so it routes to Connect.
// A hidden tab never goes stale, so a background page does not accuse the hub
// of a silence it did not break; on becoming visible it fetches first and only
// then, if still old, shows stale. Any success hides the line at once, with no
// "synced" flash on the way back.
const SYNC_STALE_MS = 5 * 60 * 1000;
const SYNC_TIMEOUT_MS = 10 * 1000;

let lastSyncTime = Date.now();
let consecutiveFailures = 0;
let syncState = "healthy";

const syncTargets = () => {
  if (typeof document === "undefined") return [];
  return [document.getElementById("rail-sync"), document.getElementById("more-sync")].filter(Boolean);
};

function renderSync() {
  for (const el of syncTargets()) {
    const text = el.querySelector(".more-sync-text") || el;
    const action = el.querySelector(".more-refresh-btn");
    if (syncState === "healthy") {
      el.hidden = true;
      text.textContent = "";
      delete el.dataset.state;
      continue;
    }
    el.hidden = false;
    el.dataset.state = syncState;
    if (syncState === "failed") {
      // The word carries the state and the tone only agrees with it.
      const stamp = new Date(lastSyncTime);
      const hh = String(stamp.getHours()).padStart(2, "0");
      const mm = String(stamp.getMinutes()).padStart(2, "0");
      text.textContent = navigatorOnline() ? `not synced · ${hh}:${mm}` : "offline";
    } else {
      text.textContent = `synced ${relative(lastSyncTime)}`;
    }
    if (action) action.textContent = syncState === "failed" ? "Retry" : "Refresh";
  }
}

function navigatorOnline() {
  return typeof navigator === "undefined" || navigator.onLine !== false;
}

// The page's own visibility is the clock's gate: a hidden tab does not age.
function staleNow() {
  if (typeof document !== "undefined" && document.visibilityState === "hidden") return false;
  return Date.now() - lastSyncTime >= SYNC_STALE_MS;
}

function recomputeSync() {
  if (consecutiveFailures >= 2 || !navigatorOnline()) {
    syncState = "failed";
  } else if (staleNow()) {
    syncState = "stale";
  } else {
    syncState = "healthy";
  }
  renderSync();
}

export function syncStateNow() {
  return syncState;
}

// A screen that carries a sync line calls this after it paints, so the line
// shows the state as it stands rather than the state as it was.
export function refreshSyncDisplay() {
  renderSync();
}

// The behavioral harness drives these states directly, because CHECK 12.1.B is
// about a counter ("two consecutive failures"), and a counter that cannot be
// set from a test cannot be checked from one either.
if (typeof window !== "undefined") {
  window.__sync = { noteSyncFailure, noteSyncSuccess, syncStateNow, refreshSyncDisplay };
}

export function noteSyncSuccess(time = Date.now()) {
  lastSyncTime = time;
  consecutiveFailures = 0;
  recomputeSync();
}

// `kind` is what the caller knows about the failure. A 401 is not a sync
// failure at all, so it routes to Connect instead of accusing the hub.
export function noteSyncFailure({ status = 0, kind = "error" } = {}) {
  if (status === 401 || kind === "unauthorized") {
    routeToConnect();
    return;
  }
  consecutiveFailures += 1;
  recomputeSync();
}

function routeToConnect() {
  if (typeof location === "undefined") return;
  if (location.hash.startsWith("#/connect")) return;
  location.hash = "#/connect";
}

export function installSyncWatch() {
  if (typeof window === "undefined" || syncWatchInstalled) return;
  syncWatchInstalled = true;
  const refresh = () => recomputeSync();
  window.addEventListener("online", refresh);
  window.addEventListener("offline", refresh);
  document.addEventListener("visibilitychange", () => {
    recomputeSync();
  });
  setInterval(refresh, 30000);
}
let syncWatchInstalled = false;

// A call that has to answer within a bounded time, so a hung request counts as
// a failure rather than as silence.
export async function syncFetch(path) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), SYNC_TIMEOUT_MS);
  try {
    const res = await api(path, { signal: controller.signal });
    noteSyncSuccess();
    return res;
  } catch (error) {
    noteSyncFailure({
      status: error?.status || 0,
      kind: error?.status === 401 ? "unauthorized" : "error",
    });
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

// The node line names the hub the operator is looking at. It is the storage
// response's node, which the real-numbers surface already reports, so the two
// cannot drift.
let askedForToken = false;
async function fillNodeLine() {
  if (!prefs.token) {
    askedForToken = true;
    setTimeout(fillNodeLine, 15000);
    return;
  }
  try {
    const usage = await syncFetch("/api/v1/storage");
    if (nodeLine) {
      nodeLine.textContent = `${usage.node.host} · ${usage.node.mode}`;
    }
  } catch {
    setTimeout(fillNodeLine, 15000);
  }
}

// The tab bar's badge is the app's one count. The rail badge mirrors it so
// the desktop Inbox link reads the same number.
const tabBadge = document.getElementById("tab-badge");
const topBadge = document.getElementById("top-badge");
function mirrorBadge() {
  if (!tabBadge || !topBadge) return;
  topBadge.hidden = tabBadge.hidden;
  topBadge.textContent = tabBadge.textContent;
}
if (tabBadge && "MutationObserver" in window) {
  new MutationObserver(mirrorBadge).observe(tabBadge, {
    attributes: true,
    childList: true,
    subtree: true,
    attributeFilter: ["hidden"],
  });
}

// A personal space is a project an agent owns, and `owner_agent` is what says
// so: the hub sets it, and deleting a project refuses on it. By owner ruling
// these never appear in the rail.
//
// Deliberately not a name test as well. Matching `space-` or a "(personal)"
// suffix adds nothing the field does not already say, and takes away an
// ordinary project that happens to be called `space-invaders`: it would
// vanish from the reader's navigation with nothing on screen to say why.
function isPersonalSpace(p) {
  return Boolean(p.owner_agent);
}

// Render regular projects into the rail's PROJECTS list.
export async function renderRailProjects() {
  if (!railProjects || !prefs.token) return;
  try {
    const { projects } = await api("/api/v1/projects");
    const regular = (projects || []).filter((p) => !isPersonalSpace(p));

    // The live dot rides on the listing this already fetched. It used to ask
    // `/stats` once per project for the same boolean, and the rail is on every
    // screen at this width, so that cost was paid on every navigation. Reading
    // it from the storage payload instead would have been worse: that endpoint
    // walks the data directory for byte totals, which is a great deal of work
    // for one dot.

    let waitingItems = [];
    try {
      const [actionRes, waitingRes] = await Promise.all([
        api("/api/v1/inbox?status=action&limit=500"),
        api("/api/v1/inbox?status=waiting&limit=500"),
      ]);
      waitingItems = [...(actionRes?.items || []), ...(waitingRes?.items || [])];
    } catch {}

    const waitingMap = new Map();
    for (const item of waitingItems) {
      if (item.project_id) {
        waitingMap.set(item.project_id, (waitingMap.get(item.project_id) || 0) + 1);
      }
    }

    const currentHash = location.hash.replace(/^#/, "").split("?")[0];
    railProjects.innerHTML = "";

    for (let i = 0; i < regular.length; i++) {
      const p = regular[i];
      const isLive = (p.agents_active || 0) > 0;
      const waiting = waitingMap.get(p.id) || 0;
      const isCurrent = currentHash.startsWith(`/projects/${encodeURIComponent(p.id)}`);

      const a = document.createElement("a");
      a.href = `#/projects/${encodeURIComponent(p.id)}/feed`;
      a.className = "rail-item rail-project-item";
      a.dataset.projectId = p.id;
      if (isCurrent) a.setAttribute("aria-current", "page");

      const dot = document.createElement("span");
      dot.className = `rail-dot ${isLive ? "live" : "idle"}`;
      dot.setAttribute("aria-hidden", "true");

      const label = document.createElement("span");
      label.className = "rail-label";
      label.textContent = p.display_name || p.id;

      a.append(dot, label);

      if (waiting > 0) {
        const badge = document.createElement("span");
        badge.className = "badge waiting rail-project-badge";
        badge.textContent = String(waiting);
        badge.setAttribute("aria-label", `${waiting} waiting on you`);
        a.appendChild(badge);
      }

      railProjects.appendChild(a);
    }
  } catch {}
}

export function installShell() {
  if (askedForToken) return;
  fillNodeLine();
  mirrorBadge();
  renderRailProjects();
  installSyncWatch();
  renderSync();
}
