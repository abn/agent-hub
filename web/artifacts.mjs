// Artifacts: the per-project gallery, and the viewer that embeds an artifact's
// own public page next to the comments drawer. The viewer is a route of its
// own, so reload and the browser's Back both keep the artifact on screen.

import { api } from "./api.mjs";
import {
  closeCommentsDrawer,
  commentsPanel,
  commentsState,
  commentsToggle,
  openCommentsDrawer,
  openCompose,
  renderDesktopCards,
  startComments,
} from "./comments.mjs";
import { confirmAction } from "./dialog.mjs";
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

const previewCache = new Map();

// What a card shows of a document it has not opened.
//
// This was the first five lines of the raw text, which for an HTML artifact
// is doctype, html, head and a placeholder title: the same five lines in
// every HTML document ever written, so a gallery of them was twelve
// identical grey boxes. Markdown was fine, which is why it went unseen.
//
// The result is only ever written with textContent, never as markup.
export function previewSnippet(text) {
  if (!/^\s*<(!doctype|html)\b/i.test(text)) {
    return text.split("\n").slice(0, 5).join("\n").slice(0, 200);
  }
  const body = text
    .replace(/<head\b[\s\S]*?<\/head>/gi, " ")
    // noscript and template are the trap here. A page that renders itself with
    // JavaScript carries a "please enable JavaScript" block as its only static
    // prose, so stripping tags alone surfaced that on every card: still the
    // same words everywhere, just a longer set of them.
    .replace(/<(script|style|noscript|template|svg)\b[\s\S]*?<\/\1>/gi, " ")
    .replace(/<!--[\s\S]*?-->/g, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/gi, " ")
    .replace(/\s+/g, " ")
    .trim();
  // A page whose words all live in script tags leaves nothing. Say so rather
  // than drawing an empty box that looks like a failed load.
  return body ? body.slice(0, 200) : "A web page";
}
let activeGrouping = "day";
let activeView = "cards";

export function artifactCard(artifact) {
  const enc = artifact.protected ? " encrypted" : " plain";
  const age = artifact.created_at || artifact.updated_at ? relative(Date.parse(artifact.created_at || artifact.updated_at)) : "";
  const actor = artifact.actor || "agent";
  const encLabel = artifact.protected ? " · encrypted" : "";

  let previewContent = "";
  if (artifact.protected) {
    previewContent = `<span class="artifact-preview-lock" aria-hidden="true">${cardGlyph(true)}</span>`;
  } else {
    const cachedSnippet = previewCache.get(artifact.id) || (artifact.description ? artifact.description.slice(0, 200) : "");
    // Two marks, one shown at a time by width. The source snippet is a
    // desktop affordance: at 390px it filled two thirds of every card, was
    // too small to read, and left three artifacts on a screen. The phone
    // gets the glyph and a row it can scan. The inline `display:none` that
    // used to hide the glyph is gone, because an inline style cannot be
    // answered by a media query.
    previewContent = `
      <span class="artifact-preview-text mono" aria-hidden="true" data-preview-id="${esc(artifact.id)}">${esc(cachedSnippet)}</span>
      <svg class="doc artifact-preview-glyph" aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="${DOC_PATH}"></path></svg>
    `;
  }

  const commentCount = artifact.comments_count || 0;
  const commentsBadge = commentCount > 0 ? `<span class="artifact-comments" aria-label="${commentCount} comments"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 5h14v10H9l-4 4z"></path></svg>${commentCount}</span>` : "";

  return `<button type="button" class="artifact-card artifact-row" data-action="artifact-open" data-id="${esc(artifact.id)}">
    <span class="artifact-preview${enc}" aria-hidden="true">${previewContent}</span>
    <span class="artifact-body">
      <span class="artifact-title">${esc(artifact.title)}</span>
      <span class="artifact-meta mono">${esc(actor)} · <span class="mono">v${artifact.version} · ${formatBytes(artifact.size_bytes)}</span> · ${age}${encLabel}</span>
      ${commentsBadge}
    </span>
    <svg class="artifact-chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 5l7 7-7 7"></path></svg>
  </button>`;
}

