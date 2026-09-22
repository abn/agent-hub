// The project view in the one shell: an index that lists the current
// section, a stage that shows the selected item, and the section switcher in
// the index header. Every segment is its own address, so reload and the
// browser's Back both land where the reader was. The session rows and the
// gallery are rendered by the modules that own them.

import { api } from "./api.mjs";
import { artifactIndex, artifactStage, wireArtifactStage } from "./artifacts.mjs";
import { confirmProjectDelete, openCreateProjectDialog } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import {
  eventStage,
  feedEvent,
  feedSection,
  feedVisit,
  formatEventSummary,
  setFeedSelection,
} from "./feed.mjs";
import { count, usedOfCapacity } from "./home.mjs";
import { registerScreen } from "./keys.mjs";
import { pickProject } from "./projects.mjs";
import { sessionRows, sessionDetailView } from "./sessions.mjs";
import { installShellLayout, shellHTML, shellIndexControls, shellStageHead } from "./shell-layout.mjs";
import { formatBytes } from "./storage.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";


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

// The one shell. Feed, artifacts and sessions are the same four zones: an
// index that lists, a stage that shows the selected item, and an aside only
// where something is read against the stage. The section switcher lives in
// the index header, so the stage's top edge never moves between sections.

function segSwitcher(id, segment, stats) {
  const count = (n) => (n == null ? "" : `<span class="shell-seg-count">${n}</span>`);
  const tab = (seg, label, n) =>
    `<a href="#/projects/${encodeURIComponent(id)}/${seg}" role="tab" id="ah-tab-${seg}" aria-controls="ah-index" aria-selected="${
      segment === seg
    }"${segment === seg ? ' aria-current="page"' : ""}>${label}${count(n)}</a>`;
  return `<div class="shell-seg" role="tablist" aria-label="Project sections">
    ${tab("feed", "Feed", null)}
    ${tab("artifacts", "Artifacts", stats?.artifacts ?? null)}
    ${tab("sessions", "Sessions", stats?.sessions ?? null)}
  </div>`;
}

const FEED_GLYPH = `<svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><path d="M16 16l4 4"></path></svg>`;

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
    const newBtn = event.target.closest?.('[data-action="new-project"]');
    if (newBtn) {
      event.preventDefault();
      openCreateProjectDialog();
      return;
    }
    const link = event.target.closest?.(".session-link, .session-row a");
    if (link) {
      const href = link.getAttribute("href") || "";
      const match = href.match(/[?&]id=([^&]+)/);
      if (match) {
        lastOpenSession = decodeURIComponent(match[1]);
      }
    }
    if (!event.target.closest(".proj-overflow-wrap")) {
      const menus = document.querySelectorAll(".proj-overflow-menu:not([hidden])");
      for (const m of menus) {
        m.hidden = true;
        const b = m.closest(".proj-overflow-wrap")?.querySelector(".proj-overflow-btn");
        if (b) b.setAttribute("aria-expanded", "false");
      }
    }
  });
}

function wireProjectHeader(project, stats, footprint) {
  const wrap = main.querySelector(".proj-overflow-wrap");
  if (!wrap) return;
  const btn = wrap.querySelector(".proj-overflow-btn");
  const menu = wrap.querySelector(".proj-overflow-menu");
  if (!btn || !menu) return;

  const close = () => {
    menu.hidden = true;
    btn.setAttribute("aria-expanded", "false");
  };

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    const open = menu.hidden;
    menu.hidden = !open;
    btn.setAttribute("aria-expanded", open ? "true" : "false");
    if (open) {
      const first = menu.querySelector("button, a");
      if (first) first.focus();
    }
  });

  menu.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
      btn.focus();
    }
  });

  const copyBtn = menu.querySelector('button[data-action="copy-project-path"]');
  if (copyBtn) {
    copyBtn.addEventListener("click", async (e) => {
      e.stopPropagation();
      close();
      try {
        if (navigator.clipboard && navigator.clipboard.writeText) {
          await navigator.clipboard.writeText(`/p/${project.id}`);
        }
      } catch {}
      toast("Path copied");
    });
  }

  const deleteBtn = menu.querySelector('button[data-action="delete-project"]');
  if (deleteBtn) {
    deleteBtn.addEventListener("click", async (e) => {
      e.stopPropagation();
      close();
      const confirmed = await confirmProjectDelete({ project, stats, footprint });
      if (confirmed) {
        try {
          await api(`/api/v1/projects/${encodeURIComponent(project.id)}`, { method: "DELETE" });
          location.hash = "#/projects";
          toast("Project deleted.");
        } catch (err) {
          toast(`Deletion failed: ${err.message}`);
        }
      }
    });
  }
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
          <button type="button" class="projects-new" data-action="new-project">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg>New
          </button>
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

