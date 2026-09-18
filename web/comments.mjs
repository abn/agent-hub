// The comments drawer on the artifact viewer. The PWA composes plain
// comments only; anchored comments posted elsewhere render marker text.
//
// Everything here is built with createElement and textContent, so no comment
// body ever reaches innerHTML.

import { api } from "./api.mjs";
import { stamp } from "./time.mjs";

const commentsDrawer = {
  artifactId: null,
  comments: [],
  open: false,
  lastFocus: null,
  elements: null,
  seq: 0,
};

function commentsBase(id) {
  return `/api/v1/artifacts/${encodeURIComponent(id)}/comments`;
}

// Marker text for an anchored comment. v1 renders no canvas pins and no
// quote selection, only this line.
function anchorMarker(comment) {
  const anchor = comment.anchor;
  if (!anchor || typeof anchor !== "object") return "";
  if (anchor.mode === "point") return "Pinned";
  if (anchor.mode === "text" && typeof anchor.quote === "string" && anchor.quote) {
    const quote = anchor.quote.length > 80 ? anchor.quote.slice(0, 80) + "..." : anchor.quote;
    return `Quoted: ${quote}`;
  }
  return "";
}

function drawerError(message) {
  const line = commentsDrawer.elements && commentsDrawer.elements.error;
  if (!line) return;
  line.hidden = false;
  line.textContent = message;
}

function clearDrawerError() {
  const line = commentsDrawer.elements && commentsDrawer.elements.error;
  if (!line) return;
  line.hidden = true;
  line.textContent = "";
}

function refreshCommentsToggle() {
  const els = commentsDrawer.elements;
  if (!els) return;
  const open = commentsDrawer.comments.filter((comment) => !comment.done).length;
  els.badge.hidden = open === 0;
  els.badge.textContent = String(open);
}

function commentRow(comment) {
  const item = document.createElement("div");
  item.className = "comment" + (comment.done ? " done" : "");
  const head = document.createElement("div");
  head.className = "comment-head";
  const author = document.createElement("span");
  author.className = "comment-author";
  author.textContent = comment.author;
  const time = document.createElement("span");
  time.className = "meta";
  time.textContent = stamp(comment.created_at);
  head.append(author, time);
  const marker = anchorMarker(comment);
  if (marker) {
    const anchor = document.createElement("span");
    anchor.className = "comment-anchor";
    anchor.textContent = marker;
    head.appendChild(anchor);
  }
  if (comment.done) {
    const pill = document.createElement("span");
    pill.className = "pill";
    pill.textContent = "Done";
    head.appendChild(pill);
  }
  const body = document.createElement("p");
  body.className = "comment-body";
  body.textContent = comment.body;
  const actions = document.createElement("div");
  actions.className = "comment-actions";
  const resolve = document.createElement("button");
  resolve.type = "button";
  resolve.textContent = comment.done ? "Reopen" : "Resolve";
  resolve.setAttribute("aria-label", `${comment.done ? "Reopen" : "Resolve"} comment by ${comment.author}`);
  resolve.addEventListener("click", () => toggleCommentDone(comment));
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "danger";
  remove.textContent = "Delete";
  remove.setAttribute("aria-label", `Delete comment by ${comment.author}`);
  remove.addEventListener("click", () => deleteComment(comment));
  actions.append(resolve, remove);
  item.append(head, body, actions);
  return item;
}

function renderComments() {
  const els = commentsDrawer.elements;
  if (!els) return;
  els.list.innerHTML = "";
  if (!commentsDrawer.comments.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "No comments yet. Be the first to leave one.";
    els.list.appendChild(empty);
  } else {
    for (const comment of commentsDrawer.comments) els.list.appendChild(commentRow(comment));
  }
  refreshCommentsToggle();
}

async function loadComments() {
  const id = commentsDrawer.artifactId;
  if (!id || !commentsDrawer.elements) return;
  const seq = commentsDrawer.seq;
  try {
    const data = await api(commentsBase(id));
    if (seq !== commentsDrawer.seq) return;
    commentsDrawer.comments = data.comments || [];
    clearDrawerError();
    renderComments();
  } catch (error) {
    if (seq !== commentsDrawer.seq) return;
    drawerError(error.message);
  }
}

async function postComment(body) {
  const id = commentsDrawer.artifactId;
  const created = await api(commentsBase(id), {
    method: "POST",
    body: JSON.stringify({ body }),
  });
  commentsDrawer.comments.push(created);
  renderComments();
}

async function toggleCommentDone(comment) {
  try {
    const updated = await api(
      `${commentsBase(commentsDrawer.artifactId)}/${encodeURIComponent(comment.id)}`,
      { method: "PATCH", body: JSON.stringify({ done: !comment.done }) },
    );
    const index = commentsDrawer.comments.findIndex((item) => item.id === comment.id);
    if (index >= 0) commentsDrawer.comments[index] = updated;
    clearDrawerError();
    renderComments();
  } catch (error) {
    drawerError(error.message);
  }
}

async function deleteComment(comment) {
  if (!window.confirm("Delete this comment? This cannot be undone.")) return;
  try {
    await api(`${commentsBase(commentsDrawer.artifactId)}/${encodeURIComponent(comment.id)}`, {
      method: "DELETE",
    });
    commentsDrawer.comments = commentsDrawer.comments.filter((item) => item.id !== comment.id);
    clearDrawerError();
    renderComments();
  } catch (error) {
    drawerError(error.message);
  }
}

