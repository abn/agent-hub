// The project picker every per-project screen carries, and the preamble that
// puts one in front of a screen. Also owns the Projects register.

import { api } from "./api.mjs";
import { esc, paint, stale } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { formatBytes } from "./storage.mjs";
import {
  installShellLayout,
  shellHTML,
  shellStageHead,
} from "./shell-layout.mjs";

function projectToolbar(projects, selected) {
  const options = projects
    .map(
      (p) =>
        `<option value="${esc(p.id)}"${p.id === selected ? " selected" : ""}>${esc(p.display_name)}</option>`,
    )
    .join("");
  return `<div class="toolbar"><label class="sr-only" for="project">Project</label>
    <select id="project" data-role="project">${options}</select></div>`;
}

// The projects and the one a screen is looking at: the one it was asked for,
// or the first.
export async function pickProject(selected) {
  const { projects } = await api("/api/v1/projects");
  return { projects, current: selected || (projects[0] && projects[0].id) };
}

// The preamble every per-project screen shares. Nothing to show without a
// project, so the empty state is the whole screen; otherwise the screen is
// handed the project it is on and the picker to put above itself.
export async function withProject({ selected, gen, empty }, screen) {
  const { projects, current } = await pickProject(selected);
  if (!projects.length) {
    paint(gen, empty);
    return;
  }
  await screen(current, projectToolbar(projects, current));
}

function size(bytes) {
  return formatBytes(bytes || 0);
}

const PROJECTS_REGISTER_STYLE = `<style>
.projects-tabs-track {
  flex: 1;
  max-width: 480px;
  height: 36px;
  box-sizing: border-box;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  padding: 2px;
  border-radius: var(--r-1);
  background: var(--surface-2);
}
.projects-tab-btn {
  border: 0;
  min-height: 32px;
  height: 32px;
  border-radius: 5px;
  background: none;
  color: var(--ink-2);
  font: 500 14px/1 var(--font-sans);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
}
.projects-tab-btn[aria-selected="true"] {
  background: var(--surface);
  box-shadow: var(--shadow-1);
  color: var(--ink);
  font-weight: 600;
}
.projects-tab-btn .tab-count {
  font: 500 12px/1 var(--font-mono);
  color: var(--ink-2);
}
.projects-screen {
  display: flex;
  flex-direction: column;
  background: var(--bg);
  min-height: calc(100vh + 100px);
  box-sizing: border-box;
  padding-bottom: 80px;
}
@media (min-width: 720px) {
  .projects-screen {
    min-height: 0;
    padding-bottom: 40px;
  }
}
.projects-list {
  display: flex;
  flex-direction: column;
}
.projects-list[hidden] {
  display: none !important;
}
.project-row {
  margin: 0;
  padding: 0;
  border-bottom: 1px solid var(--line);
  background: var(--surface);
}
.project-link {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 60px;
  padding: 10px 8px 10px 16px;
  color: var(--ink);
  text-decoration: none;
  width: 100%;
  box-sizing: border-box;
}
.project-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.project-title {
  font-size: var(--t-15);
  font-weight: 500;
  line-height: 1.25;
  color: var(--ink-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.project-title.unread {
  font-weight: 600;
  color: var(--ink);
}
.project-meta {
  font-size: var(--t-12);
  color: var(--ink-3);
  line-height: 1.3;
}
.project-pill {
  flex: none;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  box-sizing: border-box;
  border-radius: var(--r-pill);
  background: var(--accent-bg);
  color: var(--accent);
  font: 600 12px/22px var(--font-mono);
  text-align: center;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.project-pill.pill-waiting {
  background: var(--action-bg);
  color: var(--action);
}
.project-chev-slot {
  flex: none;
  width: 32px;
  height: 44px;
  display: grid;
  place-items: center;
  color: var(--ink-3);
}
.projects-empty-tab {
  padding: 24px 16px;
  font-size: var(--t-13);
  color: var(--ink-3);
  text-align: center;
}
</style>`;

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const tabBtn = event.target.closest?.(".projects-tab-btn");
    if (!tabBtn) return;
    const tab = tabBtn.dataset.tab;
    const root = tabBtn.closest(".shell");
    if (!root) return;
    root.querySelectorAll(".projects-tab-btn").forEach((btn) => {
      const isSelected = btn.dataset.tab === tab;
      btn.setAttribute("aria-selected", isSelected ? "true" : "false");
    });
    const regularList = root.querySelector(".projects-list-regular");
    const spacesList = root.querySelector(".projects-list-spaces");
    if (regularList) regularList.hidden = tab !== "projects";
    if (spacesList) spacesList.hidden = tab !== "spaces";
  });
}

// The desktop register's filter (RULE 11.14): it hides non-matching project rows
// and replaces the count with one mono line, the same shape the index filter
// uses. The register has no index pane, so it filters its own lists.
function wireProjectFilter() {
  const field = document.querySelector("[data-action='projects-filter']");
  if (!field || field.dataset.wired === "on") return;
  field.dataset.wired = "on";
  const screen = document.querySelector(".projects-screen");
  const count = document.createElement("div");
  count.className = "shell-count mono";
  count.hidden = true;
  screen?.prepend(count);
  const apply = () => {
    const q = field.value.trim().toLowerCase();
    const rows = [...document.querySelectorAll(".projects-list .project-row")];
    let shown = 0;
    for (const row of rows) {
      const match = !q || row.textContent.toLowerCase().includes(q);
      row.hidden = !match;
      if (match) shown++;
    }
    for (const list of document.querySelectorAll(".projects-list")) {
      const any = [...list.querySelectorAll(".project-row:not([hidden])")].length > 0;
      list.hidden = !any;
    }
    if (q) {
      count.textContent = `${shown} of ${rows.length} match "${field.value.trim()}"`;
      count.hidden = false;
    } else {
      count.hidden = true;
    }
  };
  field.addEventListener("input", apply);
}

