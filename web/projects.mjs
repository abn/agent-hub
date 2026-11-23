// The project picker every per-project screen carries, and the preamble that
// puts one in front of a screen. Also owns the Projects register.

import { api } from "./api.mjs";
import { beginRender, esc, paint, stale } from "./dom.mjs";
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

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const tabBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.(".projects-tab-btn"));
    if (!tabBtn) return;
    const tab = tabBtn.dataset.tab;
    const root = tabBtn.closest(".shell");
    if (!root) return;
    root.querySelectorAll(".projects-tab-btn").forEach((btn) => {
      const isSelected = /** @type {HTMLElement} */ (btn).dataset.tab === tab;
      btn.setAttribute("aria-selected", isSelected ? "true" : "false");
    });
    const regularList = /** @type {HTMLElement} */ (root.querySelector(".projects-list-regular"));
    const spacesList = /** @type {HTMLElement} */ (root.querySelector(".projects-list-spaces"));
    if (regularList) regularList.hidden = tab !== "projects";
    if (spacesList) spacesList.hidden = tab !== "spaces";
  });
}

// The register's filter (RULE 11.14): it hides non-matching project rows and
// replaces the count with one mono line, the same shape the index filter uses.
// The register has no index pane, so it filters its own lists. On a phone the
// field takes the tab switcher's place while it is open, and a live query
// searches projects and agent spaces together.
function wireProjectFilter() {
  const field = /** @type {any} */ (document.querySelector("[data-action='projects-filter']"));
  if (!field || field.dataset.wired === "on") return;
  field.dataset.wired = "on";
  const screen = document.querySelector(".projects-screen");
  let count = /** @type {HTMLElement | null} */ (screen?.querySelector(":scope > .shell-count") ?? null);
  if (!count) {
    count = document.createElement("div");
    count.className = "shell-count mono";
    count.hidden = true;
    screen?.prepend(count);
  }
  const apply = () => {
    const q = field.value.trim().toLowerCase();
    const rows = [...document.querySelectorAll(".projects-list .project-row")];
    let shown = 0;
    for (const row of rows) {
      const match = !q || row.textContent.toLowerCase().includes(q);
      /** @type {HTMLElement} */ (row).hidden = !match;
      if (match) shown++;
    }
    // The switcher leaves the row while the field is open, so it is read from
    // where it is parked. With no query the selected tab decides what shows,
    // even when that tab holds no rows; a query looks across both lists.
    const tabbed = document.querySelector(".projects-tabs-track") ?? parked;
    const tab = tabbed?.querySelector('[aria-selected="true"]')?.getAttribute("data-tab");
    for (const list of document.querySelectorAll(".projects-list")) {
      const isSpaces = list.classList.contains("projects-list-spaces");
      const any = list.querySelectorAll(".project-row:not([hidden])").length > 0;
      const off = tabbed ? isSpaces !== (tab === "spaces") : false;
      /** @type {HTMLElement} */ (list).hidden = q ? !any : off;
    }
    for (const label of document.querySelectorAll(".projects-section-label")) {
      const list = label.nextElementSibling;
      /** @type {HTMLElement} */ (label).hidden = !list || /** @type {HTMLElement} */ (list).hidden;
    }
    if (q) {
      count.textContent = `${shown} of ${rows.length} match "${field.value.trim()}"`;
      count.hidden = false;
    } else {
      count.hidden = true;
    }
  };
  field.addEventListener("input", apply);
  field.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeProjectFilter();
  });
  field._apply = apply;
}

/** @type {Element | null} */
let parked = null;

