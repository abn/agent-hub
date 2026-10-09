// The project view in the one shell: an index that lists the current
// section, a stage that shows the selected item, and the section switcher in
// the index header. Every segment is its own address, so reload and the
// browser's Back both land where the reader was. The session rows and the
// gallery are rendered by the modules that own them.

import { api } from "./api.mjs";
import { artifactGroupMenu, artifactIndex, artifactStage, wireArtifactStage } from "./artifacts.mjs";
import { confirmAction, confirmProjectDelete, openCreateProjectDialog } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import {
  activeKinds,
  eventStage,
  feedCounts,
  feedEvent,
  feedSection,
  feedVisit,
  formatEventSummary,
  kindMenu,
  setFeedSelection,
} from "./feed.mjs";
import { count, usedOfCapacity } from "./home.mjs";
import { registerScreen } from "./keys.mjs";
import { render } from "./router.mjs";
import { pickProject, projectsIndexScreen } from "./projects.mjs";
import { wikiIndexBody, wikiStage } from "./wiki.mjs";
export { projectsIndexScreen } from "./projects.mjs";
import {
  errorAsideHTML,
  fetchBrainEntry,
  kvAsideHTML,
  kvSheetHTML,
  renderMarkdown,
  sessionDetailView,
  sessionRows,
  wireSessionDetail,
} from "./sessions.mjs";
import {
  installShellLayout,
  shellHTML,
  shellIndexControls,
  shellMobileBar,
  shellStageHead,
} from "./shell-layout.mjs";
import { formatBytes } from "./storage.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";


const SEGMENTS = ["feed", "wiki", "artifacts", "sessions"];

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

export function projectLockBadge(project) {
  if (!project || !project.confidential) return "";
  return `<span class="project-lock" role="img" aria-label="confidential" style="display:inline-flex;align-items:center;gap:4px;flex:none;margin-right:8px;color:var(--ink-2);font-size:12px;white-space:nowrap">${glyphSvg("lock", { size: 16 })}<span>confidential</span></span>`;
}

// The project's own actions, in the header overflow the design draws. The
// project header carries Project settings, Copy path and Delete project; the
// project's own settings screen passes lock-only and gets the confidential
// control instead. An operator may lock or unlock from there.
export function projectOverflow(project, { lock = true, full = true, newPage = false } = {}) {
  const name = project.display_name || project.id;
  const lockItem = !lock
    ? ""
    : project.confidential
      ? `<button type="button" role="menuitem" class="proj-menu-item" data-action="project-make-public">Make public…</button>`
      : `<button type="button" role="menuitem" class="proj-menu-item" data-action="project-make-confidential">Make confidential…</button>`;
  const newPageItem = newPage
    ? `<button type="button" role="menuitem" class="proj-menu-item" data-action="wiki-new" data-id="${esc(project.id)}" data-project="${esc(name)}">New page</button>`
    : "";
  const extras = full
    ? `<a role="menuitem" class="proj-menu-item" href="#/projects/${encodeURIComponent(project.id)}/settings">Project settings</a>
      <button type="button" role="menuitem" class="proj-menu-item" data-action="copy-project-path">Copy path</button>
      <div class="proj-menu-divider" role="separator"></div>
      <button type="button" role="menuitem" class="proj-menu-item proj-menu-delete" data-action="delete-project">Delete project</button>`
    : "";
  void name;
  return `<span class="proj-overflow-wrap">
    <button type="button" class="proj-overflow-btn" aria-label="Project actions" aria-haspopup="menu" aria-expanded="false">${glyphSvg("overflow", { size: 20 })}</button>
    <div class="proj-overflow-menu" role="menu" hidden>
      ${newPageItem}
      ${lockItem}
      ${extras}
    </div>
  </span>`;
}

function segSwitcher(id, segment, stats) {
  const count = (n) => (n == null ? "" : `<span class="shell-seg-count">${n}</span>`);
  const tab = (seg, label, n) =>
    `<a href="#/projects/${encodeURIComponent(id)}/${seg}" role="tab" id="ah-tab-${seg}" aria-controls="ah-index" aria-selected="${
      segment === seg
    }"${segment === seg ? ' aria-current="page"' : ""}>${label}${count(n)}</a>`;
  return `<div class="shell-seg" role="tablist" aria-label="Project sections">
    ${tab("feed", "Feed", null)}
    ${tab("wiki", "Wiki", stats?.kb_pages ?? null)}
    ${tab("artifacts", "Artifacts", stats?.artifacts ?? null)}
    ${tab("sessions", "Sessions", stats?.sessions ?? null)}
  </div>`;
}