export async function projectsIndexScreen(gen, passedProjects) {
  let projects = passedProjects;
  if (!projects) {
    const res = await api("/api/v1/projects").catch(() => ({ projects: [] }));
    projects = res.projects || [];
  }

  const newProjectBtn = `<button type="button" class="btn-glyph projects-new" data-action="new-project" aria-label="New project"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg></button>`;

  if (!projects.length) {
    paint(
      gen,
      PROJECTS_REGISTER_STYLE +
        shellHTML({
          noIndex: true,
          stageHead: shellStageHead("Projects", "", newProjectBtn),
          stageControls: `<div class="shell-controls"></div>`,
          stageBody: `
            <div class="projects-screen">
              <div class="projects-empty" style="padding: 24px 16px;">
                <h2 class="projects-empty-title">No projects yet</h2>
                <p class="projects-empty-body">A project is a folder your agents can read and write, plus the threads and artifacts that come out of it.</p>
                <button type="button" class="projects-empty-btn" data-action="new-project">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg>New project
                </button>
                <div class="projects-empty-note" style="margin-top: 12px; font-size: 13px; color: var(--ink-3);">Agents can also create one themselves on their first write, and it appears here.</div>
              </div>
            </div>`,
        }),
    );
    installShellLayout(document.querySelector(".shell"));
    return;
  }

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

  // The hub marks a personal space on the project payload; the id prefix is
  // not a contract to read it from.
  const isPersonal = (p) => p.is_personal === true;
  const regularProjects = projects.filter((p) => !isPersonal(p));
  const agentSpaces = projects.filter(isPersonal);

  const totalUsedBytes = storage?.used_bytes ?? 0;
  const totalFootprint = size(totalUsedBytes);
  const anyActive = projects.some((p) => (statsMap.get(p.id)?.agents_active || 0) > 0);
  const headerMeta = !anyActive
    ? `${projects.length} · ${totalFootprint} · no agents active`
    : `${projects.length} · ${totalFootprint}`;

  const renderRow = (p) => {
    const stats = statsMap.get(p.id);
    const usage = usageMap.get(p.id);

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
      badgeHTML = `<span class="project-badge project-pill pill-waiting" data-kind="action" aria-label="${waitingCount} waiting on you">${waitingCount}</span>`;
    } else if (hasUnread) {
      badgeHTML = `<span class="project-badge project-pill pill-unread" data-kind="unread" aria-label="${unreadCount} unread">${unreadCount}</span>`;
    }

    const titleClass = hasWaiting || hasUnread ? "project-title unread" : "project-title";
    const lockMeta = p.confidential
      ? `<span class="project-lock" role="img" aria-label="confidential" style="display:inline-flex;align-items:center;gap:4px;margin-right:6px;color:var(--ink-2)">${glyphSvg("lock", { size: 16 })}<span>confidential</span></span>`
      : "";

    return `
      <div class="row project-row" data-id="${esc(p.id)}">
        <a class="project-link" href="#/projects/${encodeURIComponent(p.id)}/feed">
          <span class="project-info">
            <span class="title ${titleClass}">${esc(p.display_name)}</span>
            <span class="meta project-meta">${lockMeta}${artifactsText} · <span class="mono">${footprintText}</span></span>
          </span>
          ${badgeHTML}
          <span class="project-chev-slot" aria-hidden="true">
            <svg class="project-chev" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 9 6l6 6-6 6"/></svg>
          </span>
        </a>
      </div>`;
  };

  const regularRows = regularProjects.length
    ? regularProjects.map(renderRow).join("")
    : `<div class="projects-empty-tab">No projects yet</div>`;

  const spaceRows = agentSpaces.length
    ? agentSpaces.map(renderRow).join("")
    : `<div class="projects-empty-tab">No agent spaces yet</div>`;

  const bodyHTML = `
    <div class="projects-screen">
      <div class="projects-list projects-list-regular">
        ${regularRows}
      </div>
      <div class="projects-list projects-list-spaces" hidden>
        ${spaceRows}
      </div>
    </div>`;

  // The register lists every project on one screen, so the segmented switcher is
  // phone chrome. On a desktop the control row carries the filter field once the
  // list is over eight rows (RULE 11.14); the phone keeps the switcher, because
  // its control row is the only place the two lists can be told apart. Round 12
  // drops the "Agent spaces n" footer line at both widths.
  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  const tabsHTML = `
    <div class="projects-tabs-track" role="tablist" aria-label="Project filter">
      <button type="button" role="tab" class="projects-tab-btn" data-tab="projects" aria-selected="true">
        Projects <span class="tab-count mono">${regularProjects.length}</span>
      </button>
      <button type="button" role="tab" class="projects-tab-btn" data-tab="spaces" aria-selected="false">
        Agent spaces <span class="tab-count mono">${agentSpaces.length}</span>
      </button>
    </div>`;
  const filterHTML = `<div class="shell-controls">${
    isDesktop && projects.length > 8
      ? `<label class="shell-filter">
           <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><path d="M16 16l4 4"></path></svg>
           <input type="search" data-action="projects-filter" placeholder="Filter projects" aria-label="Filter projects">
         </label>`
      : isDesktop
        ? ""
        : tabsHTML
  }</div>`;


  paint(
    gen,
    PROJECTS_REGISTER_STYLE +
      shellHTML({
        noIndex: true,
        stageHead: shellStageHead("Projects", headerMeta, newProjectBtn),
        stageControls: filterHTML,
        stageBody: bodyHTML,
      }),
  );
  installShellLayout(document.querySelector(".shell"));
  wireProjectFilter();
}
