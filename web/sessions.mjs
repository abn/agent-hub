// Sessions: the list per project, one session's detail, and the two actions
// that close or reclaim a session.

import { api } from "./api.mjs";
import { unifiedBrainTree, wireTreeKeyboard } from "./brain-tree.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { pickProject, withProject } from "./projects.mjs";
import { render } from "./router.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export function truncateMiddle(id) {
  if (!id) return "";
  if (id.length <= 13) return id;
  return `${id.slice(0, 6)}…${id.slice(-5)}`;
}

export async function sessionsScreen(selected, gen) {
  const empty = `<h1>Sessions</h1><p class="empty">No projects yet.</p>`;
  await withProject({ selected, gen, empty }, async (current, picker) => {
    paint(gen, `<h1>Sessions</h1>${picker}${await sessionsSection(current)}`);
  });
}

// The list the standalone screen and the segmented project view both paint.
// Each row carries the state dot (active = ok fill, ended = ring, pruned =
// line-strong fill), the owner, the mono size and a chevron,
// matching the design's session row.
export function sessionRow(s, current, isSelected = false) {
  const isPruned = s.status === "pruned" || s.pruned;
  const dot =
    s.status === "active"
      ? `<span class="state-dot" style="background: var(--ok)"></span>`
      : isPruned
        ? `<span class="state-dot pruned"></span>`
        : `<span class="state-dot ring"></span>`;

  const size = s.brain_bytes != null ? fmtBytes(s.brain_bytes) : "";
  const owner = esc(s.owner ?? s.agent ?? "");
  const statusWord = esc(s.status || "");
  const metaText = `${owner} · ${statusWord} · ${relative(s.last_activity ?? s.created_at)}`;

  const href = `#/session?project=${encodeURIComponent(current)}&id=${encodeURIComponent(s.id)}`;

  return `<div class="row session-row${isSelected ? " selected" : ""}" data-id="${esc(s.id)}">
    ${dot}
    <a class="session-link stretched-link" href="${href}" aria-label="Open session ${esc(s.session_name)}, ${statusWord}">
      <span class="grow">
        <span class="title ${isPruned ? "pruned" : ""}">${esc(s.session_name)}</span>
        <span class="meta">${metaText}</span>
      </span>
      <span class="session-size mono">${size}</span>
      <svg class="session-chev" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 5l7 7-7 7"></path></svg>
    </a>
  </div>`;
}

// The selected project's session rows, as active and ended groups.
export async function sessionRows(current, selectedId = null) {
  const { sessions } = await api(`/api/v1/sessions?project=${encodeURIComponent(current)}`);
  const active = sessions.filter((s) => s.status === "active");
  const ended = sessions.filter((s) => s.status !== "active");
  const prunableBytes = ended
    .filter((s) => s.status === "ended" && !s.pruned && s.brain_bytes)
    .reduce((sum, s) => sum + (s.brain_bytes || 0), 0);

  let activeSection = "";
  if (active.length > 0) {
    const activeRows = active.map((s) => sessionRow(s, current, s.id === selectedId)).join("");
    activeSection = `
      <div class="session-group-header">ACTIVE · ${active.length}</div>
      <div class="session-group active-group">
        ${activeRows}
      </div>
    `;
  }

  let endedSection = "";
  if (ended.length > 0) {
    const endedRows = ended.map((s) => sessionRow(s, current, s.id === selectedId)).join("");
    const pruneAllLabel = prunableBytes > 0
      ? `Prune all · <span class="mono">${fmtBytes(prunableBytes)}</span>`
      : "Prune all";
    endedSection = `
      <div class="session-group-header ended-header">
        <span>ENDED · ${ended.length}</span>
        <button type="button" class="prune-all-btn" data-action="prune-all" data-project="${esc(current)}">${pruneAllLabel}</button>
      </div>
      <div class="session-group ended-group">
        ${endedRows}
      </div>
    `;
  }

  const footerNote = sessions.length > 0
    ? `<div class="sessions-footer-note">Pruning frees the brain of an ended session. Feed events and artifacts are never touched.</div>`
    : "";

  const content = (activeSection || endedSection)
    ? `<div class="sessions-list">${activeSection}${endedSection}${footerNote}</div>`
    : `<p class="empty">No sessions yet.</p>`;

  return { sessions, card: `<div class="sessions-card-wrap">${content}</div>` };
}

export async function sessionsSection(current) {
  return (await sessionRows(current)).card;
}