function renderArtifactsTable(artifacts) {
  const rows = artifacts.map((a) => {
    const age = a.created_at || a.updated_at ? relative(Date.parse(a.created_at || a.updated_at)) : "";
    const encBadge = a.protected ? ` · <span class="hub-lock-pill">encrypted</span>` : "";
    return `<tr class="artifact-card artifact-table-row" data-action="artifact-open" data-id="${esc(a.id)}" tabindex="0">
      <td>
        <div class="artifact-table-title">
          ${a.protected ? `<svg class="lock" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><path d="${LOCK_PATH}"></path></svg>` : `<svg class="doc" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><path d="${DOC_PATH}"></path></svg>`}
          <span>${esc(a.title)}</span>
        </div>
      </td>
      <td class="mono">v${a.version}</td>
      <td class="mono">${formatBytes(a.size_bytes)}</td>
      <td class="mono">${age}${encBadge}</td>
    </tr>`;
  }).join("");

  return `
    <div class="hub-artifacts-table-wrap">
      <table class="hub-artifacts-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Version</th>
            <th>Size</th>
            <th>Updated</th>
          </tr>
        </thead>
        <tbody>
          ${rows}
        </tbody>
      </table>
    </div>
  `;
}

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
  const totalVersions = artifacts.reduce((acc, a) => acc + (Number(a.version) || 1), 0);
  const groups = groupArtifacts(artifacts, activeGrouping);

  const pillLabel = activeGrouping === "agent" ? "Agent" : activeGrouping === "kind" ? "Kind" : "Date";

  // Pre-fetch previews for plain artifacts asynchronously
  for (const a of artifacts) {
    if (!a.protected && !previewCache.has(a.id)) {
      fetchRawText(a.id).then((text) => {
        if (!text) return;
        const snippet = previewSnippet(text);
        previewCache.set(a.id, snippet);
        const el = document.querySelector(`.artifact-preview-text[data-preview-id="${a.id}"]`);
        if (el) el.textContent = snippet;
      }).catch(() => {});
    }
  }

  // The table is a desktop view and its switch is not drawn below 900px. A
  // reader who chose it on a wide window and then narrowed would otherwise be
  // left in a view with no way out of it.
  const wide = typeof window === "undefined" || window.innerWidth >= 900;
  let contentHTML = "";
  if (activeView === "table" && wide) {
    contentHTML = renderArtifactsTable(artifacts);
  } else {
    contentHTML = groups
      .map(
        (g) =>
          `<div class="hub-group-header mono">${esc(g.title)} · ${g.items.length}</div>
           <div class="hub-artifacts-grid gallery">${g.items.map(artifactCard).join("")}</div>`
      )
      .join("");
  }

  const versionsText = `${totalVersions} ${totalVersions === 1 ? "version" : "versions"}`;
  const artifactsText = `${artifacts.length} ${artifacts.length === 1 ? "artifact" : "artifacts"}`;

  return `
    <div class="hub-artifacts-summary">
      <div class="hub-group-wrap">
        <button type="button" class="hub-group-toggle" id="hub-group-toggle" data-action="artifact-group-toggle" aria-haspopup="true" aria-expanded="false">Group${glyphSvg("chevronDown", { size: 12, strokeWidth: 2 })}</button>
        <span class="hub-group-pill pill">${pillLabel}</span>
        <div class="hub-group-menu" id="hub-group-menu" hidden>
          <button type="button" data-group="day"${activeGrouping === "day" ? ' class="active" aria-current="true"' : ""}>Date</button>
          <button type="button" data-group="agent"${activeGrouping === "agent" ? ' class="active" aria-current="true"' : ""}>Agent</button>
          <button type="button" data-group="kind"${activeGrouping === "kind" ? ' class="active" aria-current="true"' : ""}>Kind</button>
        </div>
      </div>
      <span class="hub-summary-divider"></span>
      <div role="group" aria-label="View" class="hub-view-segment">
        <button type="button" class="hub-view-btn cards${activeView === "cards" ? " active" : ""}" aria-pressed="${activeView === "cards"}" data-view="cards">Cards</button>
        <button type="button" class="hub-view-btn table${activeView === "table" ? " active" : ""}" aria-pressed="${activeView === "table"}" data-view="table">Table</button>
      </div>
      <div class="grow"></div>
      <span class="hub-artifacts-counts mono">${artifactsText} · ${versionsText} · ${formatBytes(totalBytes)}</span>
    </div>
    <div class="hub-artifacts-list">
      ${contentHTML}
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
  const viewBtn = e.target.closest(".hub-view-btn[data-view]");
  if (viewBtn) {
    activeView = viewBtn.dataset.view;
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
  const tableRow = e.target.closest(".artifact-table-row[data-id]");
  if (tableRow && !e.target.closest("button, a")) {
    openArtifact(tableRow.dataset.id);
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

// The artifacts index in the one shell: one row per artifact, the same shape
// as every other list, with the fixed glyph column so every title starts at
// the same x. The document itself is read in the stage.
export async function artifactIndex(projectId) {
  const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts`);
  if (!artifacts || !artifacts.length) {
    return { rows: `<div class="shell-body-pad"><p class="empty">No artifacts yet.</p></div>`, artifacts: [] };
  }
  const rows = artifacts
    .map((a) => {
      const comments = a.comments_count
        ? ` · ${a.comments_count} comment${a.comments_count === 1 ? "" : "s"}`
        : "";
      const href = `#/artifacts/${encodeURIComponent(a.id)}?project=${encodeURIComponent(projectId)}`;
      const lock = a.protected
        ? `<span class="sr-only">Encrypted</span>`
        : "";
      return `<div class="row artifact-row" data-id="${esc(a.id)}">
        <span class="row-glyph" aria-hidden="true">${cardGlyph(a.protected)}</span>
        <div class="grow">
          <a class="title" href="${esc(href)}">${esc(a.title || "artifact")}${lock}</a>
          <div class="meta mono">v${a.version || 1} · ${formatBytes(a.size_bytes)}${comments}</div>
        </div>
      </div>`;
    })
    .join("");
  return { rows, artifacts };
}

