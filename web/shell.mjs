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

export function updateRailSync(time = Date.now()) {
  lastSyncTime = time;
  if (syncLine) {
    syncLine.textContent = `synced ${relative(lastSyncTime)}`;
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
    const usage = await api("/api/v1/storage");
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

    const statsList = await Promise.all(
      regular.map((p) =>
        api(`/api/v1/projects/${encodeURIComponent(p.id)}/stats`).catch(() => null),
      ),
    );

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
      const stats = statsList[i];
      const isLive = (stats?.agents_active || 0) > 0;
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
  setInterval(() => updateRailSync(lastSyncTime), 30000);
}
