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
import { slugify } from "./dialog.mjs";
import { esc } from "./dom.mjs";
import { read as readFrontmatter } from "./frontmatter.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { render } from "./router.mjs";
import { renderMarkdown } from "./sessions.mjs";
import { toast } from "./toast.mjs";

const FILE_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/></svg>`;
const FOLDER_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h6l2 2h10v11H3z"/></svg>`;
const INPUT_CSS =
  "height:44px;box-sizing:border-box;padding:0 12px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--surface);color:var(--ink);font:500 14px/1 var(--font-sans)";
const CLOCK_GLYPH = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>`;
const staleMark = (entry) =>
  entry && entry.stale
    ? `<span class="wiki-stale" style="display:inline-flex;align-items:center;gap:3px;flex:none;font-size:12px;color:var(--ink-3)">${CLOCK_GLYPH}stale</span>`
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
  const thread = (comment, resolved) => `<div class="wiki-comment" style="padding:10px 0;border-top:1px solid var(--line)">
    <div style="display:flex;align-items:center;gap:8px">
      <span style="font-size:13px;font-weight:600">${esc(comment.author)}</span>
      <span class="mono" style="font-size:12px;color:var(--ink-3)">${esc(comment.created_at)}</span>
      <span style="flex:1"></span>
      ${resolved ? "" : `<button type="button" class="btn-outline" data-action="wiki-comment-resolve" data-id="${esc(id)}" data-comment="${esc(comment.id)}" style="flex:none;height:28px;padding:0 10px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 12px/1 var(--font-sans);cursor:pointer">Resolve</button>`}
    </div>
    ${
      comment.anchor && comment.anchor.quote
        ? `<p style="font-style:italic;font-size:13px;color:var(--ink-2);margin:4px 0;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden">\u201c${esc(comment.anchor.quote)}\u201d</p>`
        : ""
    }
    <p style="font-size:14px;line-height:1.45;margin:4px 0 0;white-space:pre-wrap">${esc(comment.body)}</p>
  </div>`;
  return `<section class="wiki-comments" style="padding:16px;border-top:1px solid var(--line)">
    <div class="mono" style="font-size:12px;color:var(--ink-3);letter-spacing:.06em;margin-bottom:8px">COMMENTS · ${open.length}</div>
    ${open.length ? open.map((comment) => thread(comment, false)).join("") : `<p class="empty" style="margin:0;font-size:13px">No comments yet.</p>`}
    <div class="wiki-comment-composer" style="display:flex;flex-direction:column;gap:8px;margin-top:12px">
      <label style="display:flex;flex-direction:column;gap:6px"><span class="sr-only">Comment</span>
        <textarea data-wiki-comment-body rows="3" placeholder="Add a comment" style="box-sizing:border-box;padding:10px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--surface);color:var(--ink);font:400 14px/1.45 var(--font-sans);resize:vertical"></textarea></label>
      <button type="button" class="primary" data-action="wiki-comment-add" data-id="${esc(id)}" data-path="${esc(displayPath(path))}" style="align-self:flex-start;height:34px;padding:0 14px">Comment</button>
    </div>
    ${
      done.length
        ? `<details style="margin-top:12px"><summary class="mono" style="font-size:12px;color:var(--ink-3);cursor:pointer">Resolved · ${done.length}</summary>${done.map((comment) => thread(comment, true)).join("")}</details>`
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
  const pad = 16 + depth * 20;
  const name = entry.title || path.split("/").pop();
  const on = path === selected ? ' aria-current="true"' : "";
  const href =
    narrow && entry.type === "dir"
      ? `#/projects/${encodeURIComponent(id)}/wiki?dir=${encodeURIComponent(displayPath(path))}`
      : wikiPageHash(id, path);
  if (entry.type === "dir") {
    return `<a class="row wiki-row wiki-dir" role="treeitem" aria-level="${depth + 1}" href="${href}"${on} style="display:flex;align-items:center;gap:8px;min-height:44px;padding:0 12px 0 ${pad}px;border-bottom:1px solid var(--line);color:var(--ink);text-decoration:none;box-sizing:border-box">
      <span aria-hidden="true" style="flex:none;color:var(--ink-3);display:inline-flex">${FOLDER_GLYPH}</span>
      <span style="flex:1;min-width:0;font-size:14px;font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(path.split("/").pop())}</span>
      ${staleMark(entry)}
      <span class="mono" style="flex:none;font-size:12px;color:var(--ink-3)">${entry.children ?? 0}</span>
    </a>`;
  }
  const meta = [entry.page_type || "concept", entry.status || "draft", trustWords(entry)].join(" · ");
  return `<a class="row wiki-row" role="treeitem" aria-level="${depth + 1}" href="${href}"${on} style="display:flex;flex-direction:column;justify-content:center;gap:3px;min-height:56px;padding:6px 12px 6px ${pad}px;border-bottom:1px solid var(--line);color:var(--ink);text-decoration:none;box-sizing:border-box">
    <span style="display:flex;align-items:center;gap:8px;min-width:0">
      <span aria-hidden="true" style="flex:none;color:var(--ink-3);display:inline-flex">${FILE_GLYPH}</span>
      <span style="flex:1;min-width:0;font-size:14px;font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(name)}</span>
      ${staleMark(entry)}
    </span>
    <span class="mono" style="padding-left:24px;font-size:12px;color:var(--ink-3);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(meta)}</span>
  </a>`;
}

