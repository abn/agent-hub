// The project view: the header, the segmented Feed / Artifacts / Sessions
// tabs, and the segment content each tab owns. Every segment is its own
// address, so reload and the browser's Back both land where the reader was.
// The kind-chip filter, the session rows and the gallery are rendered by the
// modules that own them; this module composes them inside the segmented shell.

import { api } from "./api.mjs";
import { gallerySection } from "./artifacts.mjs";
import { esc, paint, stale } from "./dom.mjs";
import { emptyStateHTML } from "./empty.mjs";
import { feedSection } from "./feed.mjs";
import { count, usedOfCapacity } from "./home.mjs";
import { registerScreen } from "./keys.mjs";
import { pickProject } from "./projects.mjs";
import { settingsLink } from "./project-settings.mjs";
import { sessionRows, sessionDetailView, wireSessionDetail } from "./sessions.mjs";

const SEGMENTS = ["feed", "artifacts", "sessions"];

// The id and the segment a `#/projects/<id>/<segment>` hash names. The segment
// defaults to the feed, so `#/projects/<id>` is the project's feed.
export function segmentOf(hash) {
  const parts = (hash.replace(/^#/, "").split("?")[0] || "").split("/");
  let seg = parts[3];
  if (seg === "artifact") seg = "artifacts";
  if (seg === "session") seg = "sessions";
  const segment = SEGMENTS.includes(seg) ? seg : "feed";
  return { id: parts[2] || "", segment };
}

export function projectFromHash() {
  return segmentOf(location.hash).id;
}

// The desktop list plus detail layout: one 420px list pane beside a detail
// pane, stacking on the phone. A screen opts in by composing its own panes;
// this module just wraps them and the media rule owns the split.
export function twoPane(listHTML, detailHTML) {
  return `<div class="panes"><div class="pane-list">${listHTML}</div><div class="pane-detail">${detailHTML}</div></div>`;
}

function size(bytes) {
  if (!bytes) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

// A project's own footprint from the storage response, so the header's size
// is what the hub reports rather than a sum the client invents.
async function projectFootprint(projectId) {
  try {
    const usage = await api("/api/v1/storage");
    const row = usage.projects.find((p) => p.project_id === projectId);
    return row ? size(row.artifact_bytes + row.session_bytes + row.kb_bytes) : null;
  } catch {
    return null;
  }
}

function header(project, stats, footprint) {
  const agents = stats && stats.agents_active != null ? `${count(stats.agents_active, "agent", "agents")} active` : "";
  const used = footprint ? ` · <span class="mono">${footprint}</span>` : "";
  const detail = agents || footprint ? `<p class="proj-stats">${agents}${used}</p>` : "";
  return `
    <div class="proj-head">
      <button type="button" class="proj-back" data-action="project-back" aria-label="Back to projects">
        <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 15 6l-6 6 6 6"></path></svg>
      </button>
      <div class="grow">
        <h1>${esc(project.display_name)}</h1>
        ${detail}
      </div>
      ${settingsLink(project.id)}
    </div>`;
}

function tabs(current, segment, stats) {
  const artifactCount = stats && stats.artifacts != null ? ` · ${stats.artifacts}` : "";
  const sessionCount = stats && stats.sessions != null ? ` · ${stats.sessions}` : "";
  return `
    <div class="seg" role="group" aria-label="Project sections">
      <a href="#/projects/${encodeURIComponent(current)}/feed"${segment === "feed" ? ' aria-current="page"' : ""}>Feed</a>
      <a href="#/projects/${encodeURIComponent(current)}/artifacts"${segment === "artifacts" ? ' aria-current="page"' : ""}>Artifacts${artifactCount}</a>
      <a href="#/projects/${encodeURIComponent(current)}/sessions"${segment === "sessions" ? ' aria-current="page"' : ""}>Sessions${sessionCount}</a>
    </div>`;
}

let lastOpenSession = "";

function returnSessionFocus(id) {
  requestAnimationFrame(() => {
    const link = document.querySelector(`.session-row a[href*="id=${CSS.escape(id)}"]`);
    const row = link ? link.closest(".session-row") : null;
    const target = row || link;
    if (target) {
      if (target.tabIndex < 0) target.tabIndex = 0;
      target.focus();
    }
  });
}

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const link = event.target.closest?.(".session-link, .session-row a");
    if (link) {
      const href = link.getAttribute("href") || "";
      const match = href.match(/[?&]id=([^&]+)/);
      if (match) {
        lastOpenSession = decodeURIComponent(match[1]);
      }
    }
  });
}

// The sessions segment on desktop: the list in the 420px pane and the session
// detail in the other. On a phone the detail pane is hidden by media query and
// the card replaces the list when opened, returning focus to the row when closed.
function sessionsTwoPane(current, listHTML, sessions, selectedSession, detailHTML, hasSelection) {
  const detail = selectedSession
    ? (detailHTML || `<p class="empty">Loading session…</p>`)
    : `<p class="empty">No sessions yet.</p>`;
  const paneClass = hasSelection ? "panes has-selection" : "panes";
  return `<div class="${paneClass}"><div class="pane-list">${listHTML}</div><div class="pane-detail">${detail}</div></div>`;
}

