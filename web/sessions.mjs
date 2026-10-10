// Sessions: the list per project, one session's detail, and the two actions
// that close or reclaim a session.

import { api } from "./api.mjs";
import { focusTreePath, unifiedBrainTree, wireTreeKeyboard } from "./brain-tree.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { pickProject, withProject } from "./projects.mjs";
import { render, restoreFocusAfterRender } from "./router.mjs";
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
      ? `<span class="state-dot live"></span>`
      : isPruned
        ? `<span class="state-dot pruned"></span>`
        : `<span class="state-dot ring"></span>`;

  const size = s.brain_bytes != null ? fmtBytes(s.brain_bytes) : "";
  const owner = esc(s.owner ?? s.agent ?? "");
  const statusWord = esc(s.status || "");
  const truncatedId = truncateMiddle(s.id);
  const metaText = `${owner} · ${statusWord} · ${relative(s.last_activity ?? s.created_at)}`;

  const href = current
    ? `#/projects/${encodeURIComponent(current)}/sessions?id=${encodeURIComponent(s.id)}`
    // With no project to name, the compatibility entry resolves one and hands
    // the reader the same screen inside it.
    : `#/session?id=${encodeURIComponent(s.id)}`;

  return `<div class="row session-row${isSelected ? " selected" : ""}" data-id="${esc(s.id)}">
    ${dot}
    <a class="session-link stretched-link" href="${href}" aria-label="Open session ${esc(s.session_name)}, ${statusWord}">
      <span class="grow">
        <span class="title ${isPruned ? "pruned" : ""}"><span class="session-id mono">${truncatedId}</span> <span class="sep">·</span> ${esc(s.session_name)}</span>
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
      <div class="session-group-header active-header">
        <span>ACTIVE · ${active.length}</span>
        <span class="group-header-divider" aria-hidden="true"></span>
      </div>
      <div class="session-group active-group">
        ${activeRows}
      </div>
    `;
  }

  let endedSection = "";
  if (ended.length > 0) {
    const endedRows = ended.map((s) => sessionRow(s, current, s.id === selectedId)).join("");
    const sizePart = fmtBytes(prunableBytes);
    const pruneAllLabel = `Prune all · <span class="mono">${sizePart}</span>`;
    endedSection = `
      <div class="session-group-header ended-header">
        <span>ENDED · ${ended.length}</span>
        <span class="group-header-divider" aria-hidden="true"></span>
        <button type="button" class="prune-all-btn" data-action="prune-all" data-project="${esc(current)}">${pruneAllLabel}</button>
      </div>
      <div class="session-group ended-group">
        ${endedRows}
      </div>
    `;
  }

  const totalSessions = sessions.length;
  const totalBytes = sessions.reduce((sum, s) => sum + (s.brain_bytes || 0), 0);
  const indexHead = sessions.length > 0
    ? `<div class="session-index-head">
        <div class="session-index-head-row">
          <span class="session-index-head-title">Sessions</span>
          <span class="session-index-head-stats mono">${totalSessions} · ${fmtBytes(totalBytes)}</span>
        </div>
        <div class="session-index-head-project mono">${esc(current)}</div>
      </div>`
    : "";

  const footerNote = sessions.length > 0
    ? `<div class="sessions-footer-note">Pruning frees the brain of an ended session. Feed events and artifacts are never touched.</div>`
    : "";

  const content = (activeSection || endedSection)
    ? `<div class="sessions-list">${indexHead}${activeSection}${endedSection}${footerNote}</div>`
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

  const lineage = session.lineage
    ? `<div class="lineage">${lineageLine(session.lineage)}</div>`
    : "";
  const handoff = session.handoff
    ? `<div class="session-handoff-note meta handoff">${esc(session.handoff)}</div>`
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
    : `<button type="button" class="btn-outline end-session" data-action="end" data-id="${esc(session.id)}">End session…</button>`;

  const helperNote = isEnded
    ? `Session ended ${relative(session.last_activity ?? session.created_at)}. Brain and logs can be pruned.`
    : "Pruning becomes available once the session has ended.";

  const brainFilesCount = fsEntries ? fsEntries.length : 0;
  const brainFootnote = `${brainFilesCount} files · ${brainSize} · written by ${esc(session.owner ?? session.agent ?? "agent")}`;

  return `
    <div class="session-detail-view" data-session-id="${esc(session.id)}">
      <style>
        .session-tree-pane { width: 100% !important; flex: 1 !important; border-right: none !important; }
        .session-file-viewer { display: none !important; }
        .shell .session-back-bar { display: none !important; }
      </style>
      <div class="session-back-bar">
        <a class="session-back-link" href="#/projects/${encodeURIComponent(current)}/sessions" aria-label="Back to sessions">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 18l-6-6 6-6"></path></svg>
          <span>Sessions</span>
        </a>
      </div>

      <header class="session-detail-header">
        <div class="session-header-main">
          <div class="session-detail-title-block">
            <div class="session-title-line">
              <span class="session-status ${statusClass}"><span class="state-dot${session.status === "active" ? " live" : ""}"></span>${esc(session.status)}</span>
            </div>
            <div class="meta" data-owner="${esc(session.owner ?? session.agent)}">${esc(session.owner ?? session.agent)} · <span class="mono">${truncatedId}</span> <button type="button" class="row-copy-glyph" data-action="copy-id" data-id="${esc(session.id)}" title="${esc(session.id)}" aria-label="Copy full session id: ${esc(session.id)}">${glyphSvg("copy", { size: 14 })}</button> · started ${relative(session.created_at)} · ${eventsCount} events · <span class="mono">${brainSize}</span></div>
            ${handoff}
            ${lineage}
          </div>
        </div>
        <div class="session-header-side">
          <span class="session-reassign">
            <button type="button" class="btn-outline session-reassign-btn" data-action="reassign-open" data-id="${esc(session.id)}" data-owner="${esc(session.owner ?? session.agent)}" aria-haspopup="menu" aria-expanded="false">Reassign</button>
            <div class="shell-group-menu session-reassign-menu" role="menu" aria-label="Move this session to an agent" hidden></div>
          </span>
        </div>
      </header>

      <div class="session-stage-body">
        <div class="session-tree-pane" role="region" aria-label="Brain tree">
          <div class="brain-header-line">
            <span class="brain-label">Brain <span class="mono">· ${brainSize} · ${totalItems} items</span></span>
            <span class="brain-size-col mono">size</span>
          </div>
          ${unifiedBrainTree(session.id, current, kvEntries, fsEntries, fetcher(session.id))}
          <div class="tree-pane-footnote mono">${brainFootnote}</div>
          ${lastEvent}
          <div class="session-actions-footer">
            ${primaryButton}
            <p class="action-helper-sentence action-helper-note">${helperNote}</p>
          </div>
        </div>
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
      if (!path) return;
      const query = new URLSearchParams({ id: sessionId, file: path });
      const next = `#/projects/${encodeURIComponent(current)}/sessions?${query}`;
      if (next === location.hash) return;
      // The paint that follows replaces the tree, so the item the reader
      // activated goes with it and the browser drops focus to the body. Their
      // place is the path they activated, which is what the new tree renders:
      // focus goes back to it, or to the nearest ancestor that survived. The
      // router's route focus then stays off, because this is the same screen
      // with another selection rather than a new place.
      restoreFocusAfterRender(() => focusTreePath(main.querySelector('[role="tree"]'), path));
      location.hash = next;
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

