// The project knowledge base: the Wiki segment of a project. The project view
// owns the section chrome; this module builds the tree and the stage for it.
//
// The tree is one `meta=1` listing, so a directory view, a tree and the
// needs-review queue all cost one request. The stage reads one page, renders
// it, and edits it back with the version token the read carried, so a page
// that changed under the reader is refused rather than overwritten.
//
// Copy and geometry follow ROUND-3 as round 14 answers it: 12px mono for
// literal data, no trust ladder, no rename or move in this version.

import { api } from "./api.mjs";
import { composer } from "./composer.mjs";
import { confirmAction, slugify } from "./dialog.mjs";
import { diffStat, hunks, lineDiff, onlyFinalNewline } from "./diff.mjs";
import { esc } from "./dom.mjs";
import { read as readFrontmatter } from "./frontmatter.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { render } from "./router.mjs";
import { renderMarkdown } from "./sessions.mjs";
import { toast } from "./toast.mjs";
import { fullStamp } from "./time.mjs";

const FILE_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/></svg>`;
const FOLDER_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h6l2 2h10v11H3z"/></svg>`;
const CLOCK_GLYPH = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>`;
const staleMark = (entry) =>
  entry && entry.stale
    ? `<span class="wiki-stale">${CLOCK_GLYPH}stale</span>`
    : "";

export function wikiPageApi(id, path) {
  const encoded = String(path || "")
    .split("/")
    .map(encodeURIComponent)
    .join("/");
  return `/api/v1/projects/${encodeURIComponent(id)}/kb/pages/${encoded}`;
}

export function wikiPageHash(id, path, extra = "") {
  return `#/projects/${encodeURIComponent(id)}/wiki?page=${encodeURIComponent(path)}${extra}`;
}

// A page's comment threads. One level: a reply is a comment on the page, not on
// another comment. Open threads are listed; resolved ones fold under a count.
// The anchor line is the quote alone, clamped to two lines. The human cannot
// delete an agent's comment, so there is no delete control here.
async function commentsSection(id, path) {
  let comments = [];
  try {
    const data = await api(
      `/api/v1/projects/${encodeURIComponent(id)}/kb/comments?path=${encodeURIComponent(displayPath(path))}`,
    );
    comments = data.comments || [];
  } catch {
    comments = [];
  }
  const open = comments.filter((comment) => !comment.done);
  const done = comments.filter((comment) => comment.done);
  const thread = (comment, resolved) => `<div class="wiki-comment">
    <div class="wiki-comment-head">
      <span class="wiki-comment-author">${esc(comment.author)}</span>
      <span class="mono wiki-comment-time">${esc(comment.created_at)}</span>
      <span class="wiki-comment-gap"></span>
      ${resolved ? "" : `<button type="button" class="btn-outline wiki-comment-resolve" data-action="wiki-comment-resolve" data-id="${esc(id)}" data-comment="${esc(comment.id)}">Resolve</button>`}
    </div>
    ${
      comment.anchor && comment.anchor.quote
        ? `<p class="wiki-comment-quote">\u201c${esc(comment.anchor.quote)}\u201d</p>`
        : ""
    }
    <p class="wiki-comment-body">${esc(comment.body)}</p>
  </div>`;
  return `<section class="wiki-comments">
    <div class="mono wiki-section-label">COMMENTS · ${open.length}</div>
    ${open.length ? open.map((comment) => thread(comment, false)).join("") : `<p class="empty wiki-note-empty">No comments yet.</p>`}
    <div class="wiki-comment-composer" data-id="${esc(id)}" data-path="${esc(displayPath(path))}"></div>
    ${
      done.length
        ? `<details class="wiki-resolved"><summary class="mono wiki-resolved-summary">Resolved · ${done.length}</summary>${done.map((comment) => thread(comment, true)).join("")}</details>`
        : ""
    }
  </section>`;
}

// The trust label is derived by the hub, never stored. It is a word, not a
// colour: "unverified", "human reviewed", "machine confirmed", or "edited
// since review".
function trustWords(entry) {
  switch (entry.trust) {
    case "human_reviewed":
      return "human reviewed";
    case "machine_confirmed":
      return "machine confirmed";
    case "edited_since_review":
      return "edited since review";
    default:
      return "unverified";
  }
}

function depthOf(path) {
  return Math.max(0, String(path).split("/").length - 1);
}