const PROJECT_MOBILE_STYLE = `<style>
@media (max-width: 1099px) {
  /* Only the desktop segmented switcher's wrapper hides on a phone. The phone
     header is also a .shell-head inside .shell-index, so the rule must name the
     switcher (project-seg-head), not every .shell-head, or it takes the header
     with it. */
  .shell-index .shell-head.project-seg-head { display: none !important; }
  /* The chips row is the filter panel: hidden until the filter glyph opens it.
     The tools row carries .shell-controls too now, so it is excluded here and
     stays visible. */
  .shell-index .shell-controls:not(.project-tools-mobile):not(.mobile-filter-open) { display: none !important; }
  .shell-index .shell-controls.mobile-filter-open { display: flex !important; }
  /* RULE 12.2: this is the project's tools row, so it carries .shell-controls
     and the framework owns its height, its sticky offset (76px at rest, 52px
     compressed) and its z-index. Pinning it at a hardcoded 52px put it 24px
     inside the 76px header at rest, which is the band that floated over the
     feed. */
  .project-tools-mobile {
    display: flex !important;
    padding: 0 16px;
    gap: 8px;
    min-width: 0;
  }
  /* The switcher takes what is left and scrolls sideways rather than squeezing
     its tabs into collisions; the tabs keep their drawn width. The 44px tools
     row then always holds the filter and the overflow inside 390. */
  .project-tools-mobile > .project-tools-seg {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    height: 36px;
    background: var(--surface-2);
    border-radius: var(--r-1);
    padding: 2px;
    box-sizing: border-box;
    align-items: center;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .project-tools-mobile > .project-tools-seg::-webkit-scrollbar {
    display: none;
  }
  .project-tools-seg a {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    height: 100%;
    padding: 0 8px;
    font-size: 14px;
    font-weight: 500;
    color: var(--ink-2);
    text-decoration: none;
    border-radius: 5px;
    white-space: nowrap;
  }
  /* The tabs are 32px drawn inside the 36px switcher, so under a coarse pointer
     each one grows to a 44px target with the same pseudo-element expansion the
     chips and glyph buttons use. */
  @media (pointer: coarse) {
    .project-tools-seg a::after {
      content: "";
      position: absolute;
      left: 0;
      right: 0;
      top: 50%;
      height: 44px;
      transform: translateY(-50%);
    }
  }
  .project-tools-seg a[aria-selected="true"],
  .project-tools-seg a[aria-current="page"] {
    background: var(--surface);
    color: var(--ink);
    font-weight: 600;
    box-shadow: var(--shadow-1);
  }
  .project-tools-seg .shell-seg-count {
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 500;
    color: var(--ink-2);
  }
  .project-filter-btn {
    width: 44px;
    height: 44px;
    min-width: 44px;
    min-height: 44px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    padding: 0;
    color: var(--ink-2);
    cursor: pointer;
    border-radius: var(--r-1);
    flex: none;
  }
  .project-filter-btn:hover,
  .project-filter-btn:focus-visible {
    color: var(--ink);
  }
}
@media (min-width: 1100px) {
  .project-tools-mobile {
    display: none !important;
  }
}
</style>`;