async function projectsIndexScreen(gen, projects) {
  const [statsList, storage, inboxData, homeData] = await Promise.all([
    Promise.all(
      projects.map((p) =>
        api(`/api/v1/projects/${encodeURIComponent(p.id)}/stats`).catch(() => null),
      ),
    ),
    api("/api/v1/storage").catch(() => null),
    api("/api/v1/inbox?limit=500").catch(() => ({ items: [] })),
    api("/api/v1/home").catch(() => null),
  ]);

  if (stale(gen)) return;

  const statsMap = new Map(projects.map((p, i) => [p.id, statsList[i]]));
  const usageMap = new Map((storage?.projects || []).map((row) => [row.project_id, row]));

  const waitingMap = new Map();
  const unreadMap = new Map();
  for (const item of inboxData.items || []) {
    if (!item.project_id) continue;
    if (item.status === "action" || item.status === "waiting") {
      waitingMap.set(item.project_id, (waitingMap.get(item.project_id) || 0) + 1);
    } else if (item.status === "unread") {
      unreadMap.set(item.project_id, (unreadMap.get(item.project_id) || 0) + 1);
    }
  }

  const unseenMap = new Map((homeData?.unseen || []).map((u) => [u.project_id, u.events || 0]));

  const isPersonal = (p) => p.id.startsWith("space-") || p.display_name.endsWith(" (personal)");
  const regularProjects = projects.filter((p) => !isPersonal(p));
  const agentSpaces = projects.filter(isPersonal);

  const renderRow = (p) => {
    const stats = statsMap.get(p.id);
    const usage = usageMap.get(p.id);

    const agentsActive = stats?.agents_active || 0;
    const agentsText =
      agentsActive === 0
        ? "no agents active"
        : agentsActive === 1
          ? "1 agent active"
          : `${agentsActive} agents active`;

    const artifactsCount = stats?.artifacts || 0;
    const artifactsText =
      artifactsCount === 0
        ? "no artifacts"
        : artifactsCount === 1
          ? "1 artifact"
          : `${artifactsCount} artifacts`;

    const footprintBytes = usage
      ? (usage.artifact_bytes || 0) + (usage.session_bytes || 0) + (usage.kb_bytes || 0)
      : 0;
    const footprintText = size(footprintBytes);

    const waitingCount = waitingMap.get(p.id) || 0;
    const unreadCount = unreadMap.get(p.id) || unseenMap.get(p.id) || 0;

    let badgeHTML = "";
    const hasWaiting = waitingCount > 0;
    const hasUnread = !hasWaiting && unreadCount > 0;
    if (hasWaiting) {
      badgeHTML = `<span class="project-badge badge-waiting" data-kind="action" aria-label="${waitingCount} waiting on you">${waitingCount}</span>`;
    } else if (hasUnread) {
      badgeHTML = `<span class="project-badge badge-unread" data-kind="unread" aria-label="${unreadCount} unread">${unreadCount}</span>`;
    }

    const titleClass = hasWaiting || hasUnread ? "project-title unread" : "project-title";

    return `
      <div class="row project-row" data-id="${esc(p.id)}">
        <a class="project-link" href="#/projects/${encodeURIComponent(p.id)}/feed">
          <span class="project-info">
            <span class="title ${titleClass}">${esc(p.display_name)}</span>
            <span class="meta project-meta">${agentsText} · ${artifactsText} · <span class="mono">${footprintText}</span></span>
          </span>
          ${badgeHTML}
          <svg class="project-chev" aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 9 5l7 7-7 7"/></svg>
        </a>
      </div>`;
  };

  const regularRows = regularProjects.map(renderRow).join("");

  let spacesHTML = "";
  if (agentSpaces.length > 0) {
    spacesHTML = `
      <details class="projects-agent-spaces">
        <summary class="projects-agent-spaces-summary">
          <span>Agent spaces · ${agentSpaces.length}</span>
        </summary>
        <div class="projects-agent-spaces-list">
          ${agentSpaces.map(renderRow).join("")}
        </div>
      </details>`;
  }

  const totalUsedBytes = storage?.used_bytes ?? 0;
  const totalFootprint = size(totalUsedBytes);
  const countWord = `${count(projects.length, "project", "projects")}`;

  let storageSection = "";
  if (storage) {
    const used = storage.used_bytes || 0;
    const capacity = storage.capacity_bytes || 0;
    const share = capacity ? Math.min(1, used / capacity) : 0;
    const prunableSessions = storage.prunable?.sessions || 0;
    const prunableBytes = storage.prunable?.bytes || 0;

    let pruneHint = "";
    if (prunableSessions > 0) {
      pruneHint = `<div class="projects-storage-hint">${count(prunableSessions, "ended session", "ended sessions")} can be pruned · <span class="mono">${size(prunableBytes)}</span></div>`;
    }

    storageSection = `
      <section class="projects-storage" aria-label="Storage">
        <div class="projects-storage-label">STORAGE</div>
        <div class="card projects-storage-card">
          <div class="projects-storage-head">
            <span class="projects-storage-title">Disk</span>
            <span class="mono projects-storage-capacity">${capacity ? usedOfCapacity(used, capacity) : size(used)}</span>
          </div>
          <div class="projects-storage-bar" aria-hidden="true">
            <div class="projects-storage-fill" style="width: ${(share * 100).toFixed(1)}%"></div>
          </div>
          ${pruneHint}
        </div>
      </section>`;
  }

  const html = `
    <div class="projects-screen">
      <div class="projects-head">
        <div>
          <h1 class="projects-title">Projects</h1>
          <div class="projects-meta">${countWord} · <span class="mono">${totalFootprint}</span></div>
        </div>
        <div class="projects-acts">
          <a href="#/settings" class="projects-new">New</a>
          <a href="#/settings" class="projects-gear" aria-label="Settings">
            <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M 19 12a7 7 0 0 0-.2-1.6l2-1.5-2-3.4-2.3 1a7 7 0 0 0-2.8-1.6L13.3 2h-2.6l-.4 2.9a7 7 0 0 0-2.8 1.6l-2.3-1-2 3.4 2 1.5A7 7 0 0 0 5 12c0 .5.1 1.1.2 1.6l-2 1.5 2 3.4 2.3-1a7 7 0 0 0 2.8 1.6l.4 2.9h2.6l.4-2.9a7 7 0 0 0 2.8-1.6l2.3 1 2-3.4-2-1.5c.1-.5.2-1.1.2-1.6z"></path></svg>
          </a>
        </div>
      </div>
      <div class="projects-list">
        ${regularRows}
        ${spacesHTML}
      </div>
      ${storageSection}
    </div>`;

  paint(gen, html);
}