// The hub stores a page under the knowledge base's filesystem, so a read
// echoes the path with its `/fs/` prefix. The reader sees the path they and
// their agents use.
function displayPath(path) {
  return String(path || "")
    .replace(/^\/fs\//, "")
    .replace(/^\//, "");
}

function rowHTML(entry, id, selected, narrow = false) {
  const path = entry.path;
  const depth = depthOf(path);
  const name = entry.title || path.split("/").pop();
  const on = path === selected ? ' aria-current="true"' : "";
  if (entry.type === "dir") {
    // A directory is structure, not a page. On a phone it drills in; on a
    // desktop the full tree already shows what it holds, so it is a label and
    // does not navigate. Either way it must never address a page, which is what
    // sent the reader to a 404 for a folder that was in the tree beside it.
    const inner = `<span class="wiki-row-glyph" aria-hidden="true">${FOLDER_GLYPH}</span>
      <span class="wiki-row-name">${esc(path.split("/").pop())}</span>
      ${staleMark(entry)}
      <span class="mono wiki-dir-count">${entry.children ?? 0}</span>`;
    if (narrow) {
      const dirHref = `#/projects/${encodeURIComponent(id)}/wiki?dir=${encodeURIComponent(displayPath(path))}`;
      return `<a class="row wiki-row wiki-dir" role="treeitem" aria-level="${depth + 1}" href="${dirHref}"${on} style="--depth:${depth}">${inner}</a>`;
    }
    return `<div class="row wiki-row wiki-dir" role="treeitem" aria-level="${depth + 1}" style="--depth:${depth}">${inner}</div>`;
  }
  const href = wikiPageHash(id, path);
  const meta = [entry.page_type || "concept", entry.status || "draft", trustWords(entry)].join(" · ");
  // The row is a column flexbox that inherits the row base's `align-items:
  // flex-start`, so a child is sized by its own content and may run past the
  // row rather than ellipsising inside it. The meta line asked for both halves
  // of the fix and needed both: `align-self:stretch` takes the row's width
  // instead of its content's, and `min-width:0` lets the no-wrap text shrink
  // inside that width. Without them its box ended 8.84px past a 300px index
  // pane, the pane's overflow cut the last glyph of "unverified", and the
  // ellipsis this span asks for never fired.
  return `<a class="row wiki-row" role="treeitem" aria-level="${depth + 1}" href="${href}"${on} style="--depth:${depth}">
    <span class="wiki-row-line">
      <span class="wiki-row-glyph" aria-hidden="true">${FILE_GLYPH}</span>
      <span class="wiki-row-name">${esc(name)}</span>
      ${staleMark(entry)}
    </span>
    <span class="mono wiki-row-meta">${esc(meta)}</span>
  </a>`;
}

export async function wikiIndexBody(id, selected, projectName = "", dir = "") {
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages?meta=1`);
  } catch (error) {
    return `<p class="empty wiki-index-note">Could not read the wiki: ${esc(error.message)}</p>`;
  }
  const entries = data.entries || [];
  if (!entries.length) {
    // The instruction the agent needs is a literal string, so it is shown on
    // the row that owns it and the copy control is a glyph on that row, which
    // is what the design's copy component is (no visible "Copy" label).
    const command = `hub wiki write ${projectName || id} --type concept`;
    return `<div class="wiki-empty">
      <div class="mono wiki-empty-label">wiki · empty</div>
      <h2 class="wiki-empty-title">No wiki yet in ${esc(projectName || id)}.</h2>
      <p class="wiki-empty-lead">Agents write durable knowledge here; sessions come and go, these pages stay. The first write creates index.md.</p>
      <div class="wiki-instruction-row">
        <code class="mono wiki-instruction-command">${esc(command)}</code>
        <button type="button" class="hub-btn-glyph" data-action="wiki-instruction" data-id="${esc(id)}" data-project="${esc(projectName || id)}" aria-label="Copy agent instruction">${glyphSvg("copy", { size: 16 })}</button>
      </div>
    </div>`;
  }
  const narrow = typeof window !== "undefined" && window.matchMedia("(max-width: 767px)").matches;
  if (narrow) {
    const prefix = dir ? `${displayPath(dir)}/` : "";
    const level = entries.filter((entry) => {
      const path = displayPath(entry.path);
      if (!path.startsWith(prefix)) return false;
      const rest = path.slice(prefix.length);
      return rest.length > 0 && !rest.includes("/");
    });
    // A trail is worth drawing only once there is somewhere to come back from.
    // At the root the crumb would say one word, "Wiki", which the tools row
    // above already says, so the index body is the tree and nothing else.
    const crumbs = displayPath(dir)
      ? [`<a class="wiki-crumb-link" href="#/projects/${encodeURIComponent(id)}/wiki">Wiki</a>`]
      : [];
    let acc = "";
    for (const part of displayPath(dir).split("/").filter(Boolean)) {
      acc = acc ? `${acc}/${part}` : part;
      crumbs.push(
        `<a class="wiki-crumb-link" href="#/projects/${encodeURIComponent(id)}/wiki?dir=${encodeURIComponent(acc)}">${esc(part)}</a>`,
      );
    }
    const trail = crumbs.length
      ? `<div class="wiki-breadcrumb mono">${crumbs.join(
          "<span>/</span>",
        )}</div>`
      : "";
    return `${trail}${level.length ? `<div class="wiki-tree" role="tree" aria-label="Wiki pages">${level.map((entry) => rowHTML(entry, id, selected, true)).join("")}</div>` : `<p class="empty wiki-index-note">This directory is empty.</p>`}`;
  }
  return `<div class="wiki-tree" role="tree" aria-label="Wiki pages">${entries
    .map((entry) => rowHTML(entry, id, selected))
    .join("")}</div>`;
}

// A page, or null when the hub says it is not there. Any other failure is
// thrown: a hub that could not answer has not said the page is gone, so the
// reader is never offered a restore on the strength of an outage.
async function readPage(id, path) {
  try {
    return await api(wikiPageApi(id, path));
  } catch (error) {
    if (error?.status === 404) return null;
    throw error;
  }
}

// The stage for a page the hub could not be asked about.
function unreadable(id, path, error, shellStageHead) {
  return {
    head: shellStageHead("Wiki", displayPath(path), "", `#/projects/${encodeURIComponent(id)}/wiki`),
    controls: `<div class="shell-controls"><span class="shell-meta mono">${esc(id)} / wiki</span></div>`,
    body: `<div class="shell-pad"><p class="empty">Could not read this page: ${esc(error?.message || "the hub did not answer")}</p></div>`,
  };
}

// The frontmatter is metadata, not body. A page whose block cannot be read is
// shown whole rather than refused, because the reader may still want it.
function pageFrontmatter(content) {
  try {
    return readFrontmatter(content || "");
  } catch {
    return { type: null, status: null, tags: [], title: null, description: null, body: content || "" };
  }
}

function pageBody(content) {
  return pageFrontmatter(content).body;
}

async function pageStage(id, path, shellStageHead) {
  let page;
  try {
    page = await readPage(id, path);
  } catch (error) {
    return unreadable(id, path, error, shellStageHead);
  }
  if (!page) {
    // A deleted page keeps its history, which is where it is restored from,
    // unless the operator forgot its versions: then the history only says who
    // wrote it.
    const history = await readHistory(id, path, 200);
    const listed = history && history.total > 0;
    const kept = listed && (history.versions || []).some((row) => row.kept);
    const link = `<a class="wiki-history-ref" href="${wikiPageHash(id, displayPath(path), "&history=1")}">history</a>`;
    return {
      head: shellStageHead("Wiki", "", "", `#/projects/${encodeURIComponent(id)}/wiki`),
      controls: `<div class="shell-controls"><span class="shell-meta mono">${esc(id)} / wiki</span></div>`,
      body: `<div class="shell-pad"><p class="empty">${
        kept
          ? `That page was deleted. Its ${link} holds its earlier versions.`
          : listed
            ? `That page was deleted and its earlier versions were forgotten. Its ${link} still says who wrote it and when.`
            : "That page is not in the wiki."
      }</p></div>`,
    };
  }
  const rendered = await renderMarkdown(pageBody(page.content));
  let backlinks = [];
  try {
    backlinks = await api(
      `/api/v1/projects/${encodeURIComponent(id)}/kb/backlinks?path=${encodeURIComponent(path)}`,
    );
  } catch {
    backlinks = [];
  }
  const fm = pageFrontmatter(page.content);
  let entry = null;
  try {
    const listing = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages?meta=1`);
    entry = (listing.entries || []).find((row) => displayPath(row.path) === displayPath(page.path));
  } catch {
    entry = null;
  }
  const needsReview = !entry || entry.trust !== "human_reviewed";
  // The reader's two header controls share one box: Edit is an anchor, so it
  // takes the button class and the stylesheet sizes both for the band.
  const reviewBtn = `<button type="button" class="btn-outline wiki-review-btn" data-action="wiki-review" data-id="${esc(id)}" data-path="${esc(displayPath(page.path))}" data-version="${esc(page.version)}">${needsReview ? "Review" : "Review again"}</button>`;
  const last = page.last_write ? `${page.last_write.actor} · ${page.last_write.at}` : "no write recorded";
  const meta = [
    displayPath(page.path),
    fm.type || "concept",
    fm.status || "draft",
    ...(fm.tags || []),
  ].join(" · ");
  const back = `<div class="wiki-backlinks">
    <div class="mono wiki-section-label">BACKLINKS · ${(backlinks || []).length}</div>
    ${
      (backlinks || []).length
        ? (backlinks || [])
            .map(
              (link) =>
                `<a class="row wiki-backlink" href="${wikiPageHash(id, displayPath(link.path))}">${esc(link.title || displayPath(link.path))}</a>`,
            )
            .join("")
        : `<p class="empty wiki-note-empty">No page links here yet.</p>`
    }
  </div>`;
  return {
    head: shellStageHead(
      fm.title || displayPath(page.path).split("/").pop(),
      meta,
      `<span class="wiki-page-actions">${reviewBtn}<a class="button btn-outline wiki-edit-link" href="${wikiPageHash(id, path, "&edit=1")}">Edit</a></span>`,
      `#/projects/${encodeURIComponent(id)}/wiki`,
    ),
    controls: `<div class="shell-controls wiki-page-controls">${staleMark(entry)}<span class="shell-meta mono wiki-meta-line">${esc(last)}</span><a class="wiki-history-link" href="${wikiPageHash(id, displayPath(page.path), "&history=1")}">History</a></div>`,
    body: `<article class="shell-prose wiki-page wiki-article">${fm.description ? `<p class="wiki-description">${esc(fm.description)}</p>` : ""}${rendered}</article>${back}${await commentsSection(id, path)}`,
  };
}

