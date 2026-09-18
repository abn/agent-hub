// The project feed: one project's events, day-grouped and filterable by kind.
// The segmented project view and the standalone screen both paint the same
// body, so the filter state and the row rendering live in one place.

import { api } from "./api.mjs";
import { eventRow, groupedEvents, paint } from "./dom.mjs";
import { emptyStateHTML, EMPTY_COPY } from "./empty.mjs";
import { withProject } from "./projects.mjs";
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

export function activeKinds(projectId) {
  return new Set(projectFilters.get(projectId) || []);
}

export function kindChips(active) {
  const chips = KINDS.map((kind) => {
    const pressed = active.has(kind) ? "true" : "false";
    return `<button type="button" class="chip" data-action="kind" data-kind="${kind}" aria-pressed="${pressed}">${KIND_LABELS[kind]}</button>`;
  }).join("");
  return `<div class="toolbar" role="group" aria-label="Filter by kind">${chips}</div>`;
}

// The chip row and the day-grouped events, or the empty state. The feed's
// empty state names the project, so the id (the slug) is what fills its copy.
export async function feedSection(current) {
  const active = activeKinds(current);
  // The kind filter runs in the query, before the limit, so a chip finds
  // the newest events of its kind rather than only those inside a fetched
  // window.
  const kinds = [...active].map((kind) => `&kinds=${encodeURIComponent(kind)}`).join("");
  const page = await api(`/api/v1/projects/${encodeURIComponent(current)}/feed?limit=100${kinds}`);
  const events = groupedEvents(page.events, eventRow);
  if (!events) {
    const copy = active.size
      ? { ...EMPTY_COPY.feed, title: "No events match this filter.", body: "Clear a filter to see everything in this project." }
      : EMPTY_COPY.feed;
    return `${kindChips(active)}${emptyStateHTML(copy, { project: current })}`;
  }
  return `${kindChips(active)}${events}`;
}

export async function projectsScreen(selected, gen) {
  const empty = `<h1>Projects</h1><p class="empty">No projects yet. Create one in Settings.</p>`;
  await withProject({ selected, gen, empty }, async (current) => {
    paint(gen, `<h1>Project feed</h1>${await feedSection(current)}`);
  });
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
