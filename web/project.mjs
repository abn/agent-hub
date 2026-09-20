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
import { count } from "./home.mjs";
import { registerScreen } from "./keys.mjs";
import { pickProject } from "./projects.mjs";
import { settingsLink } from "./project-settings.mjs";
import { sessionRows } from "./sessions.mjs";

const SEGMENTS = ["feed", "artifacts", "sessions"];

// The id and the segment a `#/projects/<id>/<segment>` hash names. The segment
// defaults to the feed, so `#/projects/<id>` is the project's feed.
export function segmentOf(hash) {
  const parts = (hash.replace(/^#/, "").split("?")[0] || "").split("/");
  const segment = SEGMENTS.includes(parts[3]) ? parts[3] : "feed";
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

// The sessions segment on desktop: the list in the 420px pane and the session
// detail in the other. The detail pane's internals belong to the next surface
// to fill it; for now it holds one session's real summary, so the container
// exists and the layout is the intended two-pane.
function sessionsTwoPane(current, listHTML, sessions) {
  const s = sessions[0];
  const detail = s
    ? `<div class="card">
        <div class="item-title">${esc(s.session_name)}</div>
        <p class="meta mono">${esc(s.agent)} · ${esc(s.status)} · ${esc(s.id)}</p>
      </div>`
    : `<p class="empty">No sessions yet.</p>`;
  return twoPane(`<h2 class="section-label">Sessions</h2>${listHTML}`, detail);
}

export async function projectScreen(params, gen, path) {
  const parts = (path || "").split("/");
  const segment = SEGMENTS.includes(parts[3]) ? parts[3] : "feed";
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
  if (!id || !projects.some((p) => p.id === id)) {
    // The address named no project, or one that is gone. Land on the first
    // project, keeping the segment the reader asked for.
    location.hash = `#/projects/${encodeURIComponent(current)}/${segment}`;
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
    const { sessions, card } = await sessionRows(id);
    if (stale(gen)) return;
    paint(gen, `${shell}${sessionsTwoPane(id, card, sessions)}`);
  } else {
    paint(gen, `${shell}${await feedSection(id, stats)}`);
  }
}

// The row selection the keyboard map owns: the project's segments all paint
// `.row`s, so a reader walking the feed, the gallery or the list gets the map.
registerScreen("projects", { rows: ".row" });