function editorStage(id, path, content, version, isNew, shellStageHead) {
  const title = isNew ? "New page" : path.split("/").pop();
  const note = isNew ? "Saved as type: concept · status: draft, unverified." : `Editing ${path}`;
  const back = `#/projects/${encodeURIComponent(id)}/wiki${
    path ? `?page=${encodeURIComponent(path)}` : ""
  }`;
  return {
    head: shellStageHead(title, "", "", back),
    controls: `<div class="shell-controls wiki-controls"><span class="shell-meta mono">${esc(note)}</span></div>`,
    body: `<div class="wiki-editor">
      ${
        isNew
          ? `<label class="wiki-editor-field"><span class="wiki-editor-label">Path</span>
             <input class="wiki-editor-path" id="wiki-new-path" name="path" autocomplete="off" placeholder="runbooks/deploy.md"></label>`
          : ""
      }
      <label class="wiki-editor-content">
        <span class="sr-only">Page content</span>
        <textarea class="wiki-editor-text" id="wiki-content" name="content" spellcheck="false">${esc(content)}</textarea>
      </label>
      <div class="wiki-editor-actions">
        <button type="button" class="primary wiki-editor-save" data-action="wiki-save" data-id="${esc(id)}" data-path="${esc(path)}" data-version="${esc(version)}" data-new="${isNew ? "1" : "0"}">${isNew ? "Create page" : "Save"}</button>
        <a class="button btn-outline wiki-editor-cancel" href="${back}">Cancel</a>
        <span class="wiki-editor-note"></span>
      </div>
    </div>`,
  };
}

// The stage for the wiki segment, given the route's query and the shell's
// stage-head builder (the project view owns the chrome, so it is passed in
// rather than imported, which would be a module cycle).
const OP_WORDS = {
  "kb.put": "wrote",
  "kb.delete": "deleted",
  "kb.review": "reviewed",
  "kb.promote": "promoted",
  "kb.revert": "reverted",
};

function simpleStage(shellStageHead, id, title, controls, body, actions = "") {
  return {
    head: shellStageHead(title, "", actions, `#/projects/${encodeURIComponent(id)}/wiki`),
    controls: `<div class="shell-controls wiki-controls"><span class="shell-meta mono wiki-meta-line">${esc(controls)}</span></div>`,
    body,
  };
}

// Recent changes: who did what, to which page, and when. A row opens that
// page's history, which is where a change is read and undone.
const HISTORY_PAGE = 25;

