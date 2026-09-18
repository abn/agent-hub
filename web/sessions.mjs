// Sessions: the list per project, one session's detail, and the two actions
// that close or reclaim a session.

import { api } from "./api.mjs";
import { esc, glyph, paint, stale, when } from "./dom.mjs";
import { pickProject, withProject } from "./projects.mjs";
import { render } from "./router.mjs";
import { toast } from "./toast.mjs";

export async function sessionsScreen(selected, gen) {
  const empty = `<h1>Sessions</h1><p class="empty">No projects yet.</p>`;
  await withProject({ selected, gen, empty }, async (current, picker) => {
    const { sessions } = await api(`/api/v1/sessions?project=${encodeURIComponent(current)}`);
    const rows = sessions
      .map(
        (s) => `<div class="row">
        ${glyph("session")}
        <div class="grow">
          <div class="title">${esc(s.session_name)}</div>
          <div class="meta mono">${esc(s.agent)} · ${esc(s.status)} · ${when(s.last_activity)}</div>
        </div>
        <a class="button" href="#/session?project=${encodeURIComponent(current)}&id=${esc(s.id)}" aria-label="Open ${esc(s.session_name)}">Open</a>
        ${
          s.status === "ended"
            ? `<button type="button" class="danger" data-action="prune" data-id="${esc(s.id)}">Prune</button>`
            : `<button type="button" data-action="end" data-id="${esc(s.id)}">End</button>`
        }
      </div>`,
      )
      .join("");
    paint(
      gen,
      `
    <h1>Sessions</h1>
    ${picker}
    <div class="card">${rows || '<p class="empty">No sessions yet.</p>'}</div>`,
    );
  });
}

export async function sessionDetail(project, id, gen) {
  const { current } = await pickProject(project);
  if (stale(gen)) return;
  if (!current || !id) {
    location.hash = "#/sessions";
    return;
  }
  const { sessions } = await api(`/api/v1/sessions?project=${encodeURIComponent(current)}`);
  const session = sessions.find((item) => item.id === id);
  if (stale(gen)) return;
  if (!session) {
    location.hash = "#/sessions";
    return;
  }
  const query = encodeURIComponent(id);
  const kv = await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/kv")}`);
  const fs = await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/fs")}`);
  const action =
    session.status === "ended"
      ? `<button type="button" class="danger" data-action="prune" data-id="${esc(session.id)}">Prune</button>`
      : `<button type="button" data-action="end" data-id="${esc(session.id)}">End</button>`;
  paint(
    gen,
    `
    <p class="meta"><a href="#/sessions?project=${encodeURIComponent(current)}">Back to sessions</a></p>
    <h1>${esc(session.session_name)}</h1>
    <div class="card">
      <div class="row"><div class="grow"><div class="meta">Project</div><div class="title mono">${esc(session.project_id)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Agent</div><div class="title mono">${esc(session.agent)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Status</div><div class="title">${esc(session.status)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Started</div><div class="title mono">${when(session.created_at)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Last activity</div><div class="title mono">${when(session.last_activity)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Brain file</div><div class="title mono">${esc(session.brain_path)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Session id</div><div class="title mono">${esc(session.id)}</div></div>${action}</div>
    </div>
    ${brainTree("Keys", kv.entries)}
    ${brainTree("Files", fs.entries)}`,
  );
}

function brainTree(label, entries) {
  const rows = entries
    .map(
      (entry) => `<div class="row"><div class="grow"><div class="title mono">${esc(entry)}</div></div></div>`,
    )
    .join("");
  return `<h2>${label}</h2><div class="card">${rows || '<p class="empty">Empty.</p>'}</div>`;
}

export async function endSession(id) {
  await api(`/api/v1/sessions/${encodeURIComponent(id)}/end`, { method: "POST" });
  render();
}

export async function pruneSession(id) {
  const token = await api(`/api/v1/storage/sessions/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  render();
  toast("Session pruned.", async () => {
    await api(`/api/v1/prune/undo/${encodeURIComponent(token.undo_token)}`, { method: "POST" });
    render();
  });
}
