// The project picker every per-project screen carries, and the preamble that
// puts one in front of a screen.

import { api } from "./api.mjs";
import { esc, paint } from "./dom.mjs";

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