// The write log, newest first, grouped under its day. The newest page is
// drawn; Earlier walks back one page of rows at a time in place, its count
// falling until every row is loaded.
function historyRows(rows, changesProject) {
  let out = "";
  let day = null;
  for (const row of rows) {
    const stamp = String(row.at || "");
    const date = stamp.slice(0, 10);
    if (date !== day) {
      day = date;
      out += `<div class="mono wiki-history-day">${esc(date)}</div>`;
    }
    out += `<a class="row wiki-change" href="${wikiPageHash(changesProject, displayPath(row.path), "&history=1")}">
      <span class="mono wiki-change-time">${esc(stamp)}</span>
      <span class="wiki-change-actor">${esc(row.actor)}</span>
      <span class="mono wiki-change-path">${esc(displayPath(row.path))}</span>
      <span class="wiki-change-op">${esc(OP_WORDS[row.op] || row.op)}</span>
    </a>`;
  }
  return out;
}

function historyEarlier(id, data, loaded) {
  if (data.truncated && data.next_before) {
    const remaining = Math.max(0, (data.total ?? loaded) - loaded);
    return `<button type="button" class="btn-outline wiki-history-earlier" data-action="wiki-history-earlier" data-id="${esc(id)}" data-before="${esc(data.next_before)}">Earlier · ${remaining}</button>`;
  }
  return `<div class="mono wiki-history-done">${loaded} change${loaded === 1 ? "" : "s"} · all loaded</div>`;
}

async function changesStage(id, shellStageHead) {
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/history?limit=${HISTORY_PAGE}`);
  } catch (error) {
    return simpleStage(shellStageHead, id, "Recent changes", "", `<div class="shell-pad"><p class="empty">${esc(error.message)}</p></div>`);
  }
  const rows = data.rows || [];
  const body = rows.length
    ? `<div class="wiki-changes">${historyRows(rows, id)}${historyEarlier(id, data, rows.length)}</div>`
    : `<div class="wiki-empty"><div class="mono wiki-empty-label">wiki · recent changes</div><h2 class="wiki-empty-title">No changes yet.</h2><p class="wiki-empty-text">Every write to this wiki is logged here with who and when.</p></div>`;
  return simpleStage(shellStageHead, id, "Recent changes", `${data.total ?? rows.length} change${(data.total ?? rows.length) === 1 ? "" : "s"}`, body);
}

// Walk one page further back, in place, keeping the control's position.
export async function wikiHistoryEarlier(button) {
  const id = button.dataset.id || "";
  const before = button.dataset.before || "";
  button.disabled = true;
  try {
    const data = await api(
      `/api/v1/projects/${encodeURIComponent(id)}/kb/history?limit=${HISTORY_PAGE}&before=${encodeURIComponent(before)}`,
    );
    const holder = button.parentElement;
    const rows = data.rows || [];
    const temp = document.createElement("div");
    temp.innerHTML = historyRows(rows, id);
    while (temp.firstChild) holder.insertBefore(temp.firstChild, button);
    const loaded = holder.querySelectorAll(".wiki-change").length;
    const next = historyEarlier(id, data, loaded);
    if (next.startsWith("<button")) {
      const temp2 = document.createElement("div");
      temp2.innerHTML = next;
      const fresh = temp2.firstElementChild;
      button.replaceWith(fresh);
    } else {
      const temp2 = document.createElement("div");
      temp2.innerHTML = next;
      button.replaceWith(temp2.firstElementChild);
    }
  } catch (error) {
    button.disabled = false;
    toast(error.message);
  }
}

// A page's history: every write to it, newest first, each one a version to
// read against the page as it is now. The hub keeps the bytes of every write,
// so a deleted page lists its versions too.
async function readHistory(id, path, limit = HISTORY_PAGE, before = "") {
  try {
    return await api(
      `/api/v1/projects/${encodeURIComponent(id)}/kb/versions?path=${encodeURIComponent(displayPath(path))}&limit=${limit}${
        before ? `&before=${encodeURIComponent(before)}` : ""
      }`,
    );
  } catch {
    return null;
  }
}

function versionRows(id, path, rows) {
  return rows
    .map((row) => {
      const size = row.size_bytes == null ? "" : `${row.size_bytes} B`;
      const facts = [row.actor, row.summary, size].filter(Boolean).join(" · ");
      const state = row.current ? "current" : row.version && !row.kept ? "not kept" : "";
      const inner = `<span class="wiki-version-line">
          <span class="mono wiki-version-time">${esc(row.at)}</span>
          ${state ? `<span class="mono wiki-version-state">${esc(state)}</span>` : ""}
        </span>
        <span class="wiki-version-facts">${esc(facts)}</span>`;
      // A delete stored no bytes, and a version the hub did not keep has none
      // to show, so neither is a door.
      if (!row.version || !row.kept) return `<div class="row wiki-version">${inner}</div>`;
      const href = wikiPageHash(id, displayPath(path), `&history=1&version=${encodeURIComponent(row.version)}`);
      return `<a class="row wiki-version" href="${href}">${inner}</a>`;
    })
    .join("");
}

function versionsEarlier(id, path, data, loaded) {
  if (data.truncated && data.next_before) {
    const remaining = Math.max(0, (data.total ?? loaded) - loaded);
    return `<button type="button" class="btn-outline wiki-versions-earlier" data-action="wiki-versions-earlier" data-id="${esc(id)}" data-path="${esc(displayPath(path))}" data-before="${esc(data.next_before)}">Earlier · ${remaining}</button>`;
  }
  return `<div class="mono wiki-history-done">${loaded} version${loaded === 1 ? "" : "s"} · all loaded</div>`;
}

async function pageHistoryStage(id, path, shellStageHead) {
  const back = wikiPageHash(id, displayPath(path));
  const data = await readHistory(id, path);
  const head = (meta) => shellStageHead("History", meta, "", back);
  if (!data) {
    return {
      head: head(displayPath(path)),
      controls: `<div class="shell-controls wiki-controls"></div>`,
      body: `<div class="shell-pad"><p class="empty">Could not read this page's history.</p></div>`,
    };
  }
  const rows = data.versions || [];
  const total = data.total ?? rows.length;
  // Something to forget: a version whose bytes are kept and that the page does
  // not hold now.
  // An older write of the bytes the page holds now is not current, but it is
  // not forgettable either.
  const forgettable = rows.some((row) => row.kept && row.version !== data.current_version);
  const forget = forgettable
    ? `<button type="button" class="danger wiki-forget-btn" data-action="wiki-forget-history" data-id="${esc(id)}" data-path="${esc(displayPath(path))}" data-deleted="${data.current_version ? "0" : "1"}">Forget history</button>`
    : "";
  const body = rows.length
    ? `<div class="wiki-versions">${versionRows(id, path, rows)}${versionsEarlier(id, path, data, rows.length)}</div>`
    : `<div class="wiki-empty"><div class="mono wiki-empty-label">wiki · history</div><h2 class="wiki-empty-title">No history yet.</h2><p class="wiki-empty-text">Every write to this page keeps a version here.</p></div>`;
  return {
    head: shellStageHead("History", displayPath(path), forget, back),
    controls: `<div class="shell-controls wiki-controls"><span class="shell-meta mono">${total} version${total === 1 ? "" : "s"}${data.current_version ? "" : " · deleted"}</span></div>`,
    body,
  };
}

