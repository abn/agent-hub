// The project view in the one shell: an index that lists the current
// section, a stage that shows the selected item, and the section switcher in
// the index header. Every segment is its own address, so reload and the
// browser's Back both land where the reader was. The session rows and the
// gallery are rendered by the modules that own them.

import { api } from "./api.mjs";
import { foldSessions, liveWords, participantsText } from "./author.mjs";
import { artifactGroupMenu, artifactIndex, artifactStage, endArtifactStage, wireArtifactStage } from "./artifacts.mjs";
import { confirmAction, confirmProjectDelete, openCreateProjectDialog } from "./dialog.mjs";
import { count, esc, main, paint, stale } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import {
  activeKinds,
  eventStage,
  feedCounts,
  feedEvent,
  feedScope,
  feedSection,
  feedSelectBar,
  feedVisit,
  kindMenu,
  resumePicking,
  setFeedSelection,
  stageRoot,
} from "./feed.mjs";
import { registerScreen } from "./keys.mjs";
import { render } from "./router.mjs";
import { pickProject, projectsIndexScreen } from "./projects.mjs";
import { renderRailProjects } from "./shell.mjs";
import { wikiIndexBody, wikiStage, wireWikiComments } from "./wiki.mjs";
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
import { toast } from "./toast.mjs";
import { artifactsScope, loadWho, whoActive, whoMenu } from "./who.mjs";


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
  return `<span class="project-lock project-lock-badge" role="img" aria-label="confidential">${glyphSvg("lock", { size: 16 })}<span>confidential</span></span>`;
}

// The project's own actions, in the header overflow the design draws. The
// project header carries Project settings, Copy path and Delete project; the
// project's own settings screen passes lock-only and gets the confidential
// control instead. An operator may lock or unlock from there.
export function projectOverflow(project, { lock = true, full = true, newPage = false, feed = false } = {}) {
  const name = project.display_name || project.id;
  // On a phone the tools row has no room for a second overflow, so the feed's
  // two verbs ride in this one, above the project's own controls.
  const feedItems = feed
    ? `<button type="button" role="menuitem" class="proj-menu-item" data-action="feed-select" data-project="${esc(project.id)}">Select items</button>
      <button type="button" role="menuitem" class="proj-menu-item" data-action="feed-dismiss-read" data-project="${esc(project.id)}">Dismiss read</button>
      <div class="proj-menu-divider" role="separator"></div>`
    : "";
  const lockItem = !lock
    ? ""
    : project.confidential
      ? `<button type="button" role="menuitem" class="proj-menu-item" data-action="project-make-public">Make public…</button>`
      : `<button type="button" role="menuitem" class="proj-menu-item" data-action="project-make-confidential">Make confidential…</button>`;
  const newPageItem = newPage
    ? `<button type="button" role="menuitem" class="proj-menu-item" data-action="wiki-new" data-id="${esc(project.id)}" data-project="${esc(name)}">New page</button>`
    : "";
  const extras = full
    ? `<button type="button" role="menuitem" class="proj-menu-item" data-action="capture-idea" data-project="${esc(project.id)}">Capture idea</button>
      <a role="menuitem" class="proj-menu-item" href="#/projects/${encodeURIComponent(project.id)}/settings">Project settings</a>
      <button type="button" role="menuitem" class="proj-menu-item" data-action="copy-project-path">Copy path</button>
      <div class="proj-menu-divider" role="separator"></div>
      <button type="button" role="menuitem" class="proj-menu-item proj-menu-delete" data-action="delete-project">Delete project</button>`
    : "";
  void name;
  return `<span class="proj-overflow-wrap">
    <button type="button" class="proj-overflow-btn" aria-label="Project actions" aria-haspopup="menu" aria-expanded="false">${glyphSvg("overflow", { size: 20 })}</button>
    <div class="proj-overflow-menu" role="menu" hidden>
      ${feedItems}
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

function projectToolsMobile(id, segment, stats, project) {
  const count = (n) => (n == null ? "" : `<span class="shell-seg-count">${n}</span>`);
  const tab = (seg, label, n) =>
    `<a href="#/projects/${encodeURIComponent(id)}/${seg}" role="tab" aria-selected="${
      segment === seg
    }"${segment === seg ? ' aria-current="page"' : ""}>${label}${count(n)}</a>`;
  const filterGlyph = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="4" y1="21" x2="4" y2="14"></line><line x1="4" y1="10" x2="4" y2="3"></line><line x1="12" y1="21" x2="12" y2="12"></line><line x1="12" y1="8" x2="12" y2="3"></line><line x1="20" y1="21" x2="20" y2="16"></line><line x1="20" y1="12" x2="20" y2="3"></line><line x1="1" y1="14" x2="7" y2="14"></line><line x1="9" y1="8" x2="15" y2="8"></line><line x1="17" y1="16" x2="23" y2="16"></line></svg>`;

  // What narrows this segment now: each kind the feed is held to, and a Who
  // choice. The glyph says so, since the panel it opens is shut.
  const active =
    segment === "feed"
      ? activeKinds(id).size + (whoActive(feedScope(id)) ? 1 : 0)
      : segment === "artifacts"
        ? Number(whoActive(artifactsScope(id)))
        : 0;
  const showCounts = typeof window === "undefined" || window.innerWidth >= 360;
  return `<div class="project-tools-mobile shell-controls" role="toolbar" aria-label="Project tools">
    <div class="project-tools-seg" role="tablist" aria-label="Project sections">
      ${tab("feed", "Feed", null)}
      ${tab("wiki", "Wiki", showCounts ? stats?.kb_pages ?? null : null)}
      ${tab("artifacts", "Artifacts", showCounts ? stats?.artifacts ?? null : null)}
      ${tab("sessions", "Sessions", showCounts ? stats?.sessions ?? null : null)}
    </div>
    <button type="button" class="project-filter-btn${active ? " is-active" : ""}" aria-label="${active ? `Filter and group, ${active} active` : "Filter and group"}" aria-pressed="${active > 0}" data-action="project-filter-toggle">
      ${filterGlyph}${active ? `<span class="project-filter-dot" aria-hidden="true"></span>` : ""}
    </button>
    ${projectOverflow(project, { lock: false, newPage: segment === "wiki", feed: segment === "feed" })}
    ${segment === "feed" ? feedSelectBar() : ""}
  </div>`;
}

const IDEA_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 9 18h6 M 10 21h4 M 12 3a6.5 6.5 0 0 0-3.8 11.8c.8.6 1.3 1.6 1.3 2.7h5c0-1.1.5-2.1 1.3-2.7A6.5 6.5 0 0 0 12 3z"></path></svg>`;