export async function wikiIndexBody(id, selected, projectName = "", dir = "") {
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages?meta=1`);
  } catch (error) {
    return `<p class="empty" style="padding:16px">Could not read the wiki: ${esc(error.message)}</p>`;
  }
  const entries = data.entries || [];
  if (!entries.length) {
    // The instruction the agent needs is a literal string, so it is shown on
    // the row that owns it and the copy control is a glyph on that row, which
    // is what the design's copy component is (no visible "Copy" label).
    const command = `hub wiki write ${projectName || id} --type concept`;
    return `<div class="wiki-empty" style="padding:24px 16px;max-width:640px">
      <div class="mono" style="font-size:12px;color:var(--ink-3);letter-spacing:.06em">wiki · empty</div>
      <h2 style="font-size:17px;font-weight:600;margin:6px 0">No wiki yet in ${esc(projectName || id)}.</h2>
      <p style="font-size:13px;color:var(--ink-2);line-height:1.45;margin:0 0 12px">Agents write durable knowledge here; sessions come and go, these pages stay. The first write creates index.md.</p>
      <div class="wiki-instruction-row" style="display:flex;align-items:center;gap:8px;min-height:44px;padding:0 4px 0 12px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--surface-2)">
        <code class="mono" style="flex:1;min-width:0;font-size:13px;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(command)}</code>
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
      ? [`<a href="#/projects/${encodeURIComponent(id)}/wiki" style="color:var(--accent);text-decoration:none">Wiki</a>`]
      : [];
    let acc = "";
    for (const part of displayPath(dir).split("/").filter(Boolean)) {
      acc = acc ? `${acc}/${part}` : part;
      crumbs.push(
        `<a href="#/projects/${encodeURIComponent(id)}/wiki?dir=${encodeURIComponent(acc)}" style="color:var(--accent);text-decoration:none">${esc(part)}</a>`,
      );
    }
    const trail = crumbs.length
      ? `<div class="wiki-breadcrumb mono" style="padding:8px 16px;font-size:12px;color:var(--ink-3);display:flex;gap:6px;flex-wrap:wrap">${crumbs.join(
          "<span>/</span>",
        )}</div>`
      : "";
    return `${trail}${level.length ? `<div class="wiki-tree" role="tree" aria-label="Wiki pages">${level.map((entry) => rowHTML(entry, id, selected, true)).join("")}</div>` : `<p class="empty" style="padding:16px">This directory is empty.</p>`}`;
  }
  return `<div class="wiki-tree" role="tree" aria-label="Wiki pages">${entries
    .map((entry) => rowHTML(entry, id, selected))
    .join("")}</div>`;
}