// Walk one page of versions further back, in place.
export async function wikiVersionsEarlier(button) {
  const id = button.dataset.id || "";
  const path = button.dataset.path || "";
  button.disabled = true;
  const data = await readHistory(id, path, HISTORY_PAGE, button.dataset.before || "");
  if (!data) {
    button.disabled = false;
    toast("Could not read the earlier versions.");
    return;
  }
  const holder = button.parentElement;
  const temp = document.createElement("div");
  temp.innerHTML = versionRows(id, path, data.versions || []);
  while (temp.firstChild) holder.insertBefore(temp.firstChild, button);
  const loaded = holder.querySelectorAll(".wiki-version").length;
  temp.innerHTML = versionsEarlier(id, path, data, loaded);
  button.replaceWith(temp.firstElementChild);
}

// One diff line. The mark and a hidden word carry what changed, so the tint
// is never the only way to tell an added line from a removed one.
function diffLine(line) {
  if (line.kind === "skip") {
    return `<div class="wiki-diff-skip mono">${line.count} unchanged line${line.count === 1 ? "" : "s"}</div>`;
  }
  const mark = line.kind === "add" ? "+" : line.kind === "del" ? "-" : " ";
  const word = line.kind === "add" ? "Added: " : line.kind === "del" ? "Removed: " : "";
  return `<div class="wiki-diff-line" data-kind="${line.kind}"><span class="wiki-diff-mark" aria-hidden="true">${mark}</span>${
    word ? `<span class="sr-only">${word}</span>` : ""
  }<span class="wiki-diff-text">${esc(line.text) || " "}</span></div>`;
}

// One version read against the page as it is now: what changed since, line by
// line, and the one step that puts it back.
async function versionStage(id, path, version, shellStageHead) {
  const backToHistory = wikiPageHash(id, displayPath(path), "&history=1");
  const failed = (message) => ({
    head: shellStageHead("Version", displayPath(path), "", backToHistory),
    controls: `<div class="shell-controls wiki-controls"></div>`,
    body: `<div class="shell-pad"><p class="empty">${esc(message)}</p></div>`,
  });
  let older;
  try {
    older = await api(`${wikiPageApi(id, displayPath(path))}?version=${encodeURIComponent(version)}`);
  } catch (error) {
    return failed(error.message);
  }
  let now;
  try {
    now = await readPage(id, path);
  } catch (error) {
    return failed(error.message);
  }
  // The version read says when its bytes were written and by whom, so the
  // header and the dialog keep both however far back the version is.
  const when = older.at || "";
  const lines = lineDiff(older.content, now ? now.content : "");
  const name = pageFrontmatter(older.content).title || displayPath(path).split("/").pop();
  const meta = [when, older.actor].filter(Boolean).join(" · ");
  const revert = older.current
    ? ""
    : `<button type="button" class="btn-outline wiki-revert-btn" data-action="wiki-revert" data-id="${esc(id)}" data-path="${esc(displayPath(path))}" data-version="${esc(version)}" data-current="${esc(now ? now.version : "absent")}" data-when="${esc(when)}" data-name="${esc(name)}">${now ? "Revert to this" : "Restore this"}</button>`;
  let summary;
  if (older.current) summary = "This is the page as it is now.";
  else if (!now) summary = "The page was deleted after this version.";
  else if (!lines) summary = "Too many changes to show line by line.";
  else if (onlyFinalNewline(older.content, now.content)) summary = "Only the final newline differs.";
  else {
    const { added, removed } = diffStat(lines);
    summary = `Since this version: ${added} line${added === 1 ? "" : "s"} added, ${removed} removed.`;
  }
  // A diff too long to find, or one against a page that is gone, which would
  // mark every line removed, is shown as the version itself: what a revert or
  // a restore would put back.
  const body =
    older.current || !lines || !now
      ? `<article class="shell-prose wiki-page wiki-article">${await renderMarkdown(pageBody(older.content))}</article>`
      : `<div class="wiki-diff mono" role="group" aria-label="Changes since this version">${hunks(lines).map(diffLine).join("") || `<div class="wiki-diff-skip">${esc(summary === "Only the final newline differs." ? summary : "No line changed.")}</div>`}</div>`;
  return {
    head: shellStageHead(name, meta, revert, backToHistory),
    controls: `<div class="shell-controls wiki-controls"><span class="shell-meta wiki-meta-line">${esc(summary)}</span></div>`,
    body,
  };
}