function projectToolsMobile(id, segment, stats, project) {
  const count = (n) => (n == null ? "" : `<span class="shell-seg-count">${n}</span>`);
  const tab = (seg, label, n) =>
    `<a href="#/projects/${encodeURIComponent(id)}/${seg}" role="tab" aria-selected="${
      segment === seg
    }"${segment === seg ? ' aria-current="page"' : ""}>${label}${count(n)}</a>`;
  const filterGlyph = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="4" y1="21" x2="4" y2="14"></line><line x1="4" y1="10" x2="4" y2="3"></line><line x1="12" y1="21" x2="12" y2="12"></line><line x1="12" y1="8" x2="12" y2="3"></line><line x1="20" y1="21" x2="20" y2="16"></line><line x1="20" y1="12" x2="20" y2="3"></line><line x1="1" y1="14" x2="7" y2="14"></line><line x1="9" y1="8" x2="15" y2="8"></line><line x1="17" y1="16" x2="23" y2="16"></line></svg>`;

  const showCounts = typeof window === "undefined" || window.innerWidth >= 360;
  return `<div class="project-tools-mobile shell-controls" role="toolbar" aria-label="Project tools">
    <div class="project-tools-seg" role="tablist" aria-label="Project sections">
      ${tab("feed", "Feed", null)}
      ${tab("wiki", "Wiki", showCounts ? stats?.kb_pages ?? null : null)}
      ${tab("artifacts", "Artifacts", showCounts ? stats?.artifacts ?? null : null)}
      ${tab("sessions", "Sessions", showCounts ? stats?.sessions ?? null : null)}
    </div>
    <button type="button" class="project-filter-btn" aria-label="Filter and group" data-action="project-filter-toggle">
      ${filterGlyph}
    </button>
    ${projectOverflow(project, { lock: false, newPage: segment === "wiki" })}
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
    const filterToggle = event.target.closest?.('[data-action="project-filter-toggle"]');
    if (filterToggle) {
      const controls = document.querySelector(".shell-index .shell-controls:not(.project-tools-mobile)");
      if (controls) {
        controls.classList.toggle("mobile-filter-open");
      }
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

export function wireProjectHeader(project, stats, footprint) {
  // The overflow is drawn in both the desktop section header and the phone
  // tools row, so every instance is wired; each owns its own menu.
  for (const wrap of main.querySelectorAll(".proj-overflow-wrap")) {
    const btn = wrap.querySelector(".proj-overflow-btn");
    const menu = wrap.querySelector(".proj-overflow-menu");
    if (!btn || !menu) continue;

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

    // A menu item that carries its own document-level action (New page) still
    // closes the menu it was chosen from; the delegated handler on `main` runs
    // as usual because nothing here stops the event.
    menu.addEventListener("click", (e) => {
      if (e.target.closest(".proj-menu-item")) close();
    });

    const setConfidential = (next, title, body, commitLabel) => async (e) => {
      e.stopPropagation();
      close();
      const confirmed = await confirmAction({
        title,
        body,
        safe: "Cancel",
        tone: "primary",
        danger: commitLabel,
      });
      if (!confirmed) return;
      try {
        await api(`/api/v1/projects/${encodeURIComponent(project.id)}`, {
          method: "PATCH",
          body: JSON.stringify({ confidential: next }),
        });
        toast(next ? "Project is confidential." : "Project is public.");
        render();
      } catch (err) {
        toast(err.message);
      }
    };

    const name = project.display_name || project.id;
    const lockBtn = menu.querySelector('button[data-action="project-make-confidential"]');
    if (lockBtn) {
      lockBtn.addEventListener(
        "click",
        setConfidential(
          true,
          `Make ${name} confidential?`,
          "Only agents you grant it can reach it. Any agent without a grant loses access now.",
          "Make confidential",
        ),
      );
    }
    const publicBtn = menu.querySelector('button[data-action="project-make-public"]');
    if (publicBtn) {
      publicBtn.addEventListener(
        "click",
        setConfidential(
          false,
          `Make ${name} public?`,
          "Every agent on this hub can reach it, and its grants are no longer needed.",
          "Make public",
        ),
      );
    }

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
}

async function feedShell(id, segment, stats, params, mobileBar = "", project = null) {
  const selected = params?.get?.("event") || "";
  setFeedSelection(selected);
  const indexBody = await feedSection(id, stats, { chips: false });
  const held = feedVisit(id);
  const group = kindMenu(activeKinds(id), feedCounts(id));
  const event = feedEvent(id, selected) || held?.events?.[0] || null;
  const title = event ? formatEventSummary(event) : "Feed";
  const meta = event ? `${event.actor} · ${relative(event.created_at)}` : "";
  return shellHTML({
    segment,
    indexHead: `${PROJECT_MOBILE_STYLE}${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
    indexControls: shellIndexControls("Filter events", group),
    indexBody,
    stageHead: shellStageHead(title, meta, "", `#/projects/${encodeURIComponent(id)}/feed`),
    stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(
      event ? `${id} / feed / ${event.kind}` : id,
    )}</span></div>`,
    stageBody: eventStage(event, id),
    hasSelection: Boolean(selected),
  });
}

async function artifactsShell(id, segment, stats, params, mobileBar = "", project = null) {
  const selected = params?.get?.("artifact") || "";
  const { rows, artifacts } = await artifactIndex(id, selected);
  const chosen = selected || artifacts[0]?.id || "";
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

  if (chosen) {
    info = await artifactStage(chosen, id, selected ? params?.get?.("version") : null);
    if (info) {
      stageHead = shellStageHead(info.title, info.meta, info.actions, backHref);
      stageControls = `<div class="shell-controls artifact-stage-controls">${info.controls}</div>`;
      stageBody = info.body;
    } else if (selected) {
      // The address names an artifact that has gone since the link was made,
      // so the stage says so rather than showing nothing.
      stageHead = shellStageHead("Artifact", "", "", backHref);
      stageBody = `<div class="shell-pad"><p class="empty">This artifact is no longer on the hub.</p></div>`;
    }
  }

  return {
    html: shellHTML({
      segment,
      indexHead: `${PROJECT_MOBILE_STYLE}${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
      indexControls: shellIndexControls("Filter artifacts", artifactGroupMenu()),
      indexBody: rows,
      stageHead,
      stageControls,
      stageBody,
      aside: info?.aside || "",
      hasSelection: Boolean(selected),
    }),
    info,
    selected,
    chosen,
  };
}

async function sessionsShell(id, segment, stats, params, mobileBar = "", gen, project = null) {
  const selectedId = params?.get?.("id") || params?.get?.("session") || "";
  const filePath = params?.get?.("file") || params?.get?.("entry") || "";
  const { sessions, card } = await sessionRows(id, selectedId);
  const activeSessionId = selectedId || (sessions.length > 0 ? sessions[0].id : null);
  const selectedSession = sessions.find((s) => s.id === activeSessionId) || sessions[0] || null;
  let detailHTML = "";
  // The generation goes with it: the detail's own `stale` guard is what keeps
  // a slow paint off the screen, and without it every detail returned empty.
  if (selectedSession) detailHTML = await sessionDetailView(id, selectedSession.id, gen);
  const meta = selectedSession
    ? `${selectedSession.agent} · ${selectedSession.status} · ${formatBytes(selectedSession.brain_bytes || 0)}`
    : "";

  let stageHead = shellStageHead(
    selectedSession?.session_name || "Sessions",
    meta,
    "",
    `#/projects/${encodeURIComponent(id)}/sessions`,
  );
  let stageControls = `<div class="shell-controls"><span class="shell-meta mono">${esc(activeSessionId || id)}</span></div>`;
  let stageBody = detailHTML || `<div class="shell-pad"><p class="empty">Select a session.</p></div>`;
  let aside = "";
  let hasSelection = Boolean(selectedId || filePath);
  const coarse =
    typeof window === "undefined" || window.matchMedia("(pointer: coarse)").matches;

  if (selectedSession && filePath) {
    const entryRes = await fetchBrainEntry(selectedSession.id, filePath);
    if (entryRes.ok) {
      const isFs = entryRes.entry.kind === "file" || filePath.startsWith("/fs") || filePath.startsWith("fs/");
      if (isFs) {
        const rendered = await renderMarkdown(entryRes.entry.content);
        const fileName = filePath.split("/").pop() || filePath;
        const displayPath = entryRes.entry.path.startsWith("brain/")
          ? entryRes.entry.path
          : entryRes.entry.path.startsWith("/")
            ? `brain${entryRes.entry.path}`
            : `brain/${entryRes.entry.path}`;
        const provenance = `session brain · ${displayPath} · ${formatBytes(entryRes.entry.size_bytes)} · read-only`;
        const backHref = `#/projects/${encodeURIComponent(id)}/sessions?id=${encodeURIComponent(selectedSession.id)}`;
        stageHead = shellStageHead(fileName, "", "", backHref);
        stageControls = `<div class="shell-controls" style="gap:8px;padding:0 16px">
          <span class="shell-meta mono" style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(provenance)}</span>
          <button type="button" class="btn-outline" data-action="wiki-promote" data-id="${esc(id)}" data-session="${esc(selectedSession.id)}" data-session-name="${esc(selectedSession.session_name || selectedSession.id)}" data-path="${esc(entryRes.entry.path)}" style="flex:none;height:30px;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 13px/1 var(--font-sans);cursor:pointer">Save to wiki</button>
        </div>`;
        stageBody = `<div class="hub-viewer-doc"><div class="session-doc-content" style="max-width: 640px; margin: 0; padding: 24px 16px;">${rendered}</div></div>`;
        aside = "";
        hasSelection = true;
      } else {
        // One surface per thing: a fine pointer reads the value in the aside,
        // a coarse pointer gets one bottom sheet and the aside is left empty.
        if (coarse) {
          stageBody += kvSheetHTML(filePath, entryRes.entry.content);
          aside = "";
        } else {
          aside = kvAsideHTML(filePath, entryRes.entry.content);
        }
      }
    } else {
      // The aside is hidden on a coarse pointer, so an error about the entry
      // is said in the stage there rather than in a panel nobody can see.
      if (coarse) {
        stageBody += `<div class="shell-pad"><p class="empty">${esc(entryRes.error)}</p></div>`;
        aside = "";
      } else {
        aside = errorAsideHTML(filePath, entryRes.error);
      }
    }
  }

  return {
    html: shellHTML({
      segment,
      indexHead: `${PROJECT_MOBILE_STYLE}${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
      indexControls: shellIndexControls("Filter sessions"),
      indexBody: card,
      stageHead,
      stageControls,
      stageBody,
      aside,
      hasSelection,
    }),
    selectedSession,
  };
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

  const agentsText =
    stats && stats.agents_active != null ? `${count(stats.agents_active, "agent", "agents")} active` : "";
  const mobileBar = shellMobileBar(
    project.display_name || id,
    [agentsText, footprint].filter(Boolean).join(" · "),
    "#/projects",
  );

  let shell;
  let artifactInfo = null;
  let artifactId = "";
  let sessionSelected = null;
  if (segment === "wiki") {
    const selected = params?.get("page") || "";
    const stage = await wikiStage(id, params, shellStageHead, stats, project?.display_name || id);
    const indexBody = await wikiIndexBody(id, selected, project?.display_name || id, params?.get("dir") || "");
    const newBtn = `<button type="button" class="btn-outline" data-action="wiki-new" data-id="${esc(id)}" data-project="${esc(project?.display_name || id)}" style="flex:none;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 13px/1 var(--font-sans);cursor:pointer">New page</button>`;
    shell = shellHTML({
      segment,
      indexHead: `${PROJECT_MOBILE_STYLE}${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
      indexControls: shellIndexControls("Search this wiki", "", newBtn),
      indexBody,
      stageHead: stage.head,
      stageControls: stage.controls,
      stageBody: stage.body,
      hasSelection: Boolean(selected || params?.get("new") === "1" || params?.get("edit") === "1"),
    });
  } else if (segment === "artifacts") {
    const built = await artifactsShell(id, segment, stats, params, mobileBar, project);
    shell = built.html;
    artifactInfo = built.info;
    artifactId = built.chosen || built.selected;
  } else if (segment === "sessions") {
    const built = await sessionsShell(id, segment, stats, params, mobileBar, gen, project);
    shell = built.html;
    sessionSelected = built.selectedSession;
  } else {
    shell = await feedShell(id, segment, stats, params, mobileBar, project);
  }
  if (stale(gen)) return;

  paint(gen, shell);
  installShellLayout(main);
  if (artifactInfo) wireArtifactStage(main, artifactId, artifactInfo);
  if (sessionSelected) wireSessionDetail(main, id, sessionSelected.id);
  wireProjectHeader(project, stats, footprint);
}
// The row selection the keyboard map owns: the project's segments all paint
// `.row`s, so a reader walking the feed, the gallery or the list gets the map.
registerScreen("projects", { rows: ".row" });