// The compatibility entry for `#/session?...`, the address older links carry.
// A session is read inside its project, where the index and the section
// switcher are, so this resolves the project the session belongs to and hands
// over to that project's sessions segment rather than painting a bare detail.
// It asks for nothing but the project list: the session itself is fetched by
// the screen it forwards to, so a link naming a session this hub no longer
// holds still lands on a working list instead of an empty screen.
export async function sessionInProject(params, gen) {
  const id = params.get("id");
  if (!id) {
    location.hash = "#/sessions";
    return;
  }
  const named = params.get("project");
  const { projects } = await pickProject(named);
  if (stale(gen)) return;
  // A saved link may name a project this hub no longer has. The session is
  // read in a project, so it lands in the one the hub does have.
  const project = (projects.find((p) => p.id === named) || projects[0])?.id || "";
  if (!project) {
    location.hash = "#/sessions";
    return;
  }
  const query = new URLSearchParams({ id });
  const file = params.get("file");
  if (file) query.set("file", file);
  location.hash = `#/projects/${encodeURIComponent(project)}/sessions?${query}`;
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
  // Ending stops something live and cannot be undone, so it asks first, in the
  // same dialog pattern prune and revoke use.
  const confirmed = await confirmAction({
    title: "End this session?",
    body: "The agent's session stops now. Ending cannot be undone.",
    note: "Once ended, its brain can be pruned to reclaim the space.",
    safe: "Keep",
    danger: "End session",
    tone: "action",
    commit: async () => {
      await api(`/api/v1/sessions/${encodeURIComponent(id)}/end`, { method: "POST" });
      await render();
    },
  });
  if (confirmed) toast("Session ended.");
}