// Put a page back to the version on screen, once the reader confirms. The
// revert carries the version the reader saw as current, so a page that moved
// on since is refused rather than overwritten, and a deleted page is restored
// only while it is still gone.
export async function wikiRevert(button) {
  const id = button.dataset.id || "";
  const path = button.dataset.path || "";
  const version = button.dataset.version || "";
  const current = button.dataset.current || "absent";
  const when = button.dataset.when || "";
  const name = button.dataset.name || path;
  const restoring = current === "absent";
  // The version's time as the rest of the app spells it, not the raw stamp.
  const at = Date.parse(when);
  const stamp = Number.isFinite(at) ? fullStamp(at) : "";
  const encoded = path.split("/").map(encodeURIComponent).join("/");
  const confirmed = await confirmAction({
    title: restoring ? `Restore ${name}?` : `Revert ${name}?`,
    body: `The page is written back as it was${stamp ? ` on ${stamp}` : ""}. This is a new write in its history, so ${
      restoring ? "the delete" : "the version it replaces"
    } stays there to go back to.`,
    safe: "Cancel",
    danger: restoring ? "Restore page" : "Revert page",
    tone: "primary",
    commit: async () => {
      await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages/${encoded}/revert`, {
        method: "POST",
        body: JSON.stringify({ version, if_version: current }),
      });
    },
  });
  if (!confirmed) return;
  toast(restoring ? "Page restored." : "Page reverted.");
  location.hash = wikiPageHash(id, path);
}

// Forget the kept bytes of a page's history, once the operator confirms. Every
// version but the one the page holds stops reading back; the rows stay.
export async function wikiForgetHistory(button) {
  const id = button.dataset.id || "";
  const path = button.dataset.path || "";
  const deleted = button.dataset.deleted === "1";
  const confirmed = await confirmAction({
    title: "Forget this page's history?",
    body: deleted
      ? "The page is deleted, so every earlier version goes, and the page can no longer be restored."
      : "Every earlier version goes, and none can be read or reverted to again. The page as it is now stays.",
    list: [displayPath(path)],
    note: "Who wrote what and when stays in the history. This cannot be undone.",
    safe: "Cancel",
    danger: "Forget history",
    commit: async () => {
      await api(
        `/api/v1/projects/${encodeURIComponent(id)}/kb/versions?path=${encodeURIComponent(displayPath(path))}`,
        { method: "DELETE" },
      );
    },
  });
  if (!confirmed) return;
  toast("History forgotten.");
  render();
}

// Lint: the tree's findings, with a Re-check that walks it again.
async function lintStage(id, params, shellStageHead) {
  const fresh = params?.get("fresh") === "1";
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/lint${fresh ? "?fresh=1" : ""}`);
  } catch (error) {
    return simpleStage(shellStageHead, id, "Lint", "", `<div class="shell-pad"><p class="empty">${esc(error.message)}</p></div>`);
  }
  const findings = data.findings || [];
  const recheck = `<a class="btn-outline wiki-recheck" href="#/projects/${encodeURIComponent(id)}/wiki?view=lint&fresh=1">Re-check</a>`;
  const body = findings.length
    ? findings
        .map(
          (finding) => `<div class="row wiki-finding">
        <span class="mono wiki-finding-code">${esc(finding.code)}</span>
        <span class="wiki-finding-message">${esc(finding.message)}</span>
        ${
          finding.path
            ? `<a class="mono wiki-finding-path" href="${wikiPageHash(id, displayPath(finding.path))}">${esc(displayPath(finding.path))}</a>`
            : ""
        }
      </div>`,
        )
        .join("")
    : `<div class="wiki-empty"><div class="mono wiki-empty-label">wiki · lint</div><h2 class="wiki-empty-title">Nothing to fix.</h2><p class="wiki-empty-text">Every page declares a type, every link resolves, and every page is listed in its index.</p></div>`;
  return {
    head: shellStageHead("Lint", `${findings.length} finding${findings.length === 1 ? "" : "s"}`, recheck, `#/projects/${encodeURIComponent(id)}/wiki`),
    controls: `<div class="shell-controls wiki-controls"><span class="shell-meta mono">checked ${esc(data.checked_at || "")}</span></div>`,
    body,
  };
}

// Needs review: the pages the hub judges not human-reviewed, in one list. A
// row opens the page, where the reviewer reads the bytes before stamping them.
async function reviewStage(id, shellStageHead) {
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages?meta=1`);
  } catch (error) {
    return simpleStage(shellStageHead, id, "Needs review", "", `<div class="shell-pad"><p class="empty">${esc(error.message)}</p></div>`);
  }
  const rows = (data.entries || []).filter((entry) => entry.type === "file" && entry.trust !== "human_reviewed");
  const body = rows.length
    ? rows
        .map(
          (entry) => `<a class="row wiki-review-row" href="${wikiPageHash(id, displayPath(entry.path))}">
        <span class="wiki-review-title">${esc(entry.title || displayPath(entry.path).split("/").pop())}</span>
        <span class="mono wiki-review-trust">${esc(trustWords(entry))}</span>
      </a>`,
        )
        .join("")
    : `<p class="empty wiki-index-note">Nothing needs review.</p>`;
  return simpleStage(shellStageHead, id, "Needs review", `${rows.length} page${rows.length === 1 ? "" : "s"}`, body);
}

// The Wiki home: the counts, and the housekeeping screens, which live here
// and never in the inbox.
async function homeStage(id, stats, shellStageHead, projectName = "") {
  let s = null;
  try {
    s = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/stats`);
  } catch {
    s = null;
  }
  const pages = s?.pages ?? stats?.kb_pages ?? 0;
  const review = s?.needs_review?.total ?? 0;
  const stale = s?.stale ?? 0;
  const link = (view, label) =>
    `<a class="row wiki-home-link" href="#/projects/${encodeURIComponent(id)}/wiki?view=${view}">${label}</a>`;
  const body = `<div class="wiki-home">
    <div class="wiki-home-new">
      <button type="button" class="primary wiki-new-btn" data-action="wiki-new" data-id="${esc(id)}" data-project="${esc(projectName || id)}">New page</button>
    </div>
    <div class="wiki-stats">
      <div><div class="mono wiki-stat-label">PAGES</div><div class="wiki-stat-value">${pages}</div></div>
      <div><div class="mono wiki-stat-label">NEEDS REVIEW</div><div class="wiki-stat-value">${review}</div></div>
      <div><div class="mono wiki-stat-label">STALE</div><div class="wiki-stat-value">${stale}</div></div>
    </div>
    ${link("review", "Needs review")}
    ${link("changes", "Recent changes")}
    ${link("lint", "Lint")}
  </div>`;
  return simpleStage(shellStageHead, id, "Wiki", `${pages} page${pages === 1 ? "" : "s"}`, body);
}

