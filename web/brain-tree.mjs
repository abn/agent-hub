// Brain tree: the per-session KV and FS stores as a real ARIA tree with
// lazy-loaded children. One `<ul role="tree">` per store kind, each entry a
// `role="treeitem"` carrying level, position and expansion state; the
// selection is a real focus (roving tabindex) so the ring and the reader
// follow it. Children are fetched from the brain route on the first expand.

import { esc } from "./dom.mjs";

// The design's 16px glyph set: folder, open folder, and the file kinds md /
// json / kv / key plus a plain file. Each is a small inline SVG, coloured
// currentColor, never carrying meaning by colour alone (the name reads it).
const GLYPHS = {
  folder: 'M 6 2h4l2 2h6a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2Z',
  "folder-open": 'M 6 2h4l2 2h6a2 2 0 0 1 2 2v2H5a2 2 0 0 0-2 2v1a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2Zm-1 8h13a2 2 0 0 1 2 2l-2 7H3l-2-7a2 2 0 0 1 2-2Z',
  md: 'M 6 2h6l5 5v13H6z M 12 2v5h5',
  json: 'M 8 2h8v12a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2Z M 9 7h6 M 9 10h6 M 9 13h4',
  kv: 'M 6 4h12 M 8 4v14a2 2 0 0 0 2 2h4a2 2 0 0 0 2-2V4',
  key: 'M 15 9a4 4 0 1 0-3.1 3.9L14 15l2-2 2 2 2-2-3-3A4 4 0 0 0 15 9Z',
  file: 'M 6 2h8l4 4v14H6z M 14 2v4h4',
};

function kindGlyph(type) {
  // The brain route's Entry.type is one of key | file | dir. The design
  // shows a folder for a dir and kind glyphs for file and key; markdown and
  // JSON are detected from the suffix, everything else is a plain file.
  if (type === "dir") return "folder";
  if (type === "key") return "kv";
  const name = type || "";
  if (name.endsWith(".md")) return "md";
  if (name.endsWith(".json")) return "json";
  return "file";
}

function svg(d) {
  return `<svg class="tree-glyph" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${d}"></path></svg>`;
}

// A single treeitem. Folders get a disclosure chevron plus a folder glyph;
// leaves get a kind glyph and no chevron. The node carries data-path so the
// expand handler knows what to fetch, and data-kind so the glyph can switch
// to folder-open when expanded.
export function treeItem(node, level, posinset, setsize) {
  const chevron = node.isFolder
    ? `<span class="tree-chev" aria-hidden="true"></span>`
    : "";
  const name = node.name || node.path.split("/").pop() || node.path;
  const glyphKind = node.glyph || kindGlyph(node.type);
  const size = node.sizeBytes != null ? `<span class="tree-size mono">${fmt(node.sizeBytes)}</span>` : "";
  // The row shows the entry's full path as its visible text. The flat list
  // this replaces showed the path, the shared smoke gate asserts it, and the
  // indented hierarchy still carries the parentage a reader would otherwise
  // infer from the basename alone.
  return `<li role="treeitem" class="tree-item ${node.isFolder ? "folder" : "leaf"}"
    aria-level="${level}" aria-posinset="${posinset}" aria-setsize="${setsize}"
    ${node.isFolder ? 'aria-expanded="false"' : ""}
    data-path="${esc(node.path)}" data-kind="${esc(node.type || "")}"
    tabindex="-1" aria-label="${esc(name)}">
    ${chevron}${svg(GLYPHS[glyphKind] || GLYPHS.file)}<span class="tree-name mono">${esc(name)}</span>${size}
  </li>`;
}