function lineageLine(lineage) {
  // lineage: {kind: "adopted" | "forked", session_id, agent, pruned}
  const verb = lineage.kind === "forked" ? "forked from" : "picked up from";
  const who = lineage.agent ? ` ${esc(lineage.agent)}` : "";
  const prunedNote = lineage.pruned ? " (pruned)" : "";
  return `<span class="meta">${verb}${who} · ${esc(lineage.session_id)}${prunedNote}</span>`;
}

export function sessionDetailHTML(session, current, kvEntries = [], fsEntries = []) {
  const isEnded = session.status === "ended" || session.status === "pruned";
  const statusClass = session.status === "active" ? "active" : "ended";
  const eventsCount = session.events ?? 0;
  const brainSize = fmtBytes(session.brain_bytes ?? 0);
  const totalItems = (kvEntries?.length || 0) + (fsEntries?.length || 0);

  const truncatedId = truncateMiddle(session.id);

  const file = typeof location !== "undefined" ? new URLSearchParams(location.hash.split("?")[1] || "").get("file") : null;
  const openFile = file
    ? `<div class="open-file row"><span class="mono">${esc(file)}</span><span class="meta">Content stays behind the agent surface; this names the entry opened.</span></div>`
    : "";

  const lineage = session.lineage
    ? `<div class="lineage">${lineageLine(session.lineage)}</div>`
    : "";
  const handoff = session.handoff
    ? `<div class="meta handoff">${esc(session.handoff)}</div>`
    : "";

  const lastEvent = session.last_event
    ? `<a class="audit-row" href="#/projects/${encodeURIComponent(current)}/feed">` +
        `<span class="meta mono">${esc(session.last_event.summary)}</span>` +
        `<time class="meta mono" datetime="${esc(session.last_event.at)}" title="${esc(session.last_event.at)}">${relative(session.last_event.at)}</time>` +
        `<svg class="session-chev" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 5l7 7-7 7"></path></svg>` +
      `</a>`
    : "";

  const primaryButton = isEnded
    ? `<button type="button" class="btn-primary danger" data-action="prune" data-id="${esc(session.id)}" data-agent="${esc(session.agent)}">Prune session</button>`
    : `<button type="button" class="btn-primary" data-action="end" data-id="${esc(session.id)}">End session</button>`;

  const helperNote = isEnded
    ? `Session ended ${relative(session.last_activity ?? session.created_at)}. Brain and logs can be pruned.`
    : "Pruning becomes available once the session has ended.";

  return `
    <div class="session-detail-view" data-session-id="${esc(session.id)}">
      <div class="session-back-bar">
        <a class="session-back-link" href="#/projects/${encodeURIComponent(current)}/sessions" aria-label="Back to sessions">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 18l-6-6 6-6"></path></svg>
          <span>Sessions</span>
        </a>
      </div>
      ${handoff}
      ${lineage}
      <div class="session-detail-title-block">
        <h1>${esc(session.session_name)}</h1>
        <div class="meta">${esc(session.owner ?? session.agent)} · <span class="session-status ${statusClass}">${esc(session.status)}</span> · started ${relative(session.created_at)} · ${eventsCount} events · <span class="mono">${brainSize}</span></div>
        <button type="button" class="session-copy-id" data-action="copy-id" data-id="${esc(session.id)}" title="${esc(session.id)}" aria-label="Copy full session id: ${esc(session.id)}">
          <span>${truncatedId}</span>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2"></rect><path d="M5 15V5h10"></path></svg>
        </button>
      </div>
      ${openFile}
      <div class="brain-header-line">
        <span class="brain-label mono">brain/ · ${brainSize} · ${totalItems} items</span>
        <span class="brain-size-col mono">size</span>
      </div>
      ${unifiedBrainTree(session.id, current, kvEntries, fsEntries, fetcher(session.id))}
      ${lastEvent}
      <div class="session-actions-footer">
        ${primaryButton}
        <p class="action-helper-sentence action-helper-note">${helperNote}</p>
      </div>
    </div>
  `;
}

export function wireSessionDetail(container, current, sessionId) {
  if (!container) return;
  for (const tree of container.querySelectorAll('[role="tree"]')) {
    const io = {
      fetchChildren: fetcher(sessionId),
      onError: (err) => toast(`Could not open the brain folder: ${err.message ?? err}`),
    };
    wireTreeKeyboard(tree, io);
    tree.addEventListener("openfile", (event) => {
      const path = event.detail?.node?.dataset?.path;
      if (path) location.hash = `#/projects/${encodeURIComponent(current)}/sessions?id=${encodeURIComponent(sessionId)}&file=${encodeURIComponent(path)}`;
    });
  }
}