export async function wikiStage(id, params, shellStageHead, stats, projectName = "") {
  const selected = params?.get("page") || "";
  const editing = params?.get("edit") === "1";
  const isNew = params?.get("new") === "1";
  const view = params?.get("view") || "";
  if (view === "review") return reviewStage(id, shellStageHead);
  if (view === "changes") return changesStage(id, shellStageHead);
  if (view === "lint") return lintStage(id, params, shellStageHead);
  if (isNew) return editorStage(id, "", "", "absent", true, shellStageHead);
  if (selected && params?.get("history") === "1") {
    const version = params.get("version") || "";
    return version ? versionStage(id, selected, version, shellStageHead) : pageHistoryStage(id, selected, shellStageHead);
  }
  if (editing && selected) {
    let page;
    try {
      page = (await readPage(id, selected)) || { content: "", version: "absent" };
    } catch (error) {
      return unreadable(id, selected, error, shellStageHead);
    }
    return editorStage(id, selected, page.content || "", page.version || "absent", false, shellStageHead);
  }
  if (selected) return pageStage(id, selected, shellStageHead);
  return homeStage(id, stats, shellStageHead, projectName);
}

// The version token a Save carries back: the read's token for an edit, the
// literal "absent" for a create, which the hub accepts only when nothing is
// there.
export async function saveWikiPage(id, path, content, version) {
  const body = version && version !== "absent" ? { content, if_version: version } : { content, if_version: "absent" };
  return api(wikiPageApi(id, path), { method: "PUT", body: JSON.stringify(body) });
}

// Stamp a page as reviewed by the human. The version the reader saw travels
// with it, so a page that changed since cannot be stamped by mistake.
export async function wikiReview(button) {
  const id = button.dataset.id || "";
  const path = button.dataset.path || "";
  const version = button.dataset.version || "";
  const encoded = path.split("/").map(encodeURIComponent).join("/");
  button.disabled = true;
  try {
    await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages/${encoded}/review`, {
      method: "POST",
      body: JSON.stringify(version ? { if_version: version } : {}),
    });
    toast("Page reviewed.");
    render();
  } catch (error) {
    button.disabled = false;
    toast(error.message);
  }
}

// Mount the page's comment composer once the stage is painted. It is the one
// composer every screen uses; a post re-renders the page so the new thread is
// listed above it.
export function wireWikiComments(root) {
  const slot = root.querySelector(".wiki-comment-composer");
  if (!slot || slot.firstChild) return;
  const { id = "", path = "" } = slot.dataset;
  const made = composer({
    label: "Comment",
    placeholder: "Add a comment",
    action: "Post",
    empty: "Write a comment to post it.",
    // The hub refuses a page comment over 2000 characters.
    maxLength: 2000,
    send: async (body) => {
      await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/comments`, {
        method: "POST",
        body: JSON.stringify({ path, body }),
      });
      render();
    },
  });
  slot.appendChild(made.element);
}

// Resolve a thread. Resolved threads fold under a count at the foot.
export async function wikiCommentResolve(button) {
  const id = button.dataset.id || "";
  const comment = button.dataset.comment || "";
  button.disabled = true;
  try {
    await api(
      `/api/v1/projects/${encodeURIComponent(id)}/kb/comments/${encodeURIComponent(comment)}/done`,
      { method: "POST", body: JSON.stringify({ done: true }) },
    );
    render();
  } catch (error) {
    button.disabled = false;
    toast(error.message);
  }
}

// The one-line instruction an agent needs to start a wiki. Round 14 ships the
// card without the code block until the `kb` command exists; the control still
// copies the command for whoever wants it.
export function wikiInstruction(button) {
  const project = button.dataset.project || button.dataset.id || "";
  const command = `hub wiki write ${project} --type concept`;
  const done = () => toast("Agent instruction copied.");
  try {
    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(command).then(done, done);
      return;
    }
  } catch {}
  done();
}

