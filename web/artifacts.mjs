// Artifacts: the per-project gallery, and the viewer that embeds an artifact's
// own public page next to the comments drawer. The viewer is a route of its
// own, so reload and the browser's Back both keep the artifact on screen.

import { api } from "./api.mjs";
import { commentsPanel, commentsToggle, openCommentsDrawer, startComments } from "./comments.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

const DOC_PATH = "M 6 3h9l4 4v14H6z M 8 12h8 M 8 16h8";
const LOCK_PATH = "M 7 11V8a5 5 0 0 1 10 0v3 M 5 11h14v10H5z";

function cardGlyph(protectedArtifact) {
  const path = protectedArtifact ? LOCK_PATH : DOC_PATH;
  const cls = protectedArtifact ? "lock" : "doc";
  return `<svg class="${cls}" aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="${path}"></path></svg>`;
}

function formatBytes(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function cardMeta(artifact) {
  const age = artifact.created_at || artifact.updated_at ? relative(Date.parse(artifact.created_at || artifact.updated_at)) : "";
  const actor = artifact.actor || "agent";
  const enc = artifact.protected ? " · encrypted" : "";
  return `${actor} · v${artifact.version} · ${formatBytes(artifact.size_bytes)} · ${age}${enc}`;
}

export function artifactCard(artifact) {
  const enc = artifact.protected ? " encrypted" : " plain";
  const age = artifact.created_at || artifact.updated_at ? relative(Date.parse(artifact.created_at || artifact.updated_at)) : "";
  const actor = artifact.actor || "agent";
  const encLabel = artifact.protected ? " · encrypted" : "";
  return `<button type="button" class="artifact-card artifact-row" data-action="artifact-open" data-id="${esc(artifact.id)}">
    <span class="artifact-preview${enc}">${cardGlyph(artifact.protected)}</span>
    <span class="artifact-body">
      <span class="artifact-title">${esc(artifact.title)}</span>
      <span class="artifact-meta mono">${esc(actor)} · <span class="mono">v${artifact.version} · ${formatBytes(artifact.size_bytes)}</span> · ${age}${encLabel}</span>
    </span>
    <svg class="artifact-chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 5l7 7-7 7"></path></svg>
  </button>`;
}

let activeGrouping = "day";

function groupDayKey(dateStr) {
  if (!dateStr) return "TODAY";
  const d = new Date(dateStr);
  const now = new Date();
  const isToday = d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
  if (isToday) return "TODAY";
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const isYesterday = d.getFullYear() === yesterday.getFullYear() && d.getMonth() === yesterday.getMonth() && d.getDate() === yesterday.getDate();
  if (isYesterday) return "YESTERDAY";
  const months = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
  return `${months[d.getMonth()]} ${d.getDate()}`;
}

function groupArtifacts(artifacts, mode) {
  const groups = new Map();
  for (const artifact of artifacts) {
    let key;
    if (mode === "agent") {
      key = (artifact.actor || "AGENT").toUpperCase();
    } else if (mode === "kind") {
      key = (artifact.kind || "DOCUMENT").toUpperCase();
    } else {
      key = groupDayKey(artifact.created_at || artifact.updated_at);
    }
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(artifact);
  }
  return [...groups.entries()].map(([title, items]) => ({ title, items }));
}

// The gallery the project view's Artifacts segment paints. Grouped list by day, agent, or kind.
export async function gallerySection(projectId) {
  const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts`);
  if (!artifacts || !artifacts.length) return emptyStateHTML(EMPTY_COPY.artifacts);

  const totalBytes = artifacts.reduce((acc, a) => acc + (Number(a.size_bytes) || 0), 0);
  const groups = groupArtifacts(artifacts, activeGrouping);

  const modeLabel = activeGrouping === "agent" ? "Agent" : activeGrouping === "kind" ? "Kind" : "Day";

  const groupSections = groups
    .map(
      (g) =>
        `<div class="hub-group-header">${esc(g.title)} · ${g.items.length}</div>
         <div class="hub-group-items">${g.items.map(artifactCard).join("")}</div>`
    )
    .join("");

  return `
    <div class="hub-artifacts-summary">
      <span class="mono">${artifacts.length} artifacts · ${formatBytes(totalBytes)}</span>
      <div class="hub-group-wrap">
        <button type="button" class="hub-group-toggle" id="hub-group-toggle" data-action="artifact-group-toggle" aria-haspopup="true" aria-expanded="false">${modeLabel}<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 9l6 6 6-6"></path></svg></button>
        <div class="hub-group-menu" id="hub-group-menu" hidden>
          <button type="button" data-group="day"${activeGrouping === "day" ? ' class="active" aria-current="true"' : ""}>Day</button>
          <button type="button" data-group="agent"${activeGrouping === "agent" ? ' class="active" aria-current="true"' : ""}>Agent</button>
          <button type="button" data-group="kind"${activeGrouping === "kind" ? ' class="active" aria-current="true"' : ""}>Kind</button>
        </div>
      </div>
    </div>
    <div class="gallery hub-artifacts-list">
      ${groupSections}
    </div>
  `;
}

// Global click listener for group menu in gallery and start-thread in viewer
document.addEventListener("click", (e) => {
  const toggleBtn = e.target.closest('[data-action="artifact-group-toggle"]');
  if (toggleBtn) {
    const menu = document.getElementById("hub-group-menu");
    if (menu) {
      const open = menu.hidden;
      menu.hidden = !open;
      toggleBtn.setAttribute("aria-expanded", String(open));
    }
    return;
  }
  const groupOpt = e.target.closest("#hub-group-menu button");
  if (groupOpt && groupOpt.dataset.group) {
    activeGrouping = groupOpt.dataset.group;
    const parts = location.hash.replace(/^#/, "").split("/");
    const projectId = parts[2] || "";
    if (projectId) {
      gallerySection(projectId).then((html) => {
        const summary = main.querySelector(".hub-artifacts-summary");
        const list = main.querySelector(".hub-artifacts-list");
        if (summary && list) {
          const temp = document.createElement("div");
          temp.innerHTML = html;
          const newSummary = temp.querySelector(".hub-artifacts-summary");
          const newList = temp.querySelector(".hub-artifacts-list");
          if (newSummary) summary.replaceWith(newSummary);
          if (newList) list.replaceWith(newList);
        }
      });
    }
    return;
  }
  const startThread = e.target.closest('[data-action="start-thread"]');
  if (startThread) {
    const menu = main.querySelector(".hub-overflow-menu");
    if (menu) menu.hidden = true;
    const more = main.querySelector(".hub-more");
    if (more) more.setAttribute("aria-expanded", "false");
    openCommentsDrawer();
    return;
  }
  const menu = document.getElementById("hub-group-menu");
  if (menu && !menu.hidden && !e.target.closest(".hub-group-wrap")) {
    menu.hidden = true;
    const toggle = document.getElementById("hub-group-toggle");
    if (toggle) toggle.setAttribute("aria-expanded", "false");
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  const menu = document.getElementById("hub-group-menu");
  if (menu && !menu.hidden) {
    e.stopPropagation();
    menu.hidden = true;
    const toggle = document.getElementById("hub-group-toggle");
    if (toggle) {
      toggle.setAttribute("aria-expanded", "false");
      toggle.focus();
    }
  }
});

// The legacy gallery address still works: it now points at the project's
// Artifacts segment.
export async function artifactsScreen(selected, gen) {
  if (selected) location.hash = `#/projects/${encodeURIComponent(selected)}/artifacts`;
}

export function openArtifact(id) {
  const parts = location.hash.replace(/^#/, "").split("/");
  const project = parts[1] === "projects" ? parts[2] : "";
  const base = `#/artifacts/${encodeURIComponent(id)}`;
  location.hash = project ? `${base}?project=${encodeURIComponent(project)}` : base;
}

export function viewerBack() {
  if (window.history.length > 1) {
    window.history.back();
    return;
  }
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  const project = params.get("project");
  location.hash = project ? `#/projects/${encodeURIComponent(project)}/artifacts` : "#/projects";
}

const viewer = { id: null, version: null, kind: null, raw: false };

// The framed page cannot remember a theme (a sandboxed frame has no store),
// so the viewer names the one it wants in the address. Relative to this
// document (the app shell), not the origin root, so it still lands on the
// artifact page under whatever prefix a proxy mounts the app on.
function frameSrc(id, version, theme) {
  const params = new URLSearchParams();
  if (version) params.set("version", String(version));
  if (theme) params.set("theme", theme);
  const query = params.toString();
  const address = `artifacts/${encodeURIComponent(id)}`;
  return query ? `${address}?${query}` : address;
}

function viewerSource(frame) {
  if (!frame) return;
  return frameSrc(frame.dataset.id, viewer.version, frame.getAttribute("data-theme"));
}

// Navigating the sandboxed frame in place pushes a history entry, which would
// make the browser's Back undo the theme rather than leave the artifact.
// Replacing the element avoids pushing history while preserving its setup.
function replaceFrame(frame, src, srcdoc) {
  const replacement = document.createElement("iframe");
  if (frame.id) replacement.id = frame.id;
  if (frame.hasAttribute("sandbox")) replacement.setAttribute("sandbox", frame.getAttribute("sandbox"));
  if (frame.hasAttribute("title")) replacement.setAttribute("title", frame.getAttribute("title"));
  for (const attr of frame.attributes) {
    if (attr.name.startsWith("data-")) {
      replacement.setAttribute(attr.name, attr.value);
    }
  }
  if (srcdoc !== undefined) {
    replacement.srcdoc = srcdoc;
  } else if (src) {
    replacement.src = src;
  }
  frame.replaceWith(replacement);
  return replacement;
}

// A second local escape for the raw srcdoc: the host page's own `esc` spends
// its budget on attributes, and the raw text lands in a text node.
function escText(value) {
  return String(value ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

async function fetchRawText(id, version) {
  const query = version ? `?version=${version}` : "";
  const headers = {};
  const token = localStorage.getItem("hub.token");
  if (token) headers.Authorization = "Bearer " + token;
  const res = await fetch(`/api/v1/artifacts/${encodeURIComponent(id)}/raw${query}`, { headers });
  if (!res.ok) throw new Error("Failed to fetch raw text");
  const ct = res.headers.get("content-type") || "";
  if (ct.includes("application/json")) {
    const json = await res.json();
    return typeof json === "string" ? json : JSON.stringify(json, null, 2);
  }
  return await res.text();
}

export async function toggleRaw(button) {
  const frame = main.querySelector("#hub-frame");
  const id = frame && frame.dataset.id;
  if (!id) return;
  if (viewer.raw) {
    replaceFrame(frame, viewerSource(frame));
    viewer.raw = false;
    if (button) button.textContent = "Open raw";
    return;
  }
  try {
    const text = await fetchRawText(id, viewer.version);
    const doc =
      `<style>body{margin:0;padding:24px;font-family:ui-monospace,"SF Mono",Menlo,Consolas,monospace;` +
      `font-size:13px;line-height:1.6;white-space:pre-wrap;overflow-wrap:anywhere}</style>` +
      `<pre>${escText(text)}</pre>`;
    replaceFrame(frame, null, doc);
    viewer.raw = true;
    if (button) button.textContent = "Back to view";
  } catch (error) {
    if (button) button.textContent = "Open raw";
  }
}

export function toggleVersionMenu(button) {
  const menu = document.getElementById("hub-version-menu");
  const backdrop = document.getElementById("hub-version-backdrop");
  if (!menu) return;
  const open = menu.hidden;
  menu.hidden = !open;
  if (backdrop) backdrop.hidden = !open;
  if (button) button.setAttribute("aria-expanded", String(open));
  if (open) {
    const active = menu.querySelector('[aria-current="true"]');
    (active || menu.querySelector("button"))?.focus();
  }
}

export function pickVersion(id, version) {
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  const project = params.get("project");
  const base = `#/artifacts/${encodeURIComponent(id)}?version=${encodeURIComponent(version)}`;
  const target = project ? `${base}&project=${encodeURIComponent(project)}` : base;
  location.replace(target);
}

export function toggleViewerTheme() {
  const frame = main.querySelector("#hub-frame");
  if (!frame) return;
  const next = frame.getAttribute("data-theme") === "dark" ? "light" : "dark";
  frame.setAttribute("data-theme", next);
  let src;
  if (frame.dataset.kind === "html") {
    const params = new URLSearchParams();
    if (viewer.version) params.set("version", String(viewer.version));
    params.set("theme", next);
    src = `artifacts/${encodeURIComponent(frame.dataset.id)}/frame?${params.toString()}`;
  } else {
    src = viewerSource(frame);
  }
  replaceFrame(frame, src);
}

function svg(path, width, height, cls) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("width", String(width));
  el.setAttribute("height", String(height));
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("fill", "none");
  el.setAttribute("stroke", "currentColor");
  el.setAttribute("stroke-width", "1.8");
  el.setAttribute("stroke-linecap", "round");
  el.setAttribute("stroke-linejoin", "round");
  if (cls) el.setAttribute("class", cls);
  el.setAttribute("aria-hidden", "true");
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p.setAttribute("d", path);
  el.appendChild(p);
  return el;
}

// Version sheet (Screen 02): replaces inline version select
function buildVersionSheet(id, versions, shown, projectId) {
  const backdrop = document.createElement("div");
  backdrop.id = "hub-version-backdrop";
  backdrop.className = "hub-version-backdrop";
  backdrop.hidden = true;

  const sheet = document.createElement("div");
  sheet.id = "hub-version-menu";
  sheet.className = "hub-version-sheet hub-version-menu";
  sheet.hidden = true;
  sheet.setAttribute("role", "dialog");
  sheet.setAttribute("aria-modal", "true");
  sheet.setAttribute("aria-label", "Versions");

  const handle = document.createElement("div");
  handle.className = "hub-version-sheet-handle";

  const head = document.createElement("div");
  head.className = "hub-version-sheet-header";
  head.innerHTML = `
    <span class="hub-version-sheet-title">Versions</span>
    <span class="hub-version-sheet-count mono">${versions.length} · newest first</span>
  `;

  const list = document.createElement("div");
  list.className = "hub-version-sheet-list";

  for (const v of [...versions].reverse()) {
    const isCurrent = v.version === shown;
    const row = document.createElement("button");
    row.type = "button";
    row.className = `hub-version-row${isCurrent ? " current" : ""}`;
    row.dataset.action = "version-pick";
    row.dataset.id = id;
    row.dataset.version = String(v.version);
    if (isCurrent) row.setAttribute("aria-current", "true");

    const age = v.created_at ? relative(Date.parse(v.created_at)) : "";
    const actor = v.actor || "agent";
    const primaryText = isCurrent ? "Current" : actor;
    const secondaryText = isCurrent ? `${actor} · ${age}` : age;

    row.innerHTML = `
      <span class="hub-version-num mono">v${v.version}</span>
      <span class="hub-version-info">
        <span class="hub-version-primary">${esc(primaryText)}</span>
        <span class="hub-version-secondary">${esc(secondaryText)}</span>
      </span>
      <span class="hub-version-size mono">${formatBytes(v.size_bytes)}</span>
    `;
    list.appendChild(row);
  }

  const footer = document.createElement("div");
  footer.className = "hub-version-sheet-footer";
  footer.textContent = "Versions are whole saves. Nothing is compared - the hub keeps no diff.";

  sheet.append(handle, head, list, footer);

  const closeSheet = () => {
    backdrop.hidden = true;
    sheet.hidden = true;
    const toggle = main.querySelector(".hub-version-toggle");
    if (toggle) {
      toggle.setAttribute("aria-expanded", "false");
      toggle.focus();
    }
  };

  backdrop.addEventListener("click", closeSheet);
  sheet.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeSheet();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !sheet.hidden) {
      e.stopPropagation();
      closeSheet();
    }
  });

  return { backdrop, sheet };
}