export async function projectScreen(params, gen, path) {
  const parts = (path || "").split("/");
  let seg = parts[3];
  if (seg === "artifact") seg = "artifacts";
  if (seg === "session") seg = "sessions";
  const segment = SEGMENTS.includes(seg) ? seg : "feed";
  const { projects, current } = await pickProject(parts[2]);
  if (stale(gen)) return;
  if (!projects.length) {
    paint(
      gen,
      `<h1>Projects</h1>${emptyStateHTML({
        screen: "projects",
        title: "No projects yet.",
        body: "Create one in Settings to give agents a home.",
      })}`,
    );
    return;
  }
  const id = parts[2];
  if (!id) {
    await projectsIndexScreen(gen, projects);
    return;
  }
  if (!projects.some((p) => p.id === id)) {
    // The address named a project that is gone. Land on the first
    // project, keeping the segment the reader asked for.
    location.hash = `#/projects/${encodeURIComponent(current)}/${segment}`;
    return;
  }
  const kindParam = params?.get("kind") || params?.get("kinds");
  if (kindParam === "artifact") {
    location.hash = `#/projects/${encodeURIComponent(id)}/artifacts`;
    return;
  }
  if (kindParam === "session") {
    location.hash = `#/projects/${encodeURIComponent(id)}/sessions`;
    return;
  }
  const project = projects.find((p) => p.id === id);
  let stats = null;
  try {
    stats = await api(`/api/v1/projects/${encodeURIComponent(id)}/stats`);
  } catch {}
  const footprint = await projectFootprint(id);
  if (stale(gen)) return;

  const shell = `${header(project, stats, footprint)}${tabs(id, segment, stats)}`;
  if (segment === "artifacts") {
    paint(gen, `${shell}${await gallerySection(id)}`);
  } else if (segment === "sessions") {
    const selectedId = params?.get?.("id") || params?.get?.("session");
    const { sessions, card } = await sessionRows(id, selectedId);
    if (stale(gen)) return;
    const activeSessionId = selectedId || (sessions.length > 0 ? sessions[0].id : null);
    const selectedSession = sessions.find((s) => s.id === activeSessionId) || sessions[0] || null;
    let detailHTML = "";
    if (selectedSession) {
      detailHTML = await sessionDetailView(id, selectedSession.id, gen);
    }
    if (stale(gen)) return;
    paint(gen, `${shell}${sessionsTwoPane(id, card, sessions, selectedSession, detailHTML, !!selectedId)}`);
    if (selectedSession) {
      wireSessionDetail(main.querySelector(".pane-detail") || main, id, selectedSession.id);
    }
    if (lastOpenSession) {
      const closed = lastOpenSession;
      lastOpenSession = "";
      returnSessionFocus(closed);
    }
  } else {
    paint(gen, `${shell}${await feedSection(id, stats)}`);
  }
}

// The row selection the keyboard map owns: the project's segments all paint
// `.row`s, so a reader walking the feed, the gallery or the list gets the map.
registerScreen("projects", { rows: ".row" });
