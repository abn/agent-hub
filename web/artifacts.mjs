// Artifacts: the per-project gallery, and the viewer that embeds an artifact's
// own public page next to the comments drawer. The viewer is a route of its
// own, so reload and the browser's Back both keep the artifact on screen.

import { api } from "./api.mjs";
import { commentsPanel, commentsToggle, startComments } from "./comments.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { relative } from "./time.mjs";

const DOC_PATH = "M 6 3h9l4 4v14H6z M 8 12h8 M 8 16h8";
const LOCK_PATH = "M 7 11V8a5 5 0 0 1 10 0v3 M 5 11h14v10H5z";

function cardGlyph(protectedArtifact) {
  const path = protectedArtifact ? LOCK_PATH : DOC_PATH;
  const cls = protectedArtifact ? "lock" : "doc";
  return `<svg class="${cls}" aria-hidden="true" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="${path}"></path></svg>`;
}

function formatBytes(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function cardMeta(artifact) {
  const age = artifact.updated_at ? relative(Date.parse(artifact.updated_at)) : "";
  return `v${artifact.version} · ${formatBytes(artifact.size_bytes)} · ${age}`;
}

export function artifactCard(artifact) {
  return `<button type="button" class="artifact-card" data-action="artifact-open" data-id="${esc(artifact.id)}">
    <span class="artifact-preview">${cardGlyph(artifact.protected)}</span>
    <span class="artifact-body">
      <span class="artifact-title">${esc(artifact.title)}</span>
      <span class="artifact-meta mono">${cardMeta(artifact)}</span>
    </span>
  </button>`;
}

// The gallery the project view's Artifacts segment paints. Only the listing
// is fetched: the preview tiles are the kind glyph and the lock glyph, never
// the body content.
export async function gallerySection(projectId) {
  const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts`);
  if (!artifacts.length) return emptyStateHTML(EMPTY_COPY.artifacts);
  return `<div class="gallery">${artifacts.map(artifactCard).join("")}</div>`;
}

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
// so the viewer names the one it wants in the address.
function frameSrc(id, version, theme) {
  const params = new URLSearchParams();
  if (version) params.set("version", String(version));
  if (theme) params.set("theme", theme);
  const query = params.toString();
  const address = `/artifacts/${encodeURIComponent(id)}`;
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
    const query = viewer.version ? `?version=${viewer.version}` : "";
    const raw = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/raw${query}`);
    const text = typeof raw === "string" ? raw : JSON.stringify(raw, null, 2);
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
  if (!menu) return;
  const open = menu.hidden;
  menu.hidden = !open;
  button.setAttribute("aria-expanded", String(open));
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

// The theme control. The framed public page carries its own theme switch; an
// html artifact can be re-framed through the theme-aware frame route, so the
// host control does that and reloads the public page for the kinds whose theme
// the page owns.
export function toggleViewerTheme() {
  const frame = main.querySelector("#hub-frame");
  if (!frame) return;
  const next = frame.getAttribute("data-theme") === "dark" ? "light" : "dark";
  frame.setAttribute("data-theme", next);
  drawThemeControl(main.querySelector("#hub-theme-toggle"), next);
  let src;
  if (frame.dataset.kind === "html") {
    const params = new URLSearchParams();
    if (viewer.version) params.set("version", String(viewer.version));
    params.set("theme", next);
    src = `/artifacts/${encodeURIComponent(frame.dataset.id)}/frame?${params.toString()}`;
  } else {
    src = viewerSource(frame);
  }
  replaceFrame(frame, src);
}

// One glyph at a time: the one for the theme a press switches to, which is
// what the control's name says too.
function drawThemeControl(button, theme) {
  if (!button) return;
  const to = theme === "dark" ? "light" : "dark";
  for (const glyph of button.querySelectorAll("svg")) {
    glyph.toggleAttribute("hidden", glyph.dataset.to !== to);
  }
  button.setAttribute("aria-label", `Switch to ${to} theme`);
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

function chromeTitle() {
  const el = document.createElement("div");
  el.className = "hub-title";
  return el;
}

// The version menu, listed newest first so the current head is the first item.
function versionMenu(id, versions, shown) {
  const menu = document.createElement("div");
  menu.id = "hub-version-menu";
  menu.className = "hub-version-menu";
  menu.hidden = true;
  menu.setAttribute("role", "group");
  menu.setAttribute("aria-label", "Versions");
  for (const version of versions) {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.action = "version-pick";
    button.dataset.id = id;
    button.dataset.version = String(version.version);
    if (version.version === shown) button.setAttribute("aria-current", "true");
    const age = version.created_at ? relative(Date.parse(version.created_at)) : "";
    button.textContent = `v${version.version} · ${formatBytes(version.size_bytes)} · ${age}`;
    menu.appendChild(button);
  }
  return menu;
}

export async function viewerRoute(params, gen, path) {
  const parts = (path || "").split("/");
  const id = parts[2];
  if (!id) {
    location.hash = "#/projects";
    return;
  }
  const version = params.get("version");
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
  const shown = viewer.version || (versions[0] && versions[0].version) || 1;
  const current = versions.find((v) => v.version === shown) || versions[0];
  if (!current) {
    paint(gen, `<h1>Artifact</h1><p class="error">No versions of this artifact.</p>`);
    return;
  }
  viewer.version = shown;
  viewer.kind = current.kind;

  main.innerHTML = "";
  const wrap = document.createElement("div");
  wrap.className = "hub-viewer";

  const bar = document.createElement("div");
  bar.className = "hub-viewer-bar";

  const back = document.createElement("button");
  back.type = "button";
  back.className = "hub-back";
  back.dataset.action = "viewer-back";
  back.setAttribute("aria-label", "Back to artifacts");
  back.appendChild(svg("M 15 6l-6 6 6 6", 20, 20, null));

  const block = document.createElement("div");
  block.className = "grow";
  const title = document.createElement("div");
  title.className = "hub-title";
  title.textContent = current.title;
  const meta = document.createElement("div");
  meta.className = "hub-meta mono";
  const age = current.created_at ? relative(Date.parse(current.created_at)) : "";
  meta.textContent = `v${shown} · ${formatBytes(current.size_bytes)} · ${age}`;
  block.append(title, meta);

  const { toggle, badge } = commentsToggle();

  const theme = document.createElement("button");
  theme.type = "button";
  theme.id = "hub-theme-toggle";
  theme.dataset.action = "viewer-theme";
  const sun = svg("M 12 19v1 M 12 4v1 M 4 12h1 M 19 12h1 M 6 6l.7.7 M 17.3 17.3l.7.7 M 6 18l.7-.7 M 17.3 6.7l.7-.7 M 12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", 18, 18, null);
  const moon = svg("M 20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z", 18, 18, null);
  sun.dataset.to = "light";
  moon.dataset.to = "dark";
  theme.append(sun, moon);
  // The frame opens in the app's own theme, and the control starts from there.
  const opened = document.documentElement.dataset.theme === "dark" ? "dark" : "light";
  drawThemeControl(theme, opened);

  const versionToggle = document.createElement("button");
  versionToggle.type = "button";
  versionToggle.className = "hub-version-toggle";
  versionToggle.dataset.action = "version-toggle";
  versionToggle.setAttribute("aria-expanded", "false");
  versionToggle.setAttribute("aria-controls", "hub-version-menu");
  versionToggle.textContent = `v${shown} `;
  versionToggle.appendChild(svg("M 6 9l6 6 6-6", 12, 12, "chev"));

  const raw = document.createElement("button");
  raw.type = "button";
  raw.className = "hub-raw";
  raw.dataset.action = "viewer-raw";
  raw.textContent = "Open raw";

  bar.append(back, block, toggle, theme, versionToggle, raw);
  wrap.appendChild(bar);

  const menu = versionMenu(id, versions, shown);
  wrap.appendChild(menu);

  const frame = document.createElement("iframe");
  frame.id = "hub-frame";
  frame.dataset.id = id;
  frame.dataset.kind = current.kind;
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("title", "Artifact");
  frame.setAttribute("data-theme", opened);
  frame.src = frameSrc(id, viewer.version, opened);
  wrap.appendChild(frame);

  main.appendChild(wrap);
  const { backdrop, drawer } = commentsPanel({ toggle, badge });
  main.append(backdrop, drawer);
}

// The old viewer name, kept so a caller that referenced it still resolves.
export { viewerRoute as showArtifact };