export async function viewerRoute(params, gen, path) {
  const parts = (path || "").split("/");
  const id = parts[2];
  if (!id) {
    location.hash = "#/projects";
    return;
  }
  const version = params.get("version");
  const projectId = params.get("project") || "";
  viewer.id = id;
  viewer.version = version ? Number(version) : null;
  viewer.raw = false;
  startComments(id);

  let versions = [];
  try {
    const listed = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/versions`);
    versions = listed.versions || [];
  } catch (error) {
    paint(gen, `<h1>Artifact</h1><p class="error">${esc(error.message)}</p>`);
    return;
  }
  if (stale(gen)) return;
  const newest = (versions.length && versions[versions.length - 1].version) || 1;
  const shown = viewer.version || newest;
  const current = versions.find((v) => v.version === shown) || versions[versions.length - 1] || versions[0];
  if (!current) {
    paint(gen, `<h1>Artifact</h1><p class="error">No versions of this artifact.</p>`);
    return;
  }
  viewer.version = shown;
  viewer.kind = current.kind;

  // Mono path: {project} / {slug}
  const projDisplay = projectId || current.project_id || "agent-hub";
  const slug = (current.title || "artifact")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "") + (current.kind === "markdown" ? ".md" : ".html");
  const monoPath = `${projDisplay} / ${slug}`;

  // Fetch comments count for comments strip
  let commentsCount = 0;
  try {
    const comList = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/comments`);
    commentsCount = Array.isArray(comList) ? comList.length : Array.isArray(comList?.comments) ? comList.comments.length : 0;
  } catch {}
  if (stale(gen)) return;

  main.innerHTML = "";
  const wrap = document.createElement("div");
  wrap.className = "hub-viewer";

  // Top header (Screen 01): 44px back chevron, mono path, 3 glyph buttons (36x36 drawn, 44x44 coarse hit)
  const bar = document.createElement("div");
  bar.className = "hub-viewer-bar";

  const back = document.createElement("button");
  back.type = "button";
  back.className = "hub-back";
  back.dataset.action = "viewer-back";
  back.setAttribute("aria-label", "Back to artifacts");
  back.innerHTML = glyphSvg("chevronBack", { size: 20 });
  back.addEventListener("click", viewerBack);

  const pathEl = document.createElement("div");
  pathEl.className = "hub-viewer-path mono";
  pathEl.textContent = monoPath;
  pathEl.title = monoPath;

  // Glyph button 1: start-a-thread or comments
  const threadBtn = document.createElement("button");
  threadBtn.type = "button";
  if (commentsCount > 0) {
    threadBtn.className = "hub-btn-glyph hub-comments-btn";
    threadBtn.dataset.action = "comments-toggle";
    threadBtn.setAttribute("aria-label", `Comments, ${commentsCount}`);
    threadBtn.innerHTML = `${glyphSvg("comments", { size: 20 })}<span class="hub-glyph-count mono" aria-hidden="true">${commentsCount}</span>`;
  } else {
    threadBtn.className = "hub-btn-glyph hub-start-thread";
    threadBtn.dataset.action = "start-thread";
    threadBtn.setAttribute("aria-label", "Start a thread");
    threadBtn.innerHTML = glyphSvg("threadNew", { size: 20 });
  }
  threadBtn.addEventListener("click", () => {
    openCommentsDrawer();
  });

  // Glyph button 2: copy-raw
  const copyRawBtn = document.createElement("button");
  copyRawBtn.type = "button";
  copyRawBtn.className = "hub-btn-glyph hub-copy-raw";
  copyRawBtn.dataset.action = "copy-raw";
  copyRawBtn.setAttribute("aria-label", "Copy raw text");
  copyRawBtn.innerHTML = glyphSvg("copyRaw", { size: 20 });

  const doCopyRaw = async () => {
    try {
      const text = await fetchRawText(id, viewer.version);
      try {
        if (navigator.clipboard && navigator.clipboard.writeText) {
          await navigator.clipboard.writeText(text);
        }
      } catch {}
      toast("Raw text copied");
    } catch {
      toast("Failed to copy raw text");
    }
  };
  copyRawBtn.addEventListener("click", doCopyRaw);

  // Glyph button 3: overflow
  const moreBtn = document.createElement("button");
  moreBtn.type = "button";
  moreBtn.className = "hub-btn-glyph hub-more";
  moreBtn.setAttribute("aria-label", "More");
  moreBtn.setAttribute("aria-expanded", "false");
  moreBtn.setAttribute("aria-haspopup", "true");
  moreBtn.innerHTML = glyphSvg("overflow", { size: 20 });

  const overflowMenu = document.createElement("div");
  overflowMenu.className = "hub-overflow-menu";
  overflowMenu.hidden = true;

  const menuItems = [
    { text: "Start a thread", action: "start-thread", run: () => openCommentsDrawer() },
    { text: "Comments", action: "comments-toggle", run: () => openCommentsDrawer() },
    { text: "Copy raw", action: "copy-raw", run: doCopyRaw },
    {
      text: "Copy path",
      action: "copy-path",
      run: async () => {
        try {
          if (navigator.clipboard && navigator.clipboard.writeText) {
            await navigator.clipboard.writeText(monoPath);
          }
        } catch {}
        toast("Path copied");
      },
    },
    {
      text: "Copy link",
      action: "copy-link",
      run: async () => {
        try {
          if (navigator.clipboard && navigator.clipboard.writeText) {
            await navigator.clipboard.writeText(location.href);
          }
        } catch {}
        toast("Link copied");
      },
    },
    {
      text: "Share",
      action: "share",
      run: () => {},
    },
    {
      text: "Open in browser",
      action: "open-in-browser",
      run: () => {
        const opened = document.documentElement.dataset.theme === "dark" ? "dark" : "light";
        window.open(frameSrc(id, viewer.version, opened), "_blank");
      },
    },
  ];

  for (const item of menuItems) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.dataset.action = item.action;
    btn.textContent = item.text;
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      overflowMenu.hidden = true;
      moreBtn.setAttribute("aria-expanded", "false");
      item.run();
    });
    overflowMenu.appendChild(btn);
  }

  moreBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    const open = overflowMenu.hidden;
    overflowMenu.hidden = !open;
    moreBtn.setAttribute("aria-expanded", String(open));
  });
  document.addEventListener("click", (e) => {
    if (!overflowMenu.hidden && !overflowMenu.contains(e.target) && !e.target.closest(".hub-more")) {
      overflowMenu.hidden = true;
      moreBtn.setAttribute("aria-expanded", "false");
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !overflowMenu.hidden) {
      e.stopPropagation();
      overflowMenu.hidden = true;
      moreBtn.setAttribute("aria-expanded", "false");
      moreBtn.focus();
    }
  });

  bar.append(back, pathEl, threadBtn, copyRawBtn, moreBtn);
  wrap.append(bar, overflowMenu);

  // Document meta line with version toggle (Screen 01):
  const metaWrap = document.createElement("div");
  metaWrap.className = "hub-viewer-meta-wrap";

  const metaLine = document.createElement("div");
  metaLine.className = "hub-viewer-meta";

  const actorSpan = document.createElement("span");
  actorSpan.className = "hub-viewer-actor";
  actorSpan.textContent = current.actor || "agent";

  const dot1 = document.createElement("span");
  dot1.textContent = "·";

  const versionToggle = document.createElement("button");
  versionToggle.type = "button";
  versionToggle.className = "hub-version-toggle mono";
  versionToggle.dataset.action = "version-toggle";
  versionToggle.setAttribute("aria-expanded", "false");
  versionToggle.setAttribute("aria-controls", "hub-version-menu");
  versionToggle.setAttribute("aria-label", `Version ${shown}`);
  versionToggle.innerHTML = `v${shown} of ${versions.length} ${glyphSvg("chevronDown", { size: 11, strokeWidth: 2 })}`;

  const dot2 = document.createElement("span");
  dot2.textContent = "·";

  const sizeSpan = document.createElement("span");
  sizeSpan.className = "mono hub-viewer-size";
  sizeSpan.textContent = formatBytes(current.size_bytes);

  const dot3 = document.createElement("span");
  dot3.textContent = "·";

  const ageSpan = document.createElement("span");
  ageSpan.className = "hub-viewer-age";
  ageSpan.textContent = current.created_at ? relative(Date.parse(current.created_at)) : "";

  metaLine.append(actorSpan, dot1, versionToggle, dot2, sizeSpan, dot3, ageSpan);
  metaWrap.appendChild(metaLine);
  wrap.appendChild(metaWrap);

  // Sandboxed frame: replaces element rather than src on navigation to avoid pushing history
  const opened = document.documentElement.dataset.theme === "dark" ? "dark" : "light";
  const frame = document.createElement("iframe");
  frame.id = "hub-frame";
  frame.dataset.id = id;
  frame.dataset.kind = current.kind;
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("title", current.title);
  frame.setAttribute("data-theme", opened);
  frame.src = frameSrc(id, viewer.version, opened);
  wrap.appendChild(frame);

  // Auto-size frame to avoid inner scrollbar
  const onHeight = (event) => {
    if (event.source !== frame.contentWindow) return;
    const h = event.data && event.data.hubFrameHeight;
    if (typeof h !== "number" || !isFinite(h)) return;
    frame.style.height = `${Math.max(Math.round(h), 200)}px`;
  };
  window.addEventListener("message", onHeight);

  // Comments footer strip (Screen 01) when thread exists
  const { toggle, badge } = commentsToggle();
  toggle.style.display = "none";
  wrap.appendChild(toggle);
  if (commentsCount > 0) {
    const strip = document.createElement("button");
    strip.type = "button";
    strip.className = "hub-comments-strip comments-toggle";
    strip.dataset.action = "comments-toggle";
    strip.innerHTML = `
      ${glyphSvg("comments", { size: 16 })}
      <span class="grow" style="text-align:left">Comments</span>
      <span class="mono hub-comments-count">${commentsCount}</span>
      ${glyphSvg("chevronRight", { size: 16 })}
    `;
    strip.addEventListener("click", () => {
      openCommentsDrawer();
    });
    wrap.appendChild(strip);
  }

  // Version sheet (Screen 02)
  const { backdrop, sheet } = buildVersionSheet(id, versions, shown, projectId);
  wrap.append(backdrop, sheet);

  main.appendChild(wrap);
  const { backdrop: comBackdrop, drawer: comDrawer } = commentsPanel({ toggle, badge });
  main.append(comBackdrop, comDrawer);
}

// The old viewer name, kept so a caller that referenced it still resolves.
export { viewerRoute as showArtifact };
