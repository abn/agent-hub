// Sessions: the list per project, one session's detail, and the two actions
// that close or reclaim a session.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, glyph, paint, stale, when } from "./dom.mjs";
import { pickProject, withProject } from "./projects.mjs";
import { render } from "./router.mjs";
import { toast } from "./toast.mjs";

export async function sessionsScreen(selected, gen) {
  const empty = `<h1>Sessions</h1><p class="empty">No projects yet.</p>`;
  await withProject({ selected, gen, empty }, async (current, picker) => {
    paint(gen, `<h1>Sessions</h1>${picker}${await sessionsSection(current)}`);
  });
}

// The list the standalone screen and the segmented project view both paint.
export function sessionRow(s, current) {
  return `<div class="row">
    ${glyph("session")}
    <div class="grow">
      <div class="title">${esc(s.session_name)}</div>
      <div class="meta mono">${esc(s.agent)} · ${esc(s.status)} · ${when(s.last_activity)}</div>
    </div>
    <a class="button" href="#/session?project=${encodeURIComponent(current)}&id=${esc(s.id)}" aria-label="Open ${esc(s.session_name)}">Open</a>
    ${
      s.status === "ended"
        ? `<button type="button" class="danger" data-action="prune" data-id="${esc(s.id)}" data-agent="${esc(s.agent)}">Prune</button>`
        : `<button type="button" data-action="end" data-id="${esc(s.id)}">End</button>`
    }
  </div>`;
}

export async function sessionRows(current) {
  const { sessions } = await api(`/api/v1/sessions?project=${encodeURIComponent(current)}`);
  const rows = sessions.map((s) => sessionRow(s, current)).join("");
  return { sessions, card: `<div class="card">${rows || '<p class="empty">No sessions yet.</p>'}</div>` };
}

export async function sessionsSection(current) {
  return (await sessionRows(current)).card;
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
      ? `<button type="button" class="danger" data-action="prune" data-id="${esc(session.id)}" data-agent="${esc(session.agent)}">Prune</button>`
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
      (entry) => `<div class="row"><div class="grow"><div class="title mono">${esc(entry.path)}</div></div></div>`,
    )
    .join("");
  return `<h2>${label}</h2><div class="card">${rows || '<p class="empty">Empty.</p>'}</div>`;
}

export async function endSession(id) {
  await api(`/api/v1/sessions/${encodeURIComponent(id)}/end`, { method: "POST" });
  render();
}

// Prune deletes a session's brain and audit log, so it is asked first and
// stays reversible for the length of the toast. The listing carries no size
// for the session, so the dialog names what goes rather than what is freed;
// no number is shown that the hub has not reported.
export async function pruneSession(id, agent) {
  const confirmed = await confirmAction({
    title: "Prune 1 ended session?",
    body: "The brain file and audit log for this session are deleted. Events in the feed stay.",
    list: [agent ? `${id} · ${agent}` : id],
    note: "You can undo for 30 s after pruning.",
    safe: "Keep",
    danger: "Prune 1 session",
  });
  if (!confirmed) return;
  const token = await api(`/api/v1/storage/sessions/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  await render();
  toast("Pruned 1 session.", async () => {
    await api(`/api/v1/prune/undo/${encodeURIComponent(token.undo_token)}`, { method: "POST" });
    await render();
  });
}