// The human moves a session to another agent when the one holding it is not
// coming back. The move is named and confirmed, because a session's brain
// belongs to its owner and only the human ever makes this call.
export async function reassignSession(id, agent, from) {
  const confirmed = await confirmAction({
    title: "Move this session?",
    body: `${id} moves from ${from} to ${agent}. The agent can pick the work up again.`,
    note: "The brain stays with the session. Feed events are never touched.",
    safe: "Keep",
    danger: "Reassign",
    tone: "action",
    commit: async () => {
      await api(`/api/v1/sessions/${encodeURIComponent(id)}/reassign`, {
        method: "POST",
        body: JSON.stringify({ agent }),
      });
      await render();
    },
  });
  if (confirmed) toast(`Session reassigned to ${agent}.`);
}

// The menu of agents a session can move to, opened from the header control.
async function openReassignMenu(button) {
  const wrap = button.closest(".session-reassign");
  const menu = wrap?.querySelector(".session-reassign-menu");
  if (!menu) return;
  if (!menu.hidden) {
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    return;
  }
  const id = button.dataset.id;
  const from = button.dataset.owner;
  let agents = [];
  try {
    agents = (await api("/api/v1/agents")).agents || [];
  } catch (err) {
    toast(`Could not list agents: ${err.message ?? err}`);
    return;
  }
  const targets = agents.filter((a) => a.id !== from);
  if (!targets.length) {
    toast("No other agent to move this session to.");
    return;
  }
  menu.innerHTML = targets
    .map(
      (a) =>
        `<button type="button" role="menuitem" data-action="reassign" data-id="${esc(id)}" data-agent="${esc(a.id)}" data-from="${esc(from)}">${esc(a.display_name || a.id)}</button>`,
    )
    .join("");
  const rect = button.getBoundingClientRect();
  menu.style.top = `${Math.round(rect.bottom + 6)}px`;
  menu.style.left = `${Math.round(Math.max(8, Math.min(rect.left, window.innerWidth - 200)))}px`;
  menu.hidden = false;
  button.setAttribute("aria-expanded", "true");
  menu.querySelector("button")?.focus();
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
  await render();
  toast("Pruned 1 session.", async () => {
    await api(`/api/v1/prune/undo/${encodeURIComponent(token.undo_token)}`, { method: "POST" });
    await render();
  });
}

let markedPromise = null;
export function getMarked() {
  if (typeof window === "undefined") return Promise.resolve(null);
  if (window.marked && typeof window.marked.parse === "function") {
    return Promise.resolve(window.marked);
  }
  if (!markedPromise) {
    markedPromise = new Promise((resolve) => {
      const script = document.createElement("script");
      script.src = new URL("vendor/marked.js", document.baseURI).href;
      script.onload = () => resolve(window.marked);
      script.onerror = () => resolve(null);
      document.head.appendChild(script);
    });
  }
  return markedPromise;
}