// The New page control: the editor opens empty, and the path is typed there.
// The New page and Save to wiki sheet. One sheet, two entries: writing a page
// that is not there yet, and copying a session brain entry in. The copy follows
// the design: a Location, a Title and a Description, the file name computed from
// the title, and a note naming the type, status and source.
function openWikiSheet({ id, projectName = "", mode, sessionId = "", sessionName = "", fromPath = "" }) {
  const isNew = mode === "new";
  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog";
  const titleId = "wiki-sheet-title";
  el.setAttribute("aria-labelledby", titleId);

  const form = document.createElement("form");
  form.method = "dialog";

  const heading = document.createElement("h2");
  heading.className = "dialog-title";
  heading.id = titleId;
  heading.textContent = isNew ? "New page" : "Save to wiki";
  form.appendChild(heading);

  if (!isNew) {
    const meta = document.createElement("p");
    meta.className = "dialog-body";
    meta.textContent = `from ${sessionName} · brain ${displayPath(fromPath)}`;
    form.appendChild(meta);
  }

  const field = (name, label, value) => {
    const wrap = document.createElement("label");
    wrap.className = "dialog-field-wrap";
    const span = document.createElement("span");
    span.className = "dialog-label";
    span.textContent = label;
    const input = document.createElement("input");
    input.name = name;
    input.value = value;
    input.autocomplete = "off";
    input.className = "wiki-sheet-input";
    wrap.append(span, input);
    form.appendChild(wrap);
    return input;
  };

  const defaultTitle = isNew ? "" : (fromPath.split("/").pop() || "").replace(/\.md$/, "");
  const dirInput = field("dir", isNew ? "Location" : "Location", isNew ? "" : (fromPath.split("/").slice(1, -1).join("/") || ""));
  const titleInput = field("title", "Title", defaultTitle);
  const descInput = field("description", "Description", "");

  const computed = document.createElement("p");
  computed.className = "dialog-note mono wiki-sheet-path";
  form.appendChild(computed);

  const note = document.createElement("p");
  note.className = "dialog-note";
  note.textContent = isNew
    ? "Saved as type: concept · status: draft, unverified."
    : "Saved as type: concept · status: draft, unverified, with the brain entry listed under sources. The brain file is not moved.";
  form.appendChild(note);

  const problem = document.createElement("p");
  problem.className = "dialog-note";
  problem.hidden = true;
  form.appendChild(problem);

  const actions = document.createElement("div");
  actions.className = "dialog-actions";
  const safe = document.createElement("button");
  safe.type = "button";
  safe.className = "dialog-safe";
  safe.textContent = "Cancel";
  const commit = document.createElement("button");
  commit.type = "button";
  commit.className = "dialog-commit";
  commit.textContent = isNew ? "Create page" : "Save to wiki";
  actions.append(safe, commit);
  form.appendChild(actions);

  const fileName = () => {
    const dir = dirInput.value.trim().replace(/^\/+|\/+$/g, "");
    const slug = slugify(titleInput.value.trim()) || "page";
    return `${dir ? `${dir}/` : ""}${slug}.md`;
  };
  const paintName = () => {
    computed.textContent = fileName();
  };
  titleInput.addEventListener("input", paintName);
  dirInput.addEventListener("input", paintName);
  paintName();

  safe.addEventListener("click", () => el.close("cancel"));
  commit.addEventListener("click", async () => {
    const title = titleInput.value.trim();
    if (!title) {
      problem.textContent = "A page needs a title.";
      problem.hidden = false;
      titleInput.focus();
      return;
    }
    const description = descInput.value.trim();
    const toPath = fileName();
    commit.disabled = true;
    problem.hidden = true;
    try {
      if (isNew) {
        const body = `---\ntype: concept\nstatus: draft\ntitle: ${JSON.stringify(title)}\n${description ? `description: ${JSON.stringify(description)}\n` : ""}---\n# ${title}\n${description ? `\n${description}\n` : ""}`;
        await saveWikiPage(id, toPath, body, "absent");
      } else {
        await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/promote`, {
          method: "POST",
          body: JSON.stringify({
            from_session_id: sessionId,
            from_path: fromPath,
            to_path: toPath,
            title,
            description: description || undefined,
          }),
        });
      }
      el.close("saved");
      toast(isNew ? "Page created." : "Saved to the wiki.");
      location.hash = wikiPageHash(id, toPath);
    } catch (error) {
      commit.disabled = false;
      problem.textContent = error.message;
      problem.hidden = false;
    }
  });

  el.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const ring = [...el.querySelectorAll("input, button:not([disabled])")];
    if (!ring.length) return;
    const edge = event.shiftKey ? ring[0] : ring[ring.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    (event.shiftKey ? ring[ring.length - 1] : ring[0]).focus();
  });

  el.appendChild(form);
  document.body.appendChild(el);
  document.documentElement.classList.add("has-dialog");
  el.showModal();
  titleInput.focus();

  return new Promise((resolve) => {
    el.addEventListener(
      "close",
      () => {
        el.remove();
        document.documentElement.classList.remove("has-dialog");
        if (opener instanceof HTMLElement && opener.isConnected) {
          opener.focus({ preventScroll: true });
        }
        resolve(el.returnValue === "saved");
      },
      { once: true },
    );
  });
}

export function wikiNew(button) {
  openWikiSheet({ id: button.dataset.id || "", projectName: button.dataset.project || "", mode: "new" });
}

export function wikiPromote(button) {
  openWikiSheet({
    id: button.dataset.id || "",
    mode: "promote",
    sessionId: button.dataset.session || "",
    sessionName: button.dataset.sessionName || button.dataset.session || "",
    fromPath: button.dataset.path || "",
  });
}

export async function wikiSave(button) {
  const id = button.dataset.id || "";
  const isNew = button.dataset.new === "1";
  const path = isNew
    ? (document.getElementById("wiki-new-path")?.value || "").trim()
    : button.dataset.path || "";
  const content = document.getElementById("wiki-content")?.value ?? "";
  const version = button.dataset.version || "absent";
  const note = button.closest(".wiki-editor-actions")?.querySelector(".wiki-editor-note");
  const say = (message, tone = "") => {
    if (note) {
      note.textContent = message;
      if (tone) note.dataset.tone = tone;
      else delete note.dataset.tone;
    }
  };
  if (!path) {
    say("A page needs a path.", "danger");
    return;
  }
  button.disabled = true;
  say("");
  try {
    await saveWikiPage(id, path, content, version);
    toast("Page saved.");
    location.hash = wikiPageHash(id, path);
  } catch (error) {
    button.disabled = false;
    // A page that changed under the reader is refused, not overwritten. The
    // reader either takes the hub's copy or keeps theirs over it.
    if (error.status === 409 && note) {
      note.dataset.tone = "conflict";
      note.textContent = "This page changed while you were editing. ";
      const action = (label, run) => {
        const el = document.createElement("button");
        el.type = "button";
        el.className = "btn-outline wiki-conflict-btn";
        el.textContent = label;
        el.addEventListener("click", run);
        note.appendChild(el);
      };
      action("Reload theirs", async () => {
        let page;
        try {
          page = await readPage(id, path);
        } catch (failure) {
          say(failure.message, "danger");
          return;
        }
        if (page) {
          document.getElementById("wiki-content").value = page.content || "";
          button.dataset.version = page.version || "absent";
        }
        note.textContent = "Reloaded the hub's copy.";
      });
      action("Keep mine", async () => {
        let page;
        try {
          page = await readPage(id, path);
        } catch (failure) {
          say(failure.message, "danger");
          return;
        }
        if (page) button.dataset.version = page.version || "absent";
        note.textContent = "";
        wikiSave(button);
      });
      return;
    }
    say(error.message, "danger");
  }
}
