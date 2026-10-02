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
import { esc } from "./dom.mjs";
import { read as readFrontmatter } from "./frontmatter.mjs";
import { render } from "./router.mjs";
import { renderMarkdown } from "./sessions.mjs";
import { toast } from "./toast.mjs";

const FILE_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/></svg>`;
const FOLDER_GLYPH = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h6l2 2h10v11H3z"/></svg>`;

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

function rowHTML(entry, id, selected) {
  const path = entry.path;
  const depth = depthOf(path);
  const pad = 16 + depth * 14;
  const name = entry.title || path.split("/").pop();
  const on = path === selected ? ' aria-current="true"' : "";
  if (entry.type === "dir") {
    return `<a class="row wiki-row wiki-dir" href="${wikiPageHash(id, path)}"${on} style="display:flex;align-items:center;gap:8px;min-height:44px;padding:0 12px 0 ${pad}px;border-bottom:1px solid var(--line);color:var(--ink);text-decoration:none;box-sizing:border-box">
      <span aria-hidden="true" style="flex:none;color:var(--ink-3);display:inline-flex">${FOLDER_GLYPH}</span>
      <span style="flex:1;min-width:0;font-size:14px;font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(path.split("/").pop())}</span>
      <span class="mono" style="flex:none;font-size:12px;color:var(--ink-3)">${entry.children ?? 0}</span>
    </a>`;
  }
  const meta = [entry.page_type || "concept", entry.status || "draft", trustWords(entry)].join(" · ");
  return `<a class="row wiki-row" href="${wikiPageHash(id, path)}"${on} style="display:flex;flex-direction:column;justify-content:center;gap:3px;min-height:56px;padding:6px 12px 6px ${pad}px;border-bottom:1px solid var(--line);color:var(--ink);text-decoration:none;box-sizing:border-box">
    <span style="display:flex;align-items:center;gap:8px;min-width:0">
      <span aria-hidden="true" style="flex:none;color:var(--ink-3);display:inline-flex">${FILE_GLYPH}</span>
      <span style="flex:1;min-width:0;font-size:14px;font-weight:500;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(name)}</span>
    </span>
    <span class="mono" style="padding-left:24px;font-size:12px;color:var(--ink-3);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(meta)}</span>
  </a>`;
}