export function fallbackMarkdown(text) {
  const lines = (text || "").split("\n");
  const out = [];
  let inList = false;
  for (const line of lines) {
    if (line.startsWith("# ")) {
      if (inList) { out.push("</ul>"); inList = false; }
      out.push(`<h1>${esc(line.slice(2))}</h1>`);
    } else if (line.startsWith("## ")) {
      if (inList) { out.push("</ul>"); inList = false; }
      out.push(`<h2>${esc(line.slice(3))}</h2>`);
    } else if (line.startsWith("### ")) {
      if (inList) { out.push("</ul>"); inList = false; }
      out.push(`<h3>${esc(line.slice(4))}</h3>`);
    } else if (line.startsWith("- ") || line.startsWith("* ")) {
      if (!inList) { out.push("<ul>"); inList = true; }
      out.push(`<li>${esc(line.slice(2))}</li>`);
    } else if (!line.trim()) {
      if (inList) { out.push("</ul>"); inList = false; }
    } else {
      if (inList) { out.push("</ul>"); inList = false; }
      out.push(`<p>${esc(line)}</p>`);
    }
  }
  if (inList) out.push("</ul>");
  return out.join("");
}

const ALLOWED_TAGS = new Set([
  "h1", "h2", "h3", "h4", "h5", "h6",
  "p", "blockquote", "pre", "code", "hr", "br",
  "ul", "ol", "li",
  "strong", "b", "em", "i", "s", "del", "strike", "sub", "sup", "mark", "span",
  "a", "img",
  "table", "thead", "tbody", "tr", "th", "td",
]);

const DANGEROUS_TAGS = new Set([
  "script", "style", "iframe", "frame", "object", "embed", "applet",
  "form", "input", "button", "textarea", "select", "option",
  "svg", "math", "base", "meta", "link", "template", "noscript",
]);

const ALLOWED_ATTRS = {
  a: new Set(["href", "title", "target", "rel"]),
  img: new Set(["src", "alt", "title", "width", "height"]),
  th: new Set(["align"]),
  td: new Set(["align"]),
  code: new Set(["class"]),
  pre: new Set(["class"]),
  span: new Set(["class"]),
};

const SAFE_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);

function isSafeUrl(urlStr) {
  if (!urlStr) return false;
  const cleaned = urlStr.trim().replace(/[\x00-\x20]+/g, "");
  if (
    cleaned.startsWith("#") ||
    cleaned.startsWith("/") ||
    cleaned.startsWith("./") ||
    cleaned.startsWith("../") ||
    cleaned.startsWith("?")
  ) {
    return true;
  }
  const colonIdx = cleaned.indexOf(":");
  if (colonIdx === -1) {
    return true;
  }
  const scheme = cleaned.slice(0, colonIdx + 1).toLowerCase();
  return SAFE_PROTOCOLS.has(scheme);
}

function sanitizeNode(parent) {
  const children = Array.from(parent.childNodes);
  for (const node of children) {
    if (node.nodeType === 1) {
      const tag = node.tagName.toLowerCase();
      if (DANGEROUS_TAGS.has(tag)) {
        node.remove();
        continue;
      }
      if (!ALLOWED_TAGS.has(tag)) {
        while (node.firstChild) {
          parent.insertBefore(node.firstChild, node);
        }
        node.remove();
        continue;
      }
      const attrs = Array.from(node.attributes);
      const allowedAttrsForTag = ALLOWED_ATTRS[tag];
      for (const attr of attrs) {
        const attrName = attr.name.toLowerCase();
        if (attrName.startsWith("on") || !allowedAttrsForTag || !allowedAttrsForTag.has(attrName)) {
          node.removeAttribute(attr.name);
          continue;
        }
        if (tag === "a" && attrName === "href") {
          if (!isSafeUrl(attr.value)) {
            node.removeAttribute("href");
          } else if (node.getAttribute("target") === "_blank") {
            node.setAttribute("rel", "noopener noreferrer");
          }
        } else if (tag === "img" && attrName === "src") {
          if (!isSafeUrl(attr.value)) {
            node.removeAttribute("src");
          }
        } else if (attrName === "class") {
          const safeClasses = attr.value
            .split(/\s+/)
            .filter((c) => /^language-[a-zA-Z0-9_-]+$/.test(c) || c === "mono")
            .join(" ");
          if (safeClasses) {
            node.setAttribute("class", safeClasses);
          } else {
            node.removeAttribute("class");
          }
        }
      }
      if (tag === "img" && !node.hasAttribute("src")) {
        node.remove();
        continue;
      }
      sanitizeNode(node);
    } else if (node.nodeType === 8) {
      node.remove();
    } else if (node.nodeType !== 3) {
      node.remove();
    }
  }
}