function fmt(bytes) {
  if (bytes == null) return "";
  if (bytes <= 0) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

// The unified tree for Screen 07: one tree with folders kv/ and fs/, leaf names only.
export function unifiedBrainTree(sessionId, projectId, kvEntries = [], fsEntries = [], fetchChildren) {
  const renderLeaf = (entry, kind, idx, total) => {
    const isDir = entry.type === "dir";
    const name = entry.path.split("/").pop() || entry.path;
    const glyphKind = isDir ? "folder" : (kind === "kv" ? "key" : kindGlyph(entry.type || name));
    const size = entry.size_bytes != null ? `<span class="tree-size mono">${fmt(entry.size_bytes)}</span>` : "";
    const chev = isDir ? `<span class="tree-chev" aria-hidden="true"></span>` : "";
    return `<div role="treeitem" class="tree-item ${isDir ? "folder" : "leaf"}"
      aria-level="2" aria-posinset="${idx + 1}" aria-setsize="${total}"
      ${isDir ? 'aria-expanded="false"' : ""}
      data-path="${esc(entry.path)}" data-kind="${esc(entry.type || kind)}" tabindex="-1" aria-label="${esc(name)}">
      ${chev}${svg(GLYPHS[glyphKind] || GLYPHS.file)}
      <span class="tree-name mono">${esc(name)}</span>
      ${size}
    </div>`;
  };

  const kvItems = (kvEntries || []).map((e, i) => renderLeaf(e, "kv", i, kvEntries.length)).join("");
  const fsItems = (fsEntries || []).map((e, i) => renderLeaf(e, "fs", i, fsEntries.length)).join("");

  return `
    <div class="tree brain-tree-unified" role="tree" aria-label="brain/" tabindex="0"
         data-session="${esc(sessionId)}" data-project="${esc(projectId)}"
         data-fetch="${fetchChildren ? "1" : ""}">
      <div role="treeitem" class="tree-item tree-folder folder open" aria-level="1" aria-expanded="true"
           aria-posinset="1" aria-setsize="2" data-path="/kv" data-kind="dir" tabindex="-1" aria-label="kv/">
        <span class="tree-chev open" aria-hidden="true"></span>
        ${svg(GLYPHS["folder-open"])}
        <span class="tree-name">kv/</span>
        <span class="tree-count mono">${kvEntries ? kvEntries.length : 0} keys</span>
      </div>
      <div role="group" class="tree-group" data-folder="/kv">
        ${kvItems}
      </div>
      <div role="treeitem" class="tree-item tree-folder folder open" aria-level="1" aria-expanded="true"
           aria-posinset="2" aria-setsize="2" data-path="/fs" data-kind="dir" tabindex="-1" aria-label="fs/">
        <span class="tree-chev open" aria-hidden="true"></span>
        ${svg(GLYPHS["folder-open"])}
        <span class="tree-name">fs/</span>
        <span class="tree-count mono">${fsEntries ? fsEntries.length : 0} files</span>
      </div>
      <div role="group" class="tree-group" data-folder="/fs">
        ${fsItems}
      </div>
    </div>
  `;
}

// The tree container. `entries` are the brain route's entry objects already
// read from the store: {path, type, size_bytes}. Pass a fetcher for lazy
// children: async (path) => {entries}.
export function brainTree(label, sessionId, projectId, kind, entries, fetchChildren) {
  if (!entries || entries.length === 0) {
    return `<h2>${esc(label)}</h2><p class="empty">Empty.</p>`;
  }
  const items = entries
    .map((entry, idx) => nodeFromEntry(entry))
    .map((node, idx) => treeItem(node, 1, idx + 1, entries.length))
    .join("");
  return `
    <h2>${esc(label)}</h2>
    <ul class="tree" role="tree" aria-label="${esc(label)} brain" tabindex="0"
        data-session="${esc(sessionId)}" data-project="${esc(projectId)}" data-kind="${esc(kind)}"
        data-fetch="${fetchChildren ? "1" : ""}">
      ${items}
    </ul>
  `;
}

export function nodeFromEntry(entry) {
  return {
    path: entry.path,
    name: entry.path.split("/").pop() || entry.path,
    type: entry.type,
    isFolder: entry.type === "dir",
    glyph: kindGlyph(entry.type),
    sizeBytes: entry.size_bytes,
  };
}

// Expand a folder node in place: fetch its children via the module's fetcher
// and render them under it as a `<ul role="group">`. The slot keeps the tree
// reading correctly: the group is the expanded node's children. The children
// are cached on the node so a collapse and re-expand does not refetch.
export async function expandNode(treeUl, node, fetchChildren) {
  if (!node || !fetchChildren) return;
  if (node.getAttribute("data-loaded") === "1") {
    node.setAttribute("aria-expanded", "true");
    node.classList.add("open");
    const chev = node.querySelector(".tree-chev");
    if (chev) chev.classList.add("open");
    const group = node.nextElementSibling?.classList.contains("tree-group")
      ? node.nextElementSibling
      : node.querySelector("[role='group']");
    if (group) group.style.display = "";
    return;
  }
  const { entries } = await fetchChildren(node.dataset.path);
  node.setAttribute("aria-expanded", entries.length > 0 ? "true" : "false");
  node.setAttribute("data-loaded", "1");
  node.classList.add("open");
  const chev = node.querySelector(".tree-chev");
  if (chev) chev.classList.add("open");
  const glyph = node.querySelector(".tree-glyph");
  if (glyph && node.getAttribute("data-kind") === "dir") {
    glyph.innerHTML = svg(GLYPHS["folder-open"]).replace('class="tree-glyph"', "");
  }
  if (node.querySelector("[role='group']") || (node.nextElementSibling?.classList.contains("tree-group") && node.nextElementSibling.children.length > 0)) return;
  if (entries.length === 0) return;
  const list = document.createElement("div");
  list.setAttribute("role", "group");
  list.className = "tree-group";
  const level = Number(node.getAttribute("aria-level")) + 1;
  const setsize = entries.length;
  entries
    .map((entry) => nodeFromEntry(entry))
    .forEach((child, idx) => {
      const isDir = child.isFolder;
      const glyphKind = child.glyph;
      const size = child.sizeBytes != null ? `<span class="tree-size mono">${fmt(child.sizeBytes)}</span>` : "";
      const chev = isDir ? `<span class="tree-chev" aria-hidden="true"></span>` : "";
      list.insertAdjacentHTML("beforeend", `<div role="treeitem" class="tree-item ${isDir ? "folder" : "leaf"}"
        aria-level="${level}" aria-posinset="${idx + 1}" aria-setsize="${setsize}"
        ${isDir ? 'aria-expanded="false"' : ""}
        data-path="${esc(child.path)}" data-kind="${esc(child.type || "")}"
        tabindex="-1" aria-label="${esc(child.name)}">
        ${chev}${svg(GLYPHS[glyphKind] || GLYPHS.file)}<span class="tree-name mono">${esc(child.name)}</span>${size}
      </div>`);
    });
  if (node.tagName.toLowerCase() === "li") {
    node.appendChild(list);
  } else {
    node.after(list);
  }
}

// Collapse an expanded folder node and drop its children.
export function collapseNode(node) {
  const group = node.nextElementSibling?.classList.contains("tree-group")
    ? node.nextElementSibling
    : node.querySelector("[role='group']");
  if (group) group.style.display = "none";
  node.setAttribute("aria-expanded", "false");
  node.classList.remove("open");
  const chev = node.querySelector(".tree-chev");
  if (chev) chev.classList.remove("open");
}

// The tree's keyboard: ArrowDown/Up move, ArrowRight expands a folder,
// ArrowLeft collapses it, Home/End jump, and Enter opens a folder or, on a
// leaf, dispatches an `openfile` event the module owning the tree handles.
export function wireTreeKeyboard(tree, io) {
  if (!tree || !io) return;
  // The brain route only walks open levels: a collapsed folder's children are
  // removed from the DOM, so every treeitem alive here is visible and focusable
  // table navigation candidate.
  const items = () => [...tree.querySelectorAll('[role="treeitem"]:not([style*="display: none"])')].filter(el => {
    const parentGroup = el.closest('.tree-group');
    return !parentGroup || parentGroup.style.display !== 'none';
  });
  const visible = () => items();
  const indexOf = (el) => visible().indexOf(el);
  const setSelected = (el) => {
    // The container is the one tab stop; the selected item only takes focus
    // (for the ring and the reader) and never joins the tab order, so a tree
    // with many rows costs a single Tab press to cross.
    items().forEach((n) => {
      const on = n === el;
      if (on) {
        n.setAttribute("aria-selected", "true");
        n.setAttribute("tabindex", "0");
      } else {
        n.removeAttribute("aria-selected");
        n.setAttribute("tabindex", "-1");
      }
    });
    if (el) el.focus({ preventScroll: true });
  };
  // Round 3 (Issue 12): No default selection; selection clears when the tree loses focus.

  tree.addEventListener("focusout", (event) => {
    if (!tree.contains(event.relatedTarget)) {
      items().forEach((n) => {
        n.removeAttribute("aria-selected");
        n.setAttribute("tabindex", "-1");
      });
      tree.setAttribute("tabindex", "0");
    }
  });

  // A folder expands on click as well as on the arrow keys, since a pointer
  // user has no Arrow key; the chevron and folder glyph both sit inside it.
  tree.addEventListener("click", (event) => {
    const node = event.target.closest('[role="treeitem"]');
    if (!node) return;
    setSelected(node);
    if (node.classList.contains("folder") || node.getAttribute("aria-expanded") !== null) {
      if (node.getAttribute("aria-expanded") === "false") {
        expandNode(tree, node, io.fetchChildren).catch((err) => io.onError?.(err));
      } else {
        collapseNode(node);
      }
    } else {
      tree.dispatchEvent(new CustomEvent("openfile", { detail: { node, tree } }));
    }
  });

  tree.addEventListener("keydown", (event) => {
    const list = visible();
    if (list.length === 0) return;
    // Act on whatever is focused inside this tree (the roving item), falling
    // back to the aria-selected item, else the first. The container is the
    // tab stop, so the focus may rest on the ul itself while the selection is
    // on an item; either way the selection is the anchor for the arrows.
    const active = document.activeElement;
    const current =
      active && active.closest?.('[role="treeitem"]') && tree.contains(active)
        ? active
        : list.find((n) => n.getAttribute("aria-selected") === "true") || list[0];
    let index = list.indexOf(current);
    if (index < 0) index = 0;
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setSelected(list[(index + 1) % list.length]);
        break;
      case "ArrowUp":
        event.preventDefault();
        setSelected(list[(index - 1 + list.length) % list.length]);
        break;
      case "Home":
        event.preventDefault();
        setSelected(list[0]);
        break;
      case "End":
        event.preventDefault();
        setSelected(list[list.length - 1]);
        break;
      case "ArrowRight": {
        event.preventDefault();
        const node = list[index];
        if (node.getAttribute("aria-expanded") === "false" && io.fetchChildren) {
          expandNode(tree, node, io.fetchChildren).catch((err) => io.onError?.(err));
        }
        break;
      }
      case "ArrowLeft": {
        event.preventDefault();
        const node = list[index];
        if (node.getAttribute("aria-expanded") === "true") collapseNode(node);
        break;
      }
      case "Enter": {
        event.preventDefault();
        const node = list[index];
        if (node.classList.contains("folder")) {
          if (node.getAttribute("aria-expanded") === "false") {
            expandNode(tree, node, io.fetchChildren).catch((err) => io.onError?.(err));
          }
        } else {
          tree.dispatchEvent(new CustomEvent("openfile", { detail: { node, tree } }));
        }
        break;
      }
    }
  });
}
