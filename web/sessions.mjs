// Sessions: the list per project, one session's detail, and the two actions
// that close or reclaim a session.

import { api } from "./api.mjs";
import { brainTree, wireTreeKeyboard } from "./brain-tree.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { pickProject, withProject } from "./projects.mjs";
import { render } from "./router.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export async function sessionsScreen(selected, gen) {
  const empty = `<h1>Sessions</h1><p class="empty">No projects yet.</p>`;
  await withProject({ selected, gen, empty }, async (current, picker) => {
    paint(gen, `<h1>Sessions</h1>${picker}${await sessionsSection(current)}`);
  });
}

// The list the standalone screen and the segmented project view both paint.
// Each row carries the state dot (active = ok fill, ended = ring, pruned =
// line-strong fill), the owner, the id as mono, the mono size and a chevron,
// matching the design's session row.
export function sessionRow(s, current) {
  const dot =
    s.status === "active"
      ? `<span class="state-dot" style="background: var(--ok)"></span>`
      : s.status === "ended"
        ? `<span class="state-dot ring"></span>`
        : `<span class="state-dot pruned"></span>`;
  const size = s.brain_bytes != null ? fmtBytes(s.brain_bytes) : "";
  const idPrefix = s.id && s.id.length > 8 ? `${s.id.slice(0, 8)}…` : s.id || "";
  return `<div class="row session-row">
    ${dot}
    <a class="session-link" href="#/session?project=${encodeURIComponent(current)}&id=${esc(s.id)}" aria-label="Open session ${esc(s.session_name)}">
      <span class="grow">
        <span class="title">${esc(s.session_name)} ${idPrefix ? `<span class="mono id">${esc(idPrefix)}</span>` : ""}</span>
        <span class="meta mono">${esc(s.owner ?? s.agent)} · ${esc(s.status)} · ${relative(s.last_activity)}</span>
      </span>
      <span class="session-size mono">${size}</span>
      <span class="session-chev" aria-hidden="true"></span>
    </a>
    ${
      s.status === "ended"
        ? `<button type="button" class="danger" data-action="prune" data-id="${esc(s.id)}" data-agent="${esc(s.agent)}">Prune</button>`
        : `<button type="button" data-action="end" data-id="${esc(s.id)}">End</button>`
    }
  </div>`;
}

// The selected project's session rows, as a card.
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
  // Enter on a brain leaf keeps the opened file in the hash, so reload and
  // history restore the same file view.
  const file = new URLSearchParams(location.hash.split("?")[1] || "").get("file");
  const openFile =
    file
      ? `<div class="open-file row"><span class="mono">${esc(file)}</span>` +
        `<span class="meta">Content stays behind the agent surface; this names the entry opened.</span></div>`
      : "";

  // The detail route returns events, last_event, lineage and brain_bytes in
  // one call; the brain listing is fetched separately, lazily by the tree.
  const detail = await api(`/api/v1/sessions/${encodeURIComponent(id)}`);
  if (stale(gen)) return;
  if (!detail) {
    location.hash = "#/sessions";
    return;
  }
  const session = detail;
  const query = encodeURIComponent(id);
  // Initial tree listing for the header and the two root groups.
  const kv = await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/kv")}`);
  const fs = await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/fs")}`);
  if (stale(gen)) return;

  const action =
    session.status === "ended"
      ? `<button type="button" class="danger" data-action="prune" data-id="${esc(session.id)}" data-agent="${esc(session.agent)}">Prune</button>`
      : `<button type="button" data-action="end" data-id="${esc(session.id)}">End session</button>`;

  const stat = (label, value) =>
    `<div class="stat-card"><div class="stat-label">${esc(label)}</div><div class="stat-value">${value}</div></div>`;

  const lineage = session.lineage
    ? `<div class="lineage">${lineageLine(session.lineage)}</div>`
    : "";
  const handoff = session.handoff
    ? `<div class="meta handoff">${esc(session.handoff)}</div>`
    : "";

  const lastEvent = session.last_event
    ? `<a class="audit-row" href="#/feed?project=${encodeURIComponent(current)}">` +
        `<span class="meta mono">${esc(session.last_event.summary)}</span>` +
        `<time class="meta mono" datetime="${esc(session.last_event.at)}" title="${esc(session.last_event.at)}">${relative(session.last_event.at)}</time>` +
        `<svg class="session-chev" aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M 9 6l6 6-6 6"></path></svg></a>`
    : "";

  paint(
    gen,
    `
    <p class="meta"><a href="#/sessions?project=${encodeURIComponent(current)}">Back to sessions</a></p>
    ${handoff}
    ${lineage}
    <h1>${esc(session.session_name)}</h1>
    <p class="meta mono">${esc(session.owner ?? session.agent)} · ${esc(session.status)} · ${esc(session.id)} · ${fmtBytes(session.brain_bytes ?? 0)}</p>
    ${openFile}
    <div class="card stat-row">
      ${stat("Started", relative(session.created_at))}
      ${stat("Events", session.events ?? 0)}
      ${stat("Brain", fmtBytes(session.brain_bytes ?? 0))}
    </div>
    ${lastEvent}
    ${brainTree("brain/ · Keys", id, current, "kv", kv.entries, fetcher(id))}
    ${brainTree("brain/ · Files", id, current, "fs", fs.entries, fetcher(id))}
    ${actionBar(session)}
    `,
  );
  // The trees are in the DOM now: give each its keyboard and its lazy fetcher.
  // Enter on a leaf records the opened file in the hash so the reload and the
  // history keep it, and the open-file line below names it.
  for (const tree of main.querySelectorAll('[role="tree"]')) {
    const io = {
      fetchChildren: fetcher(id),
      onError: (err) => toast(`Could not open the brain folder: ${err.message ?? err}`),
    };
    wireTreeKeyboard(tree, io);
    tree.addEventListener("openfile", (event) => {
      const path = event.detail?.node?.dataset?.path;
      if (path) location.hash = `#/session?project=${encodeURIComponent(current)}&id=${encodeURIComponent(id)}&file=${encodeURIComponent(path)}`;
    });
  }
}

// A fetcher the tree uses to lazy-load a folder's children on first expand.
// The router wires the tree's keyboard to expandNode / collapseNode once the
// module has built its trees (see the session view wiring below).
const fetcher = (sessionId) => async (path) => {
  const data = await api(
    `/api/v1/sessions/${encodeURIComponent(sessionId)}/brain?path=${encodeURIComponent(path)}`,
  );
  return { entries: data.entries || [] };
};

function actionBar(session) {
  const pruneDisabled = session.status !== "ended" ? " disabled" : "";
  return `
    <div class="session-actions">
      <button type="button" data-action="end" data-id="${esc(session.id)}">End session</button>
      <button type="button" data-action="prune" data-id="${esc(session.id)}" data-agent="${esc(session.agent)}" ${pruneDisabled}>Prune (ends first)</button>
    </div>`;
}

function lineageLine(lineage) {
  // lineage: {kind: "adopted" | "forked", session_id, agent, pruned}
  const verb = lineage.kind === "forked" ? "forked from" : "picked up from";
  const who = lineage.agent ? ` ${esc(lineage.agent)}` : "";
  const prunedNote = lineage.pruned ? " (pruned)" : "";
  return `<span class="meta">${verb}${who} · ${esc(lineage.session_id)}${prunedNote}</span>`;
}

function fmtBytes(bytes) {
  if (bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
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