async function readPage(id, path) {
  try {
    return await api(wikiPageApi(id, path));
  } catch {
    return null;
  }
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
  const page = await readPage(id, path);
  if (!page) {
    return {
      head: shellStageHead("Wiki", "", "", `#/projects/${encodeURIComponent(id)}/wiki`),
      controls: `<div class="shell-controls"><span class="shell-meta mono">${esc(id)} / wiki</span></div>`,
      body: `<div class="shell-pad"><p class="empty">That page is not in the wiki.</p></div>`,
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
  const reviewBtn = `<button type="button" class="btn-outline" data-action="wiki-review" data-id="${esc(id)}" data-path="${esc(displayPath(page.path))}" data-version="${esc(page.version)}" style="flex:none;height:30px;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 13px/1 var(--font-sans);cursor:pointer">${needsReview ? "Review" : "Review again"}</button>`;
  const last = page.last_write ? `${page.last_write.actor} · ${page.last_write.at}` : "no write recorded";
  const meta = [
    displayPath(page.path),
    fm.type || "concept",
    fm.status || "draft",
    ...(fm.tags || []),
  ].join(" · ");
  const back = `<div class="wiki-backlinks" style="padding:16px 16px 24px;border-top:1px solid var(--line)">
    <div class="mono" style="font-size:12px;color:var(--ink-3);letter-spacing:.06em;margin-bottom:8px">BACKLINKS · ${(backlinks || []).length}</div>
    ${
      (backlinks || []).length
        ? (backlinks || [])
            .map(
              (link) =>
                `<a class="row" href="${wikiPageHash(id, displayPath(link.path))}" style="display:block;padding:8px 0;color:var(--accent);text-decoration:none;font-size:14px">${esc(link.title || displayPath(link.path))}</a>`,
            )
            .join("")
        : `<p class="empty" style="margin:0;font-size:13px">No page links here yet.</p>`
    }
  </div>`;
  return {
    head: shellStageHead(
      fm.title || displayPath(page.path).split("/").pop(),
      meta,
      `<span style="display:inline-flex;gap:8px;flex:none">${reviewBtn}<a class="btn-outline" href="${wikiPageHash(id, path, "&edit=1")}" style="height:30px;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);color:var(--ink);text-decoration:none;font:600 13px/28px var(--font-sans)">Edit</a></span>`,
      `#/projects/${encodeURIComponent(id)}/wiki`,
    ),
    controls: `<div class="shell-controls" style="gap:10px;padding:0 16px">${staleMark(entry)}<span class="shell-meta mono" style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(last)}</span></div>`,
    body: `<article class="shell-prose wiki-page" style="max-width:640px;padding:16px">${fm.description ? `<p class="wiki-description" style="font-size:15px;color:var(--ink-2);margin-top:0">${esc(fm.description)}</p>` : ""}${rendered}</article>${back}${await commentsSection(id, path)}`,
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
    controls: `<div class="shell-controls" style="padding:0 16px"><span class="shell-meta mono">${esc(note)}</span></div>`,
    body: `<div class="wiki-editor" style="display:flex;flex-direction:column;gap:12px;padding:16px;height:100%;box-sizing:border-box">
      ${
        isNew
          ? `<label style="display:flex;flex-direction:column;gap:6px"><span style="font-size:13px;font-weight:600">Path</span>
             <input id="wiki-new-path" name="path" autocomplete="off" placeholder="runbooks/deploy.md" style="height:40px;box-sizing:border-box;padding:0 12px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--surface);color:var(--ink);font:500 14px/1 var(--font-mono)"></label>`
          : ""
      }
      <label style="display:flex;flex-direction:column;gap:6px;flex:1;min-height:0">
        <span class="sr-only">Page content</span>
        <textarea id="wiki-content" name="content" spellcheck="false" style="flex:1;min-height:240px;box-sizing:border-box;padding:12px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--surface);color:var(--ink);font:400 14px/1.5 var(--font-mono);resize:vertical">${esc(content)}</textarea>
      </label>
      <div class="wiki-editor-actions" style="display:flex;gap:8px;align-items:center">
        <button type="button" class="primary" data-action="wiki-save" data-id="${esc(id)}" data-path="${esc(path)}" data-version="${esc(version)}" data-new="${isNew ? "1" : "0"}" style="height:36px;padding:0 16px">${isNew ? "Create page" : "Save"}</button>
        <a class="btn-outline" href="${back}" style="height:36px;padding:0 14px;border-radius:var(--r-1);border:1px solid var(--line-strong);color:var(--ink);text-decoration:none;font:600 14px/34px var(--font-sans)">Cancel</a>
        <span class="wiki-editor-note" style="font-size:12px;color:var(--ink-3)"></span>
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
};

function simpleStage(shellStageHead, id, title, controls, body, actions = "") {
  return {
    head: shellStageHead(title, "", actions, `#/projects/${encodeURIComponent(id)}/wiki`),
    controls: `<div class="shell-controls" style="padding:0 16px"><span class="shell-meta mono" style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(controls)}</span></div>`,
    body,
  };
}

// Recent changes: who did what, to which page, and when. The rows are not
// tappable in this version (round 14 item 7), and a row says only what the log
// holds.
const HISTORY_PAGE = 25;

// The write log, newest first, grouped under its day. The newest page is
// drawn; Earlier walks back one page of rows at a time in place, its count
// falling until every row is loaded.
function historyRows(rows) {
  let out = "";
  let day = null;
  for (const row of rows) {
    const stamp = String(row.at || "");
    const date = stamp.slice(0, 10);
    if (date !== day) {
      day = date;
      out += `<div class="mono wiki-history-day" style="padding:10px 16px 4px;font-size:12px;color:var(--ink-3);letter-spacing:.06em">${esc(date)}</div>`;
    }
    out += `<div class="row wiki-change" style="display:flex;align-items:center;gap:12px;min-height:44px;padding:0 16px;border-bottom:1px solid var(--line)">
      <span class="mono" style="flex:none;font-size:12px;color:var(--ink-3)">${esc(stamp)}</span>
      <span style="flex:none;font-size:13px;color:var(--ink-2)">${esc(row.actor)}</span>
      <span class="mono" style="flex:1;min-width:0;font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(displayPath(row.path))}</span>
      <span style="flex:none;font-size:13px;color:var(--ink-2)">${esc(OP_WORDS[row.op] || row.op)}</span>
    </div>`;
  }
  return out;
}

function historyEarlier(id, data, loaded) {
  if (data.truncated && data.next_before) {
    const remaining = Math.max(0, (data.total ?? loaded) - loaded);
    return `<button type="button" class="btn-outline wiki-history-earlier" data-action="wiki-history-earlier" data-id="${esc(id)}" data-before="${esc(data.next_before)}" style="margin:12px 16px;height:32px;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--accent);font:600 13px/1 var(--font-sans);cursor:pointer">Earlier · ${remaining}</button>`;
  }
  return `<div class="mono wiki-history-done" style="padding:12px 16px;font-size:12px;color:var(--ink-3)">${loaded} change${loaded === 1 ? "" : "s"} · all loaded</div>`;
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
    ? `<div class="wiki-changes">${historyRows(rows)}${historyEarlier(id, data, rows.length)}</div>`
    : `<div class="wiki-empty" style="padding:24px 16px;max-width:640px"><div class="mono" style="font-size:12px;color:var(--ink-3);letter-spacing:.06em">wiki · recent changes</div><h2 style="font-size:17px;font-weight:600;margin:6px 0">No changes yet.</h2><p style="font-size:13px;color:var(--ink-2);line-height:1.45;margin:0">Every write to this wiki is logged here with who and when.</p></div>`;
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
    temp.innerHTML = historyRows(rows);
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
  const recheck = `<a class="btn-outline" href="#/projects/${encodeURIComponent(id)}/wiki?view=lint&fresh=1" style="flex:none;height:30px;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);color:var(--ink);text-decoration:none;font:600 13px/28px var(--font-sans)">Re-check</a>`;
  const body = findings.length
    ? findings
        .map(
          (finding) => `<div class="row wiki-finding" style="display:flex;flex-direction:column;gap:3px;padding:10px 16px;border-bottom:1px solid var(--line)">
        <span class="mono" style="font-size:12px;color:var(--ink-3)">${esc(finding.code)}</span>
        <span style="font-size:14px">${esc(finding.message)}</span>
        ${
          finding.path
            ? `<a class="mono" href="${wikiPageHash(id, displayPath(finding.path))}" style="font-size:12px;color:var(--accent);text-decoration:none">${esc(displayPath(finding.path))}</a>`
            : ""
        }
      </div>`,
        )
        .join("")
    : `<div class="wiki-empty" style="padding:24px 16px;max-width:640px"><div class="mono" style="font-size:12px;color:var(--ink-3);letter-spacing:.06em">wiki · lint</div><h2 style="font-size:17px;font-weight:600;margin:6px 0">Nothing to fix.</h2><p style="font-size:13px;color:var(--ink-2);line-height:1.45;margin:0">Every page declares a type, every link resolves, and every page is listed in its index.</p></div>`;
  return {
    head: shellStageHead("Lint", `${findings.length} finding${findings.length === 1 ? "" : "s"}`, recheck, `#/projects/${encodeURIComponent(id)}/wiki`),
    controls: `<div class="shell-controls" style="padding:0 16px"><span class="shell-meta mono">checked ${esc(data.checked_at || "")}</span></div>`,
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
          (entry) => `<a class="row wiki-review-row" href="${wikiPageHash(id, displayPath(entry.path))}" style="display:flex;align-items:center;gap:12px;min-height:48px;padding:0 16px;border-bottom:1px solid var(--line);color:var(--ink);text-decoration:none">
        <span style="flex:1;min-width:0;font-size:14px;font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(entry.title || displayPath(entry.path).split("/").pop())}</span>
        <span class="mono" style="flex:none;font-size:12px;color:var(--ink-3)">${esc(trustWords(entry))}</span>
      </a>`,
        )
        .join("")
    : `<p class="empty" style="padding:16px">Nothing needs review.</p>`;
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
    `<a class="row" href="#/projects/${encodeURIComponent(id)}/wiki?view=${view}" style="display:flex;align-items:center;justify-content:space-between;min-height:48px;padding:0 16px;border-bottom:1px solid var(--line);color:var(--ink);text-decoration:none">${label}</a>`;
  const body = `<div class="wiki-home">
    <div style="padding:16px 16px 0">
      <button type="button" class="primary" data-action="wiki-new" data-id="${esc(id)}" data-project="${esc(projectName || id)}" style="height:36px;padding:0 16px">New page</button>
    </div>
    <div style="padding:16px;display:flex;gap:24px">
      <div><div class="mono" style="font-size:12px;color:var(--ink-3)">PAGES</div><div style="font-size:22px;font-weight:600">${pages}</div></div>
      <div><div class="mono" style="font-size:12px;color:var(--ink-3)">NEEDS REVIEW</div><div style="font-size:22px;font-weight:600">${review}</div></div>
      <div><div class="mono" style="font-size:12px;color:var(--ink-3)">STALE</div><div style="font-size:22px;font-weight:600">${stale}</div></div>
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
  if (editing && selected) {
    const page = (await readPage(id, selected)) || { content: "", version: "absent" };
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

// Post a comment on the page. The body is the composer's text; an empty one
// sends nothing.
export async function wikiCommentAdd(button) {
  const id = button.dataset.id || "";
  const path = button.dataset.path || "";
  const body = (document.querySelector("[data-wiki-comment-body]")?.value || "").trim();
  if (!body) return;
  button.disabled = true;
  try {
    await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/comments`, {
      method: "POST",
      body: JSON.stringify({ path, body }),
    });
    render();
  } catch (error) {
    button.disabled = false;
    toast(error.message);
  }
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
    input.style.cssText = INPUT_CSS;
    wrap.append(span, input);
    form.appendChild(wrap);
    return input;
  };

  const defaultTitle = isNew ? "" : (fromPath.split("/").pop() || "").replace(/\.md$/, "");
  const dirInput = field("dir", isNew ? "Location" : "Location", isNew ? "" : (fromPath.split("/").slice(1, -1).join("/") || ""));
  const titleInput = field("title", "Title", defaultTitle);
  const descInput = field("description", "Description", "");

  const computed = document.createElement("p");
  computed.className = "dialog-note mono";
  computed.style.fontFamily = "var(--font-mono)";
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
      note.style.color = tone;
    }
  };
  if (!path) {
    say("A page needs a path.", "var(--danger)");
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
      note.style.color = "var(--ink-2)";
      note.textContent = "This page changed while you were editing. ";
      const action = (label, run) => {
        const el = document.createElement("button");
        el.type = "button";
        el.className = "btn-outline";
        el.textContent = label;
        el.style.cssText =
          "margin-left:8px;height:28px;padding:0 10px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 12px/1 var(--font-sans);cursor:pointer";
        el.addEventListener("click", run);
        note.appendChild(el);
      };
      action("Reload theirs", async () => {
        const page = await readPage(id, path);
        if (page) {
          document.getElementById("wiki-content").value = page.content || "";
          button.dataset.version = page.version || "absent";
        }
        note.textContent = "Reloaded the hub's copy.";
      });
      action("Keep mine", async () => {
        const page = await readPage(id, path);
        if (page) button.dataset.version = page.version || "absent";
        note.textContent = "";
        wikiSave(button);
      });
      return;
    }
    say(error.message, "var(--danger)");
  }
}
