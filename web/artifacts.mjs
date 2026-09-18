// Artifacts: the per-project gallery, and the viewer that embeds an artifact's
// own public page next to the comments drawer.

import { api } from "./api.mjs";
import { commentsPanel, commentsToggle, startComments } from "./comments.mjs";
import { beginRender, esc, glyph, main, paint } from "./dom.mjs";
import { withProject } from "./projects.mjs";

export async function artifactsScreen(selected, gen) {
  const empty = `<h1>Artifacts</h1><p class="empty">No projects yet. Create one in Settings.</p>`;
  await withProject({ selected, gen, empty }, async (current, picker) => {
    const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(current)}/artifacts`);
    const rows = artifacts
      .map(
        (a) => `<div class="row">
        ${glyph("artifact")}
        <div class="grow">
          <div class="title">${esc(a.title)}</div>
          <div class="meta mono">v${a.version} · ${a.protected ? "protected" : "public"} · ${a.size_bytes} bytes</div>
        </div>
        <button type="button" data-action="artifact-open" data-id="${esc(a.id)}" aria-label="Open ${esc(a.title)}">Open</button>
      </div>`,
      )
      .join("");
    paint(
      gen,
      `
    <h1>Artifacts</h1>
    ${picker}
    <div class="card">${rows || '<p class="empty">This project has no artifacts yet. An agent publishing one will show it here.</p>'}</div>`,
    );
  });
}

export function openArtifact(id) {
  showArtifact(id);
}

function showArtifact(id) {
  // The viewer replaces whatever screen is open, so it claims the render
  // number too and a screen still fetching cannot paint over it.
  beginRender();
  startComments(id);
  main.innerHTML = "";
  const top = document.createElement("div");
  top.className = "comments-toolbar";
  const back = document.createElement("p");
  back.style.margin = "0";
  const link = document.createElement("a");
  link.href = "#/artifacts";
  link.textContent = "Back to artifacts";
  back.appendChild(link);
  const { toggle, badge } = commentsToggle();
  top.append(back, toggle);
  const note = document.createElement("p");
  note.className = "meta";
  note.textContent =
    "The artifact opens on its public page. A protected artifact unlocks there; the server never holds its plaintext.";
  main.append(top, note);

  // The host page owns the title, picker, theme toggle, and unlock form, so
  // nothing renders twice. Scripts run, the origin stays opaque, and the
  // viewer module treats localStorage as unavailable.
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("title", "Artifact");
  frame.style.width = "100%";
  frame.style.height = "60vh";
  frame.style.border = "1px solid var(--line)";
  frame.src = `/artifacts/${encodeURIComponent(id)}`;
  main.appendChild(frame);

  const { backdrop, drawer } = commentsPanel({ toggle, badge });
  main.append(backdrop, drawer);
}