function closeProjectFilter() {
  const toggle = /** @type {HTMLElement | null} */ (document.querySelector("[data-action='projects-filter-toggle']"));
  const field = /** @type {any} */ (document.querySelector("[data-action='projects-filter']"));
  if (!toggle || !field) return;
  const wrap = field.closest(".shell-filter");
  field.value = "";
  field._apply?.();
  // The field and the switcher take turns in the row: a field's own display
  // beats [hidden], so it is made on opening and removed on closing.
  if (parked && wrap) wrap.before(parked);
  wrap?.remove();
  parked = null;
  toggle.setAttribute("aria-expanded", "false");
  toggle.focus();
}

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const toggle = /** @type {HTMLElement} */ (event.target).closest?.("[data-action='projects-filter-toggle']");
    if (!toggle) return;
    if (toggle.getAttribute("aria-expanded") === "true") {
      closeProjectFilter();
      return;
    }
    const template = /** @type {HTMLTemplateElement | null} */ (document.querySelector("template[data-role='projects-filter']"));
    if (!template) return;
    const tabs = document.querySelector(".projects-tabs-track");
    template.before(template.content.cloneNode(true));
    parked = tabs;
    tabs?.remove();
    wireProjectFilter();
    toggle.setAttribute("aria-expanded", "true");
    /** @type {HTMLInputElement | null} */ (document.querySelector("[data-action='projects-filter']"))?.focus();
  });
}

// The register is drawn for one width, so crossing the breakpoint draws it
// again for the other, in both directions.
if (typeof window !== "undefined" && window.matchMedia) {
  const mq = window.matchMedia("(min-width: 720px)");
  mq.addEventListener?.("change", () => {
    const path = location.hash.split("?")[0];
    if (path === "#/projects" || path === "#/projects/") {
      projectsIndexScreen(beginRender());
    }
  });
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
      shellHTML({
          noIndex: true,
          stageHead: shellStageHead("Projects", "", newProjectBtn),
          stageControls: `<div class="shell-controls"></div>`,
          stageBody: `
            <div class="projects-screen">
              <div class="projects-empty">
                <h2 class="projects-empty-title">No projects yet</h2>
                <p class="projects-empty-body">A project is a folder your agents can read and write, plus the threads and artifacts that come out of it.</p>
                <button type="button" class="projects-empty-btn" data-action="new-project">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg>New project
                </button>
                <div class="projects-empty-note projects-register-note">Agents can also create one themselves on their first write, and it appears here.</div>
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
      ? `<span class="project-lock project-lock-meta" role="img" aria-label="confidential">${glyphSvg("lock", { size: 16 })}<span>confidential</span></span>`
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

  // A desktop has the room for both lists under their own labels; a phone
  // splits them with a segmented switcher in its tools row.
  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  const spacesLabel =
    isDesktop && agentSpaces.length
      ? `<div class="access-section-label projects-section-label">Agent spaces</div>`
      : "";
  const bodyHTML = `
    <div class="projects-screen">
      <div class="projects-list projects-list-regular">
        ${regularRows}
      </div>
      ${spacesLabel}
      <div class="projects-list projects-list-spaces"${isDesktop ? "" : " hidden"}>
        ${isDesktop && !agentSpaces.length ? "" : spaceRows}
      </div>
    </div>`;

  const search = `<svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><path d="M16 16l4 4"></path></svg>`;
  const filterField = () => `<label class="shell-filter">
           ${search}
           <input type="search" data-action="projects-filter" placeholder="Filter projects" aria-label="Filter projects">
         </label>`;
  const filterToggle = `<button type="button" class="project-filter-btn" data-action="projects-filter-toggle" aria-label="Filter projects" aria-expanded="false">${search.replace('width="14" height="14"', 'width="16" height="16"')}</button>`;
  const tabsHTML = `
    <div class="projects-tabs-track" role="tablist" aria-label="Project filter">
      <button type="button" role="tab" class="projects-tab-btn" data-tab="projects" aria-selected="true">
        Projects <span class="tab-count mono">${regularProjects.length}</span>
      </button>
      <button type="button" role="tab" class="projects-tab-btn" data-tab="spaces" aria-selected="false">
        Agent spaces <span class="tab-count mono">${agentSpaces.length}</span>
      </button>
    </div>`;
  const many = projects.length > 8;
  const controlsInner = isDesktop
    ? many
      ? filterField()
      : ""
    : tabsHTML + (many ? `<template data-role="projects-filter">${filterField()}</template>${filterToggle}` : "");
  const filterHTML = `<div class="shell-controls">${controlsInner}</div>`;

  paint(
    gen,
    shellHTML({
        noIndex: true,
        stageHead: shellStageHead("Projects", headerMeta, newProjectBtn),
        stageControls: filterHTML,
        stageBody: bodyHTML,
      }),
  );
  installShellLayout(document.querySelector(".shell"));
  parked = null;
  wireProjectFilter();
}