export async function sessionDetailView(project, id, gen) {
  if (!id) return "";
  let detail = null;
  try {
    detail = await api(`/api/v1/sessions/${encodeURIComponent(id)}`);
  } catch (err) {
    if (err.status === 404) return "";
    throw err;
  }
  if (stale(gen) || !detail) return "";
  const query = encodeURIComponent(id);
  let kv = { entries: [] };
  let fs = { entries: [] };
  try {
    kv = (await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/kv")}`)) || kv;
  } catch {}
  try {
    fs = (await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/fs")}`)) || fs;
  } catch {}
  if (stale(gen)) return "";
  return sessionDetailHTML(detail, project, kv.entries, fs.entries);
}

export async function sessionDetail(project, id, gen) {
  const { current } = await pickProject(project);
  if (stale(gen)) return;
  if (!current || !id) {
    location.hash = "#/sessions";
    return;
  }
  const detailHTML = await sessionDetailView(current, id, gen);
  if (stale(gen)) return;
  if (!detailHTML) {
    location.hash = "#/sessions";
    return;
  }
  paint(gen, detailHTML);
  wireSessionDetail(main, current, id);
}

// A fetcher the tree uses to lazy-load a folder's children on first expand.
const fetcher = (sessionId) => async (path) => {
  const data = await api(
    `/api/v1/sessions/${encodeURIComponent(sessionId)}/brain?path=${encodeURIComponent(path)}`,
  );
  return { entries: data.entries || [] };
};

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

export async function pruneAllEnded(project) {
  const path = project
    ? `/api/v1/storage/sessions?project=${encodeURIComponent(project)}`
    : "/api/v1/storage/sessions";
  const confirmed = await confirmAction({
    title: "Prune ended sessions?",
    body: "The brain files and audit logs for ended sessions are deleted. Events in the feed stay.",
    note: "You can undo for 30 s after pruning.",
    safe: "Keep",
    danger: "Prune ended sessions",
  });
  if (!confirmed) return;
  let res;
  try {
    res = await api(path, { method: "DELETE" });
  } catch (err) {
    toast(`Nothing changed: ${err.message}`);
    return;
  }
  await render();
  const tokens = (res.sessions || []).map((s) => s.undo_token);
  if (!tokens.length) {
    toast("Nothing was pruned: no session had ended.");
    return;
  }
  toast(`Pruned ${tokens.length} ended session${tokens.length === 1 ? "" : "s"}.`, async () => {
    try {
      for (const t of tokens) {
        await api(`/api/v1/prune/undo/${encodeURIComponent(t)}`, { method: "POST" });
      }
    } catch (err) {
      toast(`Undo stopped: ${err.message}`);
    }
    await render();
  });
}

// Prune deletes a session's brain and audit log, so it is asked first and
// stays reversible for the length of the toast.
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
  const currentHash = location.hash;
  const params = new URLSearchParams(currentHash.split("?")[1] || "");
  const currentProject = params.get("project");
  if (currentHash.startsWith("#/session")) {
    const target = currentProject ? `#/projects/${encodeURIComponent(currentProject)}/sessions` : "#/sessions";
    history.replaceState(null, "", target);
  }
  await render();
  toast("Pruned 1 session.", async () => {
    await api(`/api/v1/prune/undo/${encodeURIComponent(token.undo_token)}`, { method: "POST" });
    if (currentHash.startsWith("#/session")) {
      location.hash = currentHash;
    } else {
      await render();
    }
  });
}

if (typeof document !== "undefined") {
  document.addEventListener("click", async (event) => {
    const copyBtn = event.target.closest?.('[data-action="copy-id"], .session-copy-id');
    if (copyBtn) {
      const id = copyBtn.dataset.id || copyBtn.getAttribute("title");
      if (id) {
        try {
          await navigator.clipboard.writeText(id);
        } catch {}
        toast("Copied session id.");
      }
      return;
    }

    const pruneAllBtn = event.target.closest?.('[data-action="prune-all"]');
    if (pruneAllBtn) {
      const project = pruneAllBtn.dataset.project;
      await pruneAllEnded(project);
      return;
    }
  });
}