const FEED_GLYPH = `<svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><path d="M16 16l4 4"></path></svg>`;

let lastOpenSession = "";

async function captureIdea(projectId, opener) {
  const confirmed = await confirmAction({
    title: "Capture an idea",
    body: "It lands in the project's inbox as an open item for triage.",
    safe: "Cancel",
    danger: "Capture",
    tone: "action",
    field: { label: "Idea", limit: 2000, optional: false },
    focusField: true,
    commit: async (note) => {
      const summary = (note || "").trim();
      if (!summary) throw new Error("Write an idea to capture.");
      await api(`/api/v1/projects/${encodeURIComponent(projectId)}/ideas`, {
        method: "POST",
        body: JSON.stringify({ summary }),
      });
    },
  });
  if (confirmed) {
    toast("Idea captured, open in the inbox.");
    // Re-render the feed so the new item is visible after triage.
    if (typeof window !== "undefined") window.location.hash = window.location.hash;
  }
}

function returnSessionFocus(id) {
  requestAnimationFrame(() => {
    const link = document.querySelector(`.session-row a[href*="id=${CSS.escape(id)}"]`);
    const row = link ? link.closest(".session-row") : null;
    const target = /** @type {HTMLElement} */ (row || link);
    if (target) {
      if (target.tabIndex < 0) target.tabIndex = 0;
      target.focus();
    }
  });
}

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const newBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.('[data-action="new-project"]'));
    if (newBtn) {
      event.preventDefault();
      openCreateProjectDialog();
      return;
    }
    const filterToggle = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.('[data-action="project-filter-toggle"]'));
    if (filterToggle) {
      const controls = document.querySelector(".shell-index .shell-controls:not(.project-tools-mobile)");
      if (controls) {
        controls.classList.toggle("mobile-filter-open");
      }
      return;
    }
    const captureBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.('[data-action="capture-idea"]'));
    if (captureBtn) {
      event.preventDefault();
      const projectId = captureBtn.dataset.project;
      captureIdea(projectId, captureBtn);
      return;
    }
    const link = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.(".session-link, .session-row a"));
    if (link) {
      const href = link.getAttribute("href") || "";
      const match = href.match(/[?&]id=([^&]+)/);
      if (match) {
        lastOpenSession = decodeURIComponent(match[1]);
      }
    }
    if (!/** @type {HTMLElement} */ (event.target).closest(".proj-overflow-wrap")) {
      const menus = document.querySelectorAll(".proj-overflow-menu:not([hidden])");
      for (const m of menus) {
        /** @type {HTMLElement} */ (m).hidden = true;
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
    const btn = /** @type {HTMLElement} */ (wrap.querySelector(".proj-overflow-btn"));
    const menu = /** @type {HTMLElement} */ (wrap.querySelector(".proj-overflow-menu"));
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
        const first = /** @type {HTMLElement} */ (menu.querySelector("button, a"));
        if (first) first.focus();
      }
    });

    menu.addEventListener("keydown", (e) => {
      if (/** @type {KeyboardEvent} */ (e).key === "Escape") {
        e.stopPropagation();
        close();
        btn.focus();
      }
    });

    // A menu item that carries its own document-level action (New page) still
    // closes the menu it was chosen from; the delegated handler on `main` runs
    // as usual because nothing here stops the event.
    menu.addEventListener("click", (e) => {
      if (/** @type {HTMLElement} */ (e.target).closest(".proj-menu-item")) close();
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
          "Only members you grant it can reach it. Any member without a grant loses access now.",
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
          "Every member on this hub can reach it, and its grants are no longer needed.",
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
            renderRailProjects();
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
  const group = kindMenu(activeKinds(id), feedCounts(id)) + whoMenu(feedScope(id), await loadWho(id));
  // An address that names an event the page does not hold says so rather than
  // opening the newest row under the old id. A dismissed event is one way an
  // id can be missing; an event older than the loaded page is another.
  const event = selected ? feedEvent(id, selected) : held?.events?.[0] || null;
  const missing = Boolean(selected) && !event;
  const filtered = activeKinds(id).size > 0 || whoActive(feedScope(id));
  // The stage's header says where the reader is, the project and its feed.
  // The event's own kind, title, author and time are the body's first lines,
  // so the header does not repeat them.
  const title = project?.display_name || id;
  const meta = [
    "feed",
    stats && stats.sessions_live != null ? liveWords(stats.sessions_live, stats.subagents_live) : "",
  ]
    .filter(Boolean)
    .join(" · ");
  // The overflow carries the feed's own verbs; the bar replaces the filter row
  // while the reader is picking, so both share the one control row.
  // Capture idea is a glyph with its name on hover, so the filter beside it
  // keeps room to read what is typed into it.
  const right = `<button type="button" class="feed-capture" data-action="capture-idea" data-project="${esc(id)}" aria-label="Capture idea" title="Capture idea">${IDEA_GLYPH}</button>${feedSelectBar()}`;
  return shellHTML({
    segment,
    indexHead: `${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false, feed: true })}</div>`,
    indexControls: shellIndexControls("Filter events", group, right),
    indexBody,
    stageHead: shellStageHead(title, meta, "", `#/projects/${encodeURIComponent(id)}/feed`),
    stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(
      event ? `${id} / feed / ${stageRoot(event, id).kind}` : id,
    )}</span></div>`,
    stageBody: missing
      ? `<div class="shell-pad"><p class="empty">${
          filtered
            ? "This event is not in the filtered feed. Clear a filter to see it."
            : "This event is not in the feed. It may have been dismissed, or it is older than the rows this page has loaded."
        }</p></div>`
      : await eventStage(event, id),
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
  let stageBody = `<div class="shell-pad"><p class="empty">No artifacts yet.</p></div>`;
  let info = null;

  if (chosen) {
    info = await artifactStage(
      chosen,
      id,
      selected ? params?.get?.("version") : null,
      artifacts.find((a) => a.id === chosen) || null,
    );
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
      indexHead: `${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
      indexControls: shellIndexControls("Filter artifacts", artifactGroupMenu() + whoMenu(artifactsScope(id), await loadWho(id))),
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
  const resumeMode = params?.get?.("resume") === "1";
  const { sessions, card } = await sessionRows(id, selectedId);
  // With none chosen, the stage opens the list's first row as the list draws it:
  // the active sessions first, a subagent never ahead of the parent it sits under.
  const drawn = [
    ...foldSessions(sessions.filter((s) => s.status === "active")),
    ...foldSessions(sessions.filter((s) => s.status !== "active")),
  ].map((row) => row.session);
  const activeSessionId = selectedId || (drawn.length > 0 ? drawn[0].id : null);
  const selectedSession = sessions.find((s) => s.id === activeSessionId) || sessions[0] || null;
  let detailHTML = "";
  // The generation goes with it: the detail's own `stale` guard is what keeps
  // a slow paint off the screen, and without it every detail returned empty.
  if (selectedSession) detailHTML = await sessionDetailView(id, selectedSession.id, gen, resumeMode);
  const meta = selectedSession
    ? [participantsText(selectedSession.participants, { includeLeft: true }), formatBytes(selectedSession.brain_bytes || 0)]
        .filter(Boolean)
        .join(" · ")
    : "";

  let stageHead = shellStageHead(
    selectedSession?.session_name || "Sessions",
    meta,
    "",
    `#/projects/${encodeURIComponent(id)}/sessions`,
  );
  // The session stage: the brain tree, or the events after the anchor. The
  // pair is two links rather than a script toggle, so each view is its own
  // address and the browser's own Back returns to the other.
  const brainHref = `#/projects/${encodeURIComponent(id)}/sessions?id=${encodeURIComponent(
    activeSessionId || "",
  )}`;
  const viewToggle =
    selectedSession
      ? `<div class="session-view-toggle" role="group" aria-label="Session view">
          <a class="session-view-link" href="${brainHref}&resume=1"${resumeMode ? ' aria-current="page"' : ""}>Resume</a>
          <a class="session-view-link" href="${brainHref}"${resumeMode ? "" : ' aria-current="page"'}>Brain</a>
        </div>`
      : "";
  let stageControls = `<div class="shell-controls session-view-controls">
    ${viewToggle}
  </div>`;
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
        stageControls = `<div class="shell-controls brain-file-controls">
          <span class="shell-meta mono brain-file-meta">${esc(provenance)}</span>
          <button type="button" class="btn-outline brain-promote-btn" data-action="wiki-promote" data-id="${esc(id)}" data-session="${esc(selectedSession.id)}" data-session-name="${esc(selectedSession.session_name || selectedSession.id)}" data-path="${esc(entryRes.entry.path)}">Save to wiki</button>
        </div>`;
        stageBody = `<div class="hub-viewer-doc"><div class="session-doc-content">${rendered}</div></div>`;
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
      indexHead: `${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
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

  const liveText =
    stats && stats.sessions_live != null ? liveWords(stats.sessions_live, stats.subagents_live) : "";
  const mobileBar = shellMobileBar(
    project.display_name || id,
    [liveText, footprint].filter(Boolean).join(" · "),
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
    const newBtn = `<button type="button" class="btn-outline project-wiki-new" data-action="wiki-new" data-id="${esc(id)}" data-project="${esc(project?.display_name || id)}">New page</button>`;
    shell = shellHTML({
      segment,
      indexHead: `${mobileBar}${projectToolsMobile(id, segment, stats, project)}<div class="shell-head project-seg-head">${segSwitcher(id, segment, stats)}${projectOverflow(project, { lock: false })}</div>`,
      indexControls: shellIndexControls("Search this wiki", "", newBtn),
      indexBody,
      stageHead: stage.head,
      stageControls: stage.controls,
      stageBody: stage.body,
      hasSelection: Boolean(selected || params?.get("view") || params?.get("new") === "1" || params?.get("edit") === "1"),
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

  endArtifactStage();
  paint(gen, shell);
  installShellLayout(main);
  if (segment === "feed") resumePicking();
  if (artifactInfo) wireArtifactStage(main, artifactId, artifactInfo);
  if (sessionSelected) wireSessionDetail(main, id, sessionSelected.id);
  if (segment === "wiki") wireWikiComments(main);
  wireProjectHeader(project, stats, footprint);
}
// The row selection the keyboard map owns: the project's segments all paint
// `.row`s, so a reader walking the feed, the gallery or the list gets the map.
registerScreen("projects", { rows: ".row" });