async function feedShell(id, segment, stats, params) {
  const selected = params?.get?.("event") || "";
  setFeedSelection(selected);
  const indexBody = await feedSection(id, stats, { chips: false });
  const held = feedVisit(id);
  const event = feedEvent(id, selected) || held?.events?.[0] || null;
  const title = event ? formatEventSummary(event) : "Feed";
  const meta = event ? `${event.actor} · ${relative(event.created_at)}` : "";
  return shellHTML({
    segment,
    indexHead: `<div class="shell-head">${segSwitcher(id, segment, stats)}</div>`,
    indexControls: shellIndexControls("Filter events"),
    indexBody,
    stageHead: shellStageHead(title, meta, "", `#/projects/${encodeURIComponent(id)}/feed`),
    stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(
      event ? `${id} / feed / ${event.kind}` : id,
    )}</span></div>`,
    stageBody: eventStage(event, id),
    hasSelection: Boolean(selected),
  });
}

async function artifactsShell(id, segment, stats, params) {
  const selected = params?.get?.("artifact") || "";
  const { rows, artifacts } = await artifactIndex(id, selected);
  const totalBytes = artifacts.reduce((sum, a) => sum + (Number(a.size_bytes) || 0), 0);
  const versions = artifacts.reduce((sum, a) => sum + (Number(a.version) || 1), 0);
  const listMeta = `${count(artifacts.length, "artifact", "artifacts")} · ${count(
    versions,
    "version",
    "versions",
  )} · ${formatBytes(totalBytes)}`;
  const backHref = `#/projects/${encodeURIComponent(id)}/artifacts`;

  let stageHead = shellStageHead("Artifacts", listMeta);
  let stageControls = `<div class="shell-controls"><span class="shell-meta mono">${esc(id)} / artifacts</span></div>`;
  let stageBody = `<div class="shell-pad"><p class="empty">Select an artifact from the list.</p></div>`;
  let info = null;

  if (selected) {
    info = await artifactStage(selected, id);
    if (info) {
      stageHead = shellStageHead(info.title, info.meta, info.actions, backHref);
      stageControls = `<div class="shell-controls">${info.controls}</div>`;
      stageBody = info.body;
    }
  }

  return {
    html: shellHTML({
      segment,
      indexHead: `<div class="shell-head">${segSwitcher(id, segment, stats)}</div>`,
      indexControls: shellIndexControls("Filter artifacts"),
      indexBody: rows,
      stageHead,
      stageControls,
      stageBody,
      hasSelection: Boolean(selected && info),
    }),
    info,
    selected,
  };
}

async function sessionsShell(id, segment, stats, params) {
  const selectedId = params?.get?.("id") || params?.get?.("session") || "";
  const { sessions, card } = await sessionRows(id, selectedId);
  const activeSessionId = selectedId || (sessions.length > 0 ? sessions[0].id : null);
  const selectedSession = sessions.find((s) => s.id === activeSessionId) || sessions[0] || null;
  let detailHTML = "";
  if (selectedSession) detailHTML = await sessionDetailView(id, selectedSession.id);
  const meta = selectedSession
    ? `${selectedSession.agent_name} · ${count(selectedSession.events || 0, "event", "events")} · ${formatBytes(
        selectedSession.size_bytes || 0,
      )}`
    : "";
  return shellHTML({
    segment,
    indexHead: `<div class="shell-head">${segSwitcher(id, segment, stats)}</div>`,
    indexControls: shellIndexControls("Filter sessions"),
    indexBody: card,
    stageHead: shellStageHead(selectedSession?.name || "Sessions", meta, "", `#/projects/${encodeURIComponent(id)}/sessions`),
    stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(activeSessionId || id)}</span></div>`,
    stageBody: detailHTML || `<div class="shell-pad"><p class="empty">Select a session.</p></div>`,
    hasSelection: Boolean(selectedId),
  });
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
      `<div class="projects-screen">
        <div class="projects-head">
          <div>
            <h1 class="projects-title">Projects</h1>
          </div>
          <div class="projects-acts">
            <button type="button" class="projects-new" data-action="new-project">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg>New
            </button>
          </div>
        </div>
        <div class="projects-empty">
          <h2 class="projects-empty-title">No projects yet</h2>
          <p class="projects-empty-body">A project is a folder your agents can read and write, plus the threads and artifacts that come out of it.</p>
          <button type="button" class="projects-empty-btn" data-action="new-project">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg>New project
          </button>
          <div class="projects-empty-note">Agents can also create one themselves on their first write, and it appears here.</div>
        </div>
      </div>`,
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

  let shell;
  let artifactInfo = null;
  let artifactId = "";
  if (segment === "artifacts") {
    const built = await artifactsShell(id, segment, stats, params);
    shell = built.html;
    artifactInfo = built.info;
    artifactId = built.selected;
  } else if (segment === "sessions") shell = await sessionsShell(id, segment, stats, params);
  else shell = await feedShell(id, segment, stats, params);
  if (stale(gen)) return;

  paint(gen, shell);
  installShellLayout(main);
  if (artifactInfo) wireArtifactStage(main, artifactId, artifactInfo);
  wireProjectHeader(project, stats, footprint);
}
// The row selection the keyboard map owns: the project's segments all paint
// `.row`s, so a reader walking the feed, the gallery or the list gets the map.
registerScreen("projects", { rows: ".row" });