export function openArtifact(id) {
  const parts = location.hash.replace(/^#/, "").split("/");
  const project = parts[1] === "projects" ? parts[2] : "";
  const base = `#/artifacts/${encodeURIComponent(id)}`;
  location.hash = project ? `${base}?project=${encodeURIComponent(project)}` : base;
}

export function viewerBack() {
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  const project = params.get("project") || viewer.project;
  location.hash = project ? `#/projects/${encodeURIComponent(project)}/artifacts` : "#/projects";
}

const viewer = { id: null, version: null, kind: null, raw: false, project: null };

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

// Share sheet (Screens 08 & 09): opens from overflow menu
function buildShareSheet(id, current, shown, moreBtn) {
  const backdrop = document.createElement("div");
  backdrop.className = "hub-share-backdrop";
  backdrop.hidden = true;

  const sheet = document.createElement("div");
  sheet.className = "hub-share-sheet";
  sheet.hidden = true;
  sheet.setAttribute("role", "dialog");
  sheet.setAttribute("aria-modal", "true");
  sheet.setAttribute("aria-label", "Share this artifact");

  const handle = document.createElement("div");
  handle.className = "hub-share-handle";

  const header = document.createElement("div");
  header.className = "hub-share-header";
  header.innerHTML = `
    <span class="hub-share-title">Share this artifact</span>
    <span class="hub-share-subline">${esc(current.title || "")} · <span class="mono">v${shown}</span></span>
  `;

  const shareUrl = `${location.origin}/artifacts/${encodeURIComponent(id)}`;

  const linkRow = document.createElement("div");
  linkRow.className = "hub-share-link-row";
  linkRow.innerHTML = `
    ${glyphSvg("link", { size: 18 })}
    <span class="hub-share-url mono">${esc(shareUrl)}</span>
    <button type="button" class="hub-share-copy-btn" data-action="copy-link">Copy</button>
  `;

  const copyLinkBtn = linkRow.querySelector('button[data-action="copy-link"]');
  copyLinkBtn.addEventListener("click", async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(shareUrl);
      }
    } catch {}
    toast("Link copied");
  });

  const helper = document.createElement("div");
  helper.className = "hub-share-helper";
  helper.textContent = "Anyone with this link can open it. No account, no sign-in.";

  let isLocked = false;
  let hasLink = false;
  let password = "";
  let showPassword = false;

  function generatePassword() {
    const chars = "abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!@#$%&*";
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    let res = "";
    for (let i = 0; i < 16; i++) {
      res += chars[bytes[i] % chars.length];
    }
    return res;
  }

  const pwRow = document.createElement("div");
  pwRow.className = "hub-share-pw-row";
  pwRow.innerHTML = `
    <span class="hub-share-pw-icon">${glyphSvg("lock", { size: 18 })}</span>
    <span class="hub-share-pw-info">
      <span class="hub-share-pw-label">Lock with a password</span>
      <span class="hub-share-pw-helper">Encrypts this artifact. The hub cannot read it, and cannot recover it if the password is lost.</span>
    </span>
    <button type="button" role="switch" aria-checked="false" aria-label="Lock with a password" class="hub-share-switch">
      <span class="hub-share-switch-track"><span class="hub-share-switch-thumb"></span></span>
    </button>
  `;

  const pwHelper = pwRow.querySelector(".hub-share-pw-helper");
  const switchBtn = pwRow.querySelector(".hub-share-switch");

  const pwControls = document.createElement("div");
  pwControls.className = "hub-share-pw-controls";
  pwControls.hidden = true;
  pwControls.innerHTML = `
    <div class="hub-share-pw-field">
      <input type="password" class="hub-share-pw-input mono" placeholder="Password" aria-label="Password">
      <button type="button" class="hub-share-pw-toggle" aria-label="Show password">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 2 12s3.6-6 10-6 10 6 10 6-3.6 6-10 6-10-6-10-6z"/><circle cx="12" cy="12" r="2.6"/></svg>
      </button>
    </div>
    <div class="hub-share-pw-gen-row">
      <button type="button" class="hub-share-pw-gen" data-action="generate-password">${glyphSvg("key", { size: 16 })}Generate</button>
      <span style="font-size: 12px; line-height: 1.45; color: var(--ink-2);">Send it by another route than the link.</span>
    </div>
    <div class="hub-share-cost">Encryption happens here, not on the server. If this password is lost the artifact is unreadable by everyone, including us. Existing readers of the current link will be asked for it.</div>
  `;

  const pwInput = pwControls.querySelector(".hub-share-pw-input");
  const pwToggle = pwControls.querySelector(".hub-share-pw-toggle");
  const genBtn = pwControls.querySelector('button[data-action="generate-password"]');

  pwInput.addEventListener("input", () => {
    password = pwInput.value;
  });

  pwToggle.addEventListener("click", () => {
    showPassword = !showPassword;
    pwInput.type = showPassword ? "text" : "password";
    pwToggle.setAttribute("aria-label", showPassword ? "Hide password" : "Show password");
  });

  genBtn.addEventListener("click", () => {
    password = generatePassword();
    pwInput.value = password;
  });

  const actions = document.createElement("div");
  actions.className = "hub-share-actions";
  actions.innerHTML = `
    <div class="hub-share-status">
      <span class="hub-share-status-dot"></span>
      <span>This artifact has no link yet</span>
    </div>
    <button type="button" class="hub-share-primary">Create link</button>
  `;

  const statusRow = actions.querySelector(".hub-share-status");
  const primaryBtn = actions.querySelector(".hub-share-primary");

  const copyPwBtn = document.createElement("button");
  copyPwBtn.type = "button";
  copyPwBtn.className = "hub-share-copy-pw-btn";
  copyPwBtn.dataset.action = "copy-password";
  copyPwBtn.textContent = "Copy password";
  copyPwBtn.addEventListener("click", async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(pwInput.value || password);
      }
    } catch {}
    toast("Password copied");
  });

  const postNote = document.createElement("div");
  postNote.className = "hub-share-post-note";
  postNote.hidden = true;
  postNote.innerHTML = `
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 5 12l5 5 9-9"/></svg>
    <span>After creating, the sheet offers Copy link and Copy password separately.</span>
  `;

  switchBtn.addEventListener("click", () => {
    isLocked = !isLocked;
    switchBtn.setAttribute("aria-checked", String(isLocked));
    if (isLocked) {
      pwRow.classList.add("is-on");
      pwHelper.textContent = "On. The artifact is encrypted before it leaves this device.";
      pwControls.hidden = false;
      postNote.hidden = false;
      if (!hasLink) {
        primaryBtn.textContent = "Create locked link";
      }
      if (!password) {
        password = generatePassword();
        pwInput.value = password;
      }
    } else {
      pwRow.classList.remove("is-on");
      pwHelper.textContent = "Encrypts this artifact. The hub cannot read it, and cannot recover it if the password is lost.";
      pwControls.hidden = true;
      postNote.hidden = true;
      copyPwBtn.remove();
      if (!hasLink) {
        primaryBtn.textContent = "Create link";
      }
    }
  });

  primaryBtn.addEventListener("click", async () => {
    if (!hasLink) {
      hasLink = true;
      statusRow.hidden = true;
      primaryBtn.textContent = "Revoke link";
      primaryBtn.classList.add("is-revoking");
      if (isLocked) {
        if (!actions.contains(copyPwBtn)) {
          actions.insertBefore(copyPwBtn, primaryBtn);
        }
      }
    } else {
      const ok = await confirmAction({
        title: "Revoke public link?",
        body: "The old URL stops working immediately. Anyone with the link will no longer be able to open this artifact.",
        safe: "Keep link",
        danger: "Revoke link",
      });
      if (ok) {
        hasLink = false;
        statusRow.hidden = false;
        primaryBtn.textContent = isLocked ? "Create locked link" : "Create link";
        primaryBtn.classList.remove("is-revoking");
        copyPwBtn.remove();
      }
    }
  });

  sheet.append(handle, header, linkRow, helper, pwRow, pwControls, actions, postNote);

  const closeSheet = () => {
    backdrop.hidden = true;
    sheet.hidden = true;
    if (moreBtn) {
      moreBtn.setAttribute("aria-expanded", "false");
      moreBtn.focus();
    }
  };

  const openSheet = () => {
    backdrop.hidden = false;
    sheet.hidden = false;
    primaryBtn.focus();
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

  return { backdrop, sheet, openSheet, closeSheet };
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
  startComments(id, shown, current.protected);

  // Mono path: {project} / {slug}
  const projDisplay = projectId || current.project_id || "agent-hub";
  viewer.project = projDisplay;
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

  let projectArtifacts = [];
  try {
    const listRes = await api(`/api/v1/projects/${encodeURIComponent(projDisplay)}/artifacts`);
    projectArtifacts = listRes.artifacts || [];
  } catch {}
  if (!projectArtifacts.length) {
    projectArtifacts = [current];
  }
  if (stale(gen)) return;

  main.innerHTML = "";
  const wrap = document.createElement("div");
  wrap.className = "hub-viewer panes panes-artifacts has-selection";

  // Left index column: 280px for reading runs of artifacts
  const indexCol = document.createElement("aside");
  indexCol.className = "pane-index hub-viewer-index";
  indexCol.setAttribute("aria-label", "Artifacts index");

  const indexHead = document.createElement("div");
  indexHead.className = "hub-viewer-index-head";
  const indexTitle = document.createElement("span");
  indexTitle.className = "hub-viewer-index-title";
  indexTitle.textContent = "Artifacts";
  const indexCount = document.createElement("span");
  indexCount.className = "mono hub-viewer-index-count";
  indexCount.textContent = String(projectArtifacts.length);
  indexHead.append(indexTitle, indexCount);
  indexCol.appendChild(indexHead);

  const indexList = document.createElement("div");
  indexList.className = "hub-viewer-index-list";

  for (const a of projectArtifacts) {
    const isCurrent = a.id === id;
    const item = document.createElement("a");
    item.href = `#/artifacts/${encodeURIComponent(a.id)}?project=${encodeURIComponent(projDisplay)}`;
    item.className = "hub-viewer-index-item" + (isCurrent ? " active" : "");
    if (isCurrent) item.setAttribute("aria-current", "page");

    const tRow = document.createElement("span");
    tRow.className = "hub-viewer-index-item-title";
    if (a.protected) {
      const lockIcon = document.createElement("span");
      lockIcon.innerHTML = cardGlyph(true);
      tRow.appendChild(lockIcon);
    }
    const tText = document.createElement("span");
    tText.textContent = a.title || "artifact";
    tRow.appendChild(tText);

    const mRow = document.createElement("span");
    mRow.className = "hub-viewer-index-item-meta mono";
    const commentsInfo = a.comments_count ? ` · ${a.comments_count} comment${a.comments_count === 1 ? "" : "s"}` : "";
    mRow.textContent = `v${a.version || 1} · ${formatBytes(a.size_bytes)}${commentsInfo}`;

    item.append(tRow, mRow);
    indexList.appendChild(item);
  }
  indexCol.appendChild(indexList);

  // Top header (Screen 01): 44px back chevron, mono path, 3 glyph buttons (36x36 drawn, 44x44 coarse hit)
  const bar = document.createElement("div");
  bar.className = "hub-viewer-bar";

  const back = document.createElement("button");
  back.type = "button";
  back.className = "hub-back";
  back.dataset.action = "viewer-back";
  back.setAttribute("aria-label", "Back to artifacts");
  back.innerHTML = glyphSvg("chevronBack", { size: 20 });

  const pathEl = document.createElement("div");
  pathEl.className = "hub-viewer-path mono";
  pathEl.textContent = monoPath;
  pathEl.title = monoPath;

  // One control in two states, not two controls. A bubble with a plus and a
  // bubble with a count were never on screen together, so the set carried two
  // glyphs for one place; the bubble alone is the empty state and the numeral
  // beside it is the full one. What the control does still differs, because
  // there is nothing to open until a thread exists.
  const threadBtn = document.createElement("button");
  threadBtn.type = "button";
  const hasThreads = commentsCount > 0;
  threadBtn.className = hasThreads
    ? "hub-btn-glyph hub-comments-btn"
    : "hub-btn-glyph hub-start-thread";
  threadBtn.dataset.action = hasThreads ? "comments-toggle" : "start-thread";
  threadBtn.setAttribute("aria-label", hasThreads ? `Comments, ${commentsCount}` : "Start a thread");
  threadBtn.innerHTML =
    glyphSvg("comments", { size: 20 }) +
    (hasThreads
      ? `<span class="hub-glyph-count mono" aria-hidden="true">${commentsCount}</span>`
      : "");
  threadBtn.addEventListener("click", () => {
    if (!hasThreads) {
      openCompose(null);
    } else if (commentsState.open && commentsState.viewMode === "list") {
      closeCommentsDrawer();
    } else {
      openCommentsDrawer();
    }
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

  let openShareSheet = () => {};

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
      run: () => openShareSheet(),
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

  if (current.protected) {
    const lockedSpan = document.createElement("span");
    lockedSpan.className = "hub-viewer-locked";
    lockedSpan.textContent = "Locked · only readable with the password";
    metaLine.append(lockedSpan, dot1, versionToggle, dot2, sizeSpan, dot3, ageSpan);
  } else {
    metaLine.append(actorSpan, dot1, versionToggle, dot2, sizeSpan, dot3, ageSpan);
  }
  metaWrap.appendChild(metaLine);

  // Center Stage: 640px document
  const stageCol = document.createElement("div");
  stageCol.className = "pane-stage hub-viewer-stage";

  const content = document.createElement("div");
  content.className = "hub-viewer-content";

  const docCol = document.createElement("div");
  docCol.className = "hub-viewer-doc";

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
  docCol.appendChild(frame);

  content.appendChild(docCol);
  stageCol.append(bar, overflowMenu, metaWrap, content);

  // Right Comments Column (Aside): 320px
  const commentsCol = document.createElement("aside");
  commentsCol.className = "pane-aside hub-comments-column";
  commentsCol.setAttribute("aria-label", "Comments");

  const commentsHead = document.createElement("div");
  commentsHead.className = "hub-comments-head";
  const cTitle = document.createElement("span");
  cTitle.className = "hub-comments-head-title";
  cTitle.textContent = "Comments";
  const cMeta = document.createElement("span");
  cMeta.className = "mono hub-comments-head-meta";
  cMeta.textContent = `${commentsCount} · v${shown}`;
  const cAdd = document.createElement("button");
  cAdd.type = "button";
  cAdd.className = "hub-comments-head-add";
  cAdd.dataset.action = "start-thread";
  cAdd.setAttribute("aria-label", "New thread");
  cAdd.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M12 6v12M6 12h12"></path></svg>`;
  cAdd.addEventListener("click", () => openCompose(null));

  commentsHead.append(cTitle, cMeta, cAdd);

  const cardsList = document.createElement("div");
  cardsList.className = "hub-comments-cards-list";

  const commentsFooter = document.createElement("div");
  commentsFooter.className = "hub-comments-footer";
  const cHelper = document.createElement("div");
  cHelper.className = "hub-comments-helper";
  cHelper.textContent = "Select text to comment on it";
  commentsFooter.appendChild(cHelper);

  commentsCol.append(commentsHead, cardsList, commentsFooter);

  // Auto-size frame to avoid inner scrollbar
  const onHeight = (event) => {
    if (event.source !== frame.contentWindow) return;
    const h = event.data && event.data.hubFrameHeight;
    if (typeof h !== "number" || !isFinite(h)) return;
    frame.style.height = `${Math.max(Math.round(h), 200)}px`;
  };
  window.addEventListener("message", onHeight);

  // Comments footer strip (Screen 01) when thread exists on mobile
  const { toggle, badge } = commentsToggle();
  toggle.style.display = "none";
  stageCol.appendChild(toggle);
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
    stageCol.appendChild(strip);
  }

  // Version sheet (Screen 02)
  const { backdrop, sheet } = buildVersionSheet(id, versions, shown, projectId);
  stageCol.append(backdrop, sheet);

  // Share sheet (Screens 08 & 09)
  const share = buildShareSheet(id, current, shown, moreBtn);
  openShareSheet = share.openSheet;
  stageCol.append(share.backdrop, share.sheet);

  wrap.append(indexCol, stageCol, commentsCol);
  main.appendChild(wrap);

  const { backdrop: comBackdrop, drawer: comDrawer } = commentsPanel({ toggle, badge });
  main.append(comBackdrop, comDrawer);

  commentsState.desktopContainer = cardsList;
  renderDesktopCards();
}

// The old viewer name, kept so a caller that referenced it still resolves.
export { viewerRoute as showArtifact };
