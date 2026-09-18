// The project feed: one project's events, day-grouped and filterable by kind.

import { api } from "./api.mjs";
import { eventRow, groupedEvents, main } from "./dom.mjs";
import { projectToolbar } from "./projects.mjs";
import { focusAfterRender, render } from "./router.mjs";

const KINDS = ["signal", "finished", "question", "answer", "approval", "artifact", "session"];
const KIND_LABELS = {
  signal: "Updates",
  finished: "Finished",
  question: "Questions",
  answer: "Answers",
  approval: "Approvals",
  artifact: "Artifacts",
  session: "Sessions",
};

// The kind filters are per project, so switching projects does not carry a
// filter across. They are view state, not a saved preference.
const projectFilters = new Map();

function kindChips(active) {
  const chips = KINDS.map((kind) => {
    const pressed = active.has(kind) ? "true" : "false";
    return `<button type="button" class="chip" data-action="kind" data-kind="${kind}" aria-pressed="${pressed}">${KIND_LABELS[kind]}</button>`;
  }).join("");
  return `<div class="toolbar" role="group" aria-label="Filter by kind">${chips}</div>`;
}

export async function projectsScreen(selected) {
  const { projects } = await api("/api/v1/projects");
  if (!projects.length) {
    main.innerHTML = `<h1>Projects</h1><p class="empty">No projects yet. Create one in Settings.</p>`;
    return;
  }
  const current = selected || projects[0].id;
  const active = new Set(projectFilters.get(current) || []);
  // The kind filter runs in the query, before the limit, so a chip finds the
  // newest events of its kind rather than only those inside a fetched window.
  const kinds = [...active].map((kind) => `&kinds=${encodeURIComponent(kind)}`).join("");
  const page = await api(`/api/v1/projects/${encodeURIComponent(current)}/feed?limit=100${kinds}`);
  const empty = active.size ? "No events match this filter." : "No events yet.";
  main.innerHTML = `
    <h1>Project feed</h1>
    ${projectToolbar(projects, current)}
    ${kindChips(active)}
    ${groupedEvents(page.events, eventRow) || `<p class="empty">${empty}</p>`}`;
}

// The kind chips are per project. The toggle keeps focus on the chip it
// pressed rather than dropping a keyboard user back at the top of the page.
export function toggleKind(kind, projectId) {
  if (!projectId) return;
  const active = new Set(projectFilters.get(projectId) || []);
  if (active.has(kind)) active.delete(kind);
  else active.add(kind);
  projectFilters.set(projectId, [...active]);
  focusAfterRender(kind);
  render();
}
