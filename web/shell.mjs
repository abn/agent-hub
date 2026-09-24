// The shell around the screens: the desktop app rail's node line, project list,
// unread badge mirroring, and sync indicator.

import { api } from "./api.mjs";
import { esc } from "./dom.mjs";
import { prefs } from "./prefs.mjs";
import { relative } from "./time.mjs";

const nodeLine = document.getElementById("top-node");
const railProjects = document.getElementById("rail-projects");
const syncLine = document.getElementById("rail-sync");

let lastSyncTime = Date.now();
// The rail shows the sync line only when there is something to say. A healthy
// hub gets no standing "Synced": a line that is always there is a line nobody
// reads. Stale means the last successful call is older than this; failed means
// a call threw. Threshold chosen here because the designer's round 12.1 says
// "stale or failed" without defining stale, and that is an open question.
const SYNC_STALE_MS = 5 * 60 * 1000;
let syncFailed = false;

export function updateRailSync(time = Date.now(), failed = false) {
  lastSyncTime = time;
  if (failed) syncFailed = true;
  revealSyncLineWhenUnhealthy();
}

function revealSyncLineWhenUnhealthy() {
  if (!syncLine) return;
  const age = Date.now() - lastSyncTime;
  const stale = age >= SYNC_STALE_MS;
  if (!syncFailed && !stale) {
    syncLine.hidden = true;
    syncLine.textContent = "";
    return;
  }
  syncLine.hidden = false;
  syncLine.textContent = syncFailed ? "not synced" : `synced ${relative(lastSyncTime)}`;
  syncLine.dataset.state = syncFailed ? "failed" : "stale";
}

// The one call the rail already makes is the storage read that fills the node
// line. Its failure is the rail's failure signal, so the sync line appears
// exactly when the hub stops answering.
export function noteSyncFailure() {
  syncFailed = true;
  revealSyncLineWhenUnhealthy();
}

export function noteSyncSuccess() {
  syncFailed = false;
  updateRailSync(Date.now());
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
    const usage = await api("/api/v1/storage");
    if (nodeLine) {
      nodeLine.textContent = `${usage.node.host} · ${usage.node.mode}`;
    }
    noteSyncSuccess();
  } catch {
    noteSyncFailure();
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
  setInterval(() => updateRailSync(lastSyncTime, syncFailed), 30000);
}