function openCommentsDrawer() {
  const els = commentsDrawer.elements;
  if (!els) return;
  commentsDrawer.lastFocus = document.activeElement;
  commentsDrawer.open = true;
  clearDrawerError();
  els.backdrop.hidden = false;
  els.drawer.hidden = false;
  els.toggle.setAttribute("aria-expanded", "true");
  els.close.focus();
  loadComments();
}

function closeCommentsDrawer() {
  const els = commentsDrawer.elements;
  commentsDrawer.open = false;
  if (!els) return;
  els.backdrop.hidden = true;
  els.drawer.hidden = true;
  els.toggle.setAttribute("aria-expanded", "false");
  if (commentsDrawer.lastFocus && document.contains(commentsDrawer.lastFocus)) {
    commentsDrawer.lastFocus.focus();
  }
  commentsDrawer.lastFocus = null;
}

// Point the drawer at another artifact. The sequence number invalidates any
// load still in flight for the artifact just left.
export function startComments(id) {
  commentsDrawer.artifactId = id;
  commentsDrawer.comments = [];
  commentsDrawer.open = false;
  commentsDrawer.lastFocus = null;
  commentsDrawer.elements = null;
  commentsDrawer.seq += 1;
}

// The control that opens the drawer, for the viewer's toolbar.
export function commentsToggle() {
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "comments-toggle";
  toggle.setAttribute("aria-haspopup", "dialog");
  toggle.setAttribute("aria-expanded", "false");
  const toggleLabel = document.createElement("span");
  toggleLabel.textContent = "Comments";
  const badge = document.createElement("span");
  badge.className = "comments-badge";
  badge.hidden = true;
  toggle.append(toggleLabel, document.createTextNode(" "), badge);
  toggle.addEventListener("click", () => {
    if (commentsDrawer.open) closeCommentsDrawer();
    else openCommentsDrawer();
  });
  return { toggle, badge };
}

// The backdrop and the drawer itself, ready to append. The caller owns where
// they land in the document.
export function commentsPanel({ toggle, badge }) {
  const backdrop = document.createElement("div");
  backdrop.className = "drawer-backdrop";
  backdrop.hidden = true;
  backdrop.addEventListener("click", closeCommentsDrawer);
  const drawer = document.createElement("aside");
  drawer.className = "comments-drawer";
  drawer.setAttribute("role", "dialog");
  drawer.setAttribute("aria-modal", "true");
  drawer.setAttribute("aria-labelledby", "comments-heading");
  drawer.hidden = true;
  const head = document.createElement("div");
  head.className = "comments-drawer-head";
  const heading = document.createElement("h2");
  heading.id = "comments-heading";
  heading.textContent = "Comments";
  const close = document.createElement("button");
  close.type = "button";
  close.textContent = "Close";
  close.setAttribute("aria-label", "Close comments");
  close.addEventListener("click", closeCommentsDrawer);
  head.append(heading, close);
  const error = document.createElement("p");
  error.className = "drawer-error";
  error.setAttribute("role", "alert");
  error.hidden = true;
  const list = document.createElement("div");
  list.className = "comments-list";
  list.setAttribute("role", "log");
  list.setAttribute("aria-label", "Comments");
  const form = document.createElement("form");
  form.className = "comments-compose";
  const label = document.createElement("label");
  label.setAttribute("for", "comment-body");
  label.textContent = "New comment";
  const box = document.createElement("textarea");
  box.id = "comment-body";
  box.name = "body";
  box.rows = 3;
  box.maxLength = 2000;
  box.placeholder = "Write a comment";
  const composeError = document.createElement("p");
  composeError.className = "drawer-error";
  composeError.setAttribute("role", "alert");
  composeError.hidden = true;
  const post = document.createElement("button");
  post.type = "submit";
  post.className = "primary";
  post.textContent = "Post";
  form.append(label, box, composeError, post);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const body = box.value.trim();
    if (!body) {
      composeError.hidden = false;
      composeError.textContent = "Write a comment before posting.";
      box.focus();
      return;
    }
    composeError.hidden = true;
    composeError.textContent = "";
    post.disabled = true;
    postComment(body)
      .then(() => {
        box.value = "";
      })
      .catch((err) => {
        composeError.hidden = false;
        composeError.textContent = err.message;
      })
      .finally(() => {
        post.disabled = false;
      });
  });
  drawer.append(head, error, list, form);
  drawer.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeCommentsDrawer();
      return;
    }
    // The drawer promises a modal, so Tab cycles inside it instead of
    // leaving for the page behind.
    if (event.key !== "Tab") return;
    const focusable = Array.from(
      drawer.querySelectorAll(
        'button, textarea, select, input, a[href], [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((el) => !el.disabled);
    if (!focusable.length) {
      event.preventDefault();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });
  commentsDrawer.elements = { backdrop, drawer, toggle, badge, close, list, error };
  // The first list arrives a tick later, by which time the caller has put the
  // drawer in the document.
  loadComments();
  return { backdrop, drawer };
}