export function sanitizeMarkdownHtml(html) {
  if (!html) return "";
  if (typeof DOMParser === "undefined") {
    return esc(html);
  }
  const parser = new DOMParser();
  const doc = parser.parseFromString(`<body>${html}</body>`, "text/html");
  sanitizeNode(doc.body);
  return doc.body.innerHTML;
}

export async function renderMarkdown(content) {
  if (!content) return "";
  let raw = "";
  try {
    const m = await getMarked();
    if (m && typeof m.parse === "function") {
      raw = m.parse(content);
    } else {
      raw = fallbackMarkdown(content);
    }
  } catch {
    raw = fallbackMarkdown(content);
  }
  return sanitizeMarkdownHtml(raw);
}

export async function fetchBrainEntry(sessionId, path) {
  try {
    const data = await api(
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/brain/entry?path=${encodeURIComponent(path)}`,
    );
    return { ok: true, entry: data };
  } catch (err) {
    if (err.status === 404) {
      return { ok: false, status: 404, error: "Entry not found" };
    }
    if (err.status === 409) {
      return { ok: false, status: 409, error: "Path is a directory" };
    }
    if (err.status === 422) {
      return { ok: false, status: 422, error: "Entry is not valid UTF-8" };
    }
    return { ok: false, status: err.status || 500, error: err.message || "Could not read entry" };
  }
}

export function kvAsideHTML(path, content) {
  const copyGlyph = glyphSvg("copy", { size: 14 });
  return `
    <div class="shell-head">
      <div class="shell-title"><span class="shell-title-line">Key</span></div>
      <span class="shell-meta mono">${esc(path)}</span>
    </div>
    <div class="shell-controls">
      <span class="shell-meta mono">${esc(path)}</span>
      <div class="grow"></div>
      <button type="button" class="hub-btn-glyph" data-action="copy-kv-value" data-value="${esc(content)}" aria-label="Copy key value">
        ${copyGlyph}
      </button>
    </div>
    <div class="shell-body">
      <pre class="mono session-kv-value session-kv-read">${esc(content)}</pre>
    </div>
  `;
}

// The same value on a coarse pointer, where the aside is not drawn at all: one
// bottom sheet, the precedent the comments drawer sets. It is mounted only
// where the aside is not, so no code path mounts both (one surface per thing).
export function kvSheetHTML(path, content) {
  const copyGlyph = glyphSvg("copy", { size: 16 });
  return `
    <div class="kv-sheet-backdrop" data-action="kv-sheet-close"></div>
    <div class="kv-sheet" role="dialog" aria-labelledby="kv-sheet-title">
      <div class="hub-sheet-handle" aria-hidden="true"></div>
      <div class="hub-sheet-head">
        <span class="hub-sheet-title" id="kv-sheet-title">Key</span>
        <span class="shell-meta mono kv-sheet-path">${esc(path)}</span>
        <button type="button" class="hub-btn-glyph kv-sheet-copy" data-action="copy-kv-value" data-value="${esc(content)}" aria-label="Copy key value">
          ${copyGlyph}
        </button>
        <button type="button" class="hub-sheet-close" data-action="kv-sheet-close" aria-label="Close key value">
          ${glyphSvg("close", { size: 18 })}
        </button>
      </div>
      <pre class="mono session-kv-value kv-sheet-value">${esc(content)}</pre>
    </div>
  `;
}

export function errorAsideHTML(path, message) {
  return `
    <div class="shell-head">
      <div class="shell-title"><span class="shell-title-line">Entry</span></div>
      <span class="shell-meta mono">${esc(path)}</span>
    </div>
    <div class="shell-controls">
      <span class="shell-meta mono">${esc(path)}</span>
    </div>
    <div class="shell-body session-kv-missing">
      <p class="empty">${esc(message)}</p>
    </div>
  `;
}

if (typeof document !== "undefined") {
  // Esc closes the coarse-pointer key sheet, the same way it closes the
  // comments drawer, so the sheet is not a trap on a keyboard.
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const menu = document.querySelector(".session-reassign-menu:not([hidden])");
    if (menu) {
      const trigger = menu.closest(".session-reassign")?.querySelector('[data-action="reassign-open"]');
      trigger?.setAttribute("aria-expanded", "false");
      menu.hidden = true;
      trigger?.focus();
      return;
    }
    const sheet = document.querySelector(".kv-sheet:not([hidden])");
    if (!sheet) return;
    sheet.hidden = true;
    const backdrop = document.querySelector(".kv-sheet-backdrop");
    if (backdrop) backdrop.hidden = true;
  });

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

    const reassignOpen = event.target.closest?.('[data-action="reassign-open"]');
    if (reassignOpen) {
      await openReassignMenu(reassignOpen);
      return;
    }

    const reassignTo = event.target.closest?.('[data-action="reassign"]');
    if (reassignTo) {
      const menu = reassignTo.closest(".session-reassign-menu");
      if (menu) menu.hidden = true;
      const opener = reassignTo.closest(".session-reassign")?.querySelector('[data-action="reassign-open"]');
      opener?.setAttribute("aria-expanded", "false");
      await reassignSession(reassignTo.dataset.id, reassignTo.dataset.agent, reassignTo.dataset.from);
      return;
    }

    // Any click that is not on the trigger or its items closes an open menu.
    const openMenu = document.querySelector(".session-reassign-menu:not([hidden])");
    if (openMenu) {
      const trigger = openMenu.closest(".session-reassign")?.querySelector('[data-action="reassign-open"]');
      trigger?.setAttribute("aria-expanded", "false");
      openMenu.hidden = true;
    }

    const pruneAllBtn = event.target.closest?.('[data-action="prune-all"]');
    if (pruneAllBtn) {
      const project = pruneAllBtn.dataset.project;
      await pruneAllEnded(project);
      return;
    }

    const copyFileBtn = event.target.closest?.('[data-action="copy-file-path"]');
    if (copyFileBtn) {
      const path = copyFileBtn.dataset.path;
      if (path) {
        try {
          await navigator.clipboard.writeText(path);
        } catch {}
        toast("Copied file path.");
      }
      return;
    }

    const copyKvBtn = event.target.closest?.('[data-action="copy-kv-value"]');
    if (copyKvBtn) {
      const val = copyKvBtn.dataset.value;
      if (val != null) {
        try {
          await navigator.clipboard.writeText(val);
        } catch {}
        toast("Copied key value.");
      }
      return;
    }

    // The coarse-pointer kv sheet closes on its backdrop or its own close
    // control, and either way the reader stays on the sessions screen with the
    // tree behind it.
    const closeKv = event.target.closest?.('[data-action="kv-sheet-close"]');
    if (closeKv) {
      const sheet = document.querySelector(".kv-sheet");
      if (sheet) {
        sheet.hidden = true;
        sheet.setAttribute("aria-hidden", "true");
      }
      const backdrop = document.querySelector(".kv-sheet-backdrop");
      if (backdrop) backdrop.hidden = true;
      return;
    }
  });
}
