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
      `<a class="btn-outline" href="${wikiPageHash(id, path, "&edit=1")}" style="flex:none;height:30px;padding:0 12px;border-radius:var(--r-1);border:1px solid var(--line-strong);color:var(--ink);text-decoration:none;font:600 13px/28px var(--font-sans)">Edit</a>`,
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
export async function wikiStage(id, params, shellStageHead) {
  const selected = params?.get("page") || "";
  const editing = params?.get("edit") === "1";
  const isNew = params?.get("new") === "1";
  if (isNew) return editorStage(id, "", "", "absent", true, shellStageHead);
  if (editing && selected) {
    const page = (await readPage(id, selected)) || { content: "", version: "absent" };
    return editorStage(id, selected, page.content || "", page.version || "absent", false, shellStageHead);
  }
  if (selected) return pageStage(id, selected, shellStageHead);
  return {
    head: shellStageHead("Wiki", "", "", `#/projects/${encodeURIComponent(id)}/feed`),
    controls: `<div class="shell-controls"><span class="shell-meta mono">${esc(id)} / wiki</span></div>`,
    body: `<div class="shell-pad"><p class="empty">Select a page from the tree, or write a new one.</p></div>`,
  };
}

// The version token a Save carries back: the read's token for an edit, the
// literal "absent" for a create, which the hub accepts only when nothing is
// there.
export async function saveWikiPage(id, path, content, version) {
  const body = version && version !== "absent" ? { content, if_version: version } : { content, if_version: "absent" };
  return api(wikiPageApi(id, path), { method: "PUT", body: JSON.stringify(body) });
}

// The New page control: the editor opens empty, and the path is typed there.
export function wikiNew(id) {
  location.hash = `#/projects/${encodeURIComponent(id)}/wiki?new=1`;
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
    say(error.message, "var(--danger)");
  }
}