export async function wikiIndexBody(id, selected) {
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/pages?meta=1`);
  } catch (error) {
    return `<p class="empty" style="padding:16px">Could not read the wiki: ${esc(error.message)}</p>`;
  }
  const entries = data.entries || [];
  if (!entries.length) {
    return `<p class="empty" style="padding:16px">No pages yet. A page arrives when an agent promotes one, or you write one here.</p>`;
  }
  return entries.map((entry) => rowHTML(entry, id, selected)).join("");
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
    controls: `<div class="shell-controls" style="padding:0 16px"><span class="shell-meta mono" style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(last)}</span></div>`,
    body: `<article class="shell-prose wiki-page" style="max-width:640px;padding:16px">${fm.description ? `<p class="wiki-description" style="font-size:15px;color:var(--ink-2);margin-top:0">${esc(fm.description)}</p>` : ""}${rendered}</article>${back}`,
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
async function changesStage(id, shellStageHead) {
  let data;
  try {
    data = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/history?limit=50`);
  } catch (error) {
    return simpleStage(shellStageHead, id, "Recent changes", "", `<div class="shell-pad"><p class="empty">${esc(error.message)}</p></div>`);
  }
  const rows = data.rows || [];
  const body = rows.length
    ? rows
        .map(
          (row) => `<div class="row wiki-change" style="display:flex;align-items:center;gap:12px;min-height:44px;padding:0 16px;border-bottom:1px solid var(--line)">
        <span class="mono" style="flex:none;font-size:12px;color:var(--ink-3)">${esc(row.at)}</span>
        <span style="flex:none;font-size:13px;color:var(--ink-2)">${esc(row.actor)}</span>
        <span class="mono" style="flex:1;min-width:0;font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(displayPath(row.path))}</span>
        <span style="flex:none;font-size:13px;color:var(--ink-2)">${esc(OP_WORDS[row.op] || row.op)}</span>
      </div>`,
        )
        .join("")
    : `<p class="empty" style="padding:16px">No changes yet.</p>`;
  return simpleStage(shellStageHead, id, "Recent changes", `${rows.length} change${rows.length === 1 ? "" : "s"}`, `<div class="wiki-changes">${body}</div>`);
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
    : `<p class="empty" style="padding:16px">No findings. The tree is consistent.</p>`;
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
async function homeStage(id, stats, shellStageHead) {
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

export async function wikiStage(id, params, shellStageHead, stats) {
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
  return homeStage(id, stats, shellStageHead);
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

// The New page control: the editor opens empty, and the path is typed there.
export function wikiNew(id) {
  location.hash = `#/projects/${encodeURIComponent(id)}/wiki?new=1`;
}

// Save a session brain entry into the wiki. The hub copies the entry and
// patches its frontmatter; the brain entry is left as it is. A page whose
// target already exists is refused unless the caller names the version, so the
// dialog says the path and lets the hub speak.
export function wikiPromote(button) {
  const id = button.dataset.id || "";
  const sessionId = button.dataset.session || "";
  const sessionName = button.dataset.sessionName || sessionId;
  const fromPath = button.dataset.path || "";
  const base = fromPath.split("/").pop() || "page.md";
  const defaultTo = base.endsWith(".md") ? base : `${base}.md`;

  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog";
  const titleId = "wiki-promote-title";
  el.setAttribute("aria-labelledby", titleId);

  const form = document.createElement("form");
  form.method = "dialog";

  const heading = document.createElement("h2");
  heading.className = "dialog-title";
  heading.id = titleId;
  heading.textContent = "Save to wiki";
  form.appendChild(heading);

  const body = document.createElement("p");
  body.className = "dialog-body";
  body.textContent = `Copies ${displayPath(fromPath)} from ${sessionName} into the project wiki. The brain entry is left as it is.`;
  form.appendChild(body);

  const makeField = (name, label, value) => {
    const wrap = document.createElement("label");
    wrap.className = "dialog-field-wrap";
    const text = document.createElement("span");
    text.className = "dialog-label";
    text.textContent = label;
    const input = document.createElement("input");
    input.name = name;
    input.value = value;
    input.autocomplete = "off";
    input.style.cssText =
      "height:36px;box-sizing:border-box;padding:0 10px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--surface);color:var(--ink);font:500 14px/1 var(--font-sans)";
    wrap.append(text, input);
    form.appendChild(wrap);
    return input;
  };
  const toInput = makeField("to_path", "Path", defaultTo);
  const typeInput = makeField("type", "Type", "concept");
  const titleInput = makeField("title", "Title", base.replace(/\.md$/, ""));

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
  commit.textContent = "Save to wiki";
  actions.append(safe, commit);
  form.appendChild(actions);

  safe.addEventListener("click", () => el.close("cancel"));
  commit.addEventListener("click", async () => {
    const toPath = toInput.value.trim();
    if (!toPath) {
      problem.textContent = "A page needs a path.";
      problem.hidden = false;
      toInput.focus();
      return;
    }
    commit.disabled = true;
    problem.hidden = true;
    try {
      const result = await api(`/api/v1/projects/${encodeURIComponent(id)}/kb/promote`, {
        method: "POST",
        body: JSON.stringify({
          from_session_id: sessionId,
          from_path: fromPath,
          to_path: toPath,
          type: typeInput.value.trim() || undefined,
          title: titleInput.value.trim() || undefined,
        }),
      });
      el.close("saved");
      toast("Saved to the wiki.");
      location.hash = wikiPageHash(id, displayPath(result.path || toPath));
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
  toInput.focus();
  toInput.select();

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

// Save from the editor. A create takes the path from the field; an edit takes
// it from the button. A page that changed under the reader comes back 409 and
// the editor stays as it is with the hub's reason said in place.
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
