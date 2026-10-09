// Discussion and document comments on artifacts.
// Highlights in text, phone sheet for threads, list drawer, and desktop margin cards.
//
// All user content is built via createElement and textContent or esc(),
// so no comment body or quote ever executes as markup.

import { api } from "./api.mjs";
import { composer } from "./composer.mjs";
import { confirmAction } from "./dialog.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { timeNode } from "./time.mjs";
import { relative } from "./time.mjs";

export const commentsState = {
  artifactId: null,
  shownVersion: 1,
  isProtected: false,
  comments: [],
  open: false,
  activeThreadId: null,
  composeQuote: null,
  viewMode: "list", // "list" | "sheet" | "compose"
  resolvedExpanded: false,
  lastFocus: null,
  elements: null,
  desktopContainer: null,
  seq: 0,
};

function commentsBase(id) {
  return `/api/v1/artifacts/${encodeURIComponent(id)}/comments`;
}

// Marker text for an anchored comment.
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
  const line = commentsState.elements && commentsState.elements.error;
  if (!line) return;
  line.hidden = false;
  line.textContent = message;
}

function clearDrawerError() {
  const line = commentsState.elements && commentsState.elements.error;
  if (!line) return;
  line.hidden = true;
  line.textContent = "";
}

export function sendCommentsToFrame() {
  const frame = document.querySelector("#hub-frame");
  if (!frame || !frame.contentWindow) return;
  try {
    frame.contentWindow.postMessage(
      {
        type: "hub:set-comments",
        comments: commentsState.comments,
        shownVersion: commentsState.shownVersion,
        protected: commentsState.isProtected,
      },
      "*",
    );
  } catch (_) {}
}

export function refreshCommentsToggle() {
  const els = commentsState.elements;
  const openCount = commentsState.comments.filter((c) => !c.done).length;
  if (els && els.badge) {
    els.badge.hidden = openCount === 0;
    els.badge.textContent = String(openCount);
  }

  // Update top bar comments button count
  const barBtn = document.querySelector(".hub-comments-btn .hub-glyph-count");
  if (barBtn) {
    barBtn.textContent = String(openCount);
  }

  // Update strip on mobile
  const stripCount = document.querySelector(".hub-comments-count");
  if (stripCount) {
    stripCount.textContent = String(openCount);
  }

  // The comments panel's own header carries the same open count, so the three
  // surfaces agree. It keeps the version it was built with, which is why the
  // count is replaced and the rest of the line is left alone.
  const headMeta = document.querySelector(".hub-comments-head-meta");
  if (headMeta && headMeta.dataset.version) {
    headMeta.textContent = `${openCount} · v${headMeta.dataset.version}`;
  }
}

export function authorNode(authorText) {
  const isHuman = authorText === "human" || authorText === "you";
  const span = document.createElement("span");
  span.className = "comment-author" + (isHuman ? " human" : " agent mono");
  if (!isHuman) span.dataset.agent = "true";
  span.textContent = isHuman ? "you" : authorText;
  return span;
}

function quoteBlockNode(comment, shownVersion) {
  const anchor = comment && comment.anchor;
  if (!anchor || typeof anchor !== "object") return null;

  const block = document.createElement("div");
  const isThisVersion = comment.anchor_version === shownVersion;
  const isDone = comment.done;

  if (isDone) {
    block.className = "hub-quote-block hairline";
  } else if (isThisVersion) {
    block.className = "hub-quote-block tinted";
  } else {
    block.className = "hub-quote-block outlined";
  }

  const quoteRow = document.createElement("div");
  quoteRow.className = "hub-quote-row";
  const anchorIcon = document.createElement("span");
  anchorIcon.className = "hub-quote-icon";
  anchorIcon.setAttribute("aria-hidden", "true");
  anchorIcon.innerHTML = glyphSvg("anchorPin", { size: 14 });
  const quoteText = document.createElement("div");
  quoteText.className = "hub-quote-text mono";
  if (anchor.mode === "text" && anchor.quote) {
    quoteText.textContent = `“${anchor.quote}”`;
  } else if (anchor.mode === "point") {
    quoteText.textContent = "Pinned";
  }
  quoteRow.append(anchorIcon, quoteText);
  block.appendChild(quoteRow);

  const stamp = document.createElement("div");
  stamp.className = "hub-quote-stamp mono";
  if (isDone) {
    stamp.textContent = `v${comment.anchor_version || shownVersion}`;
  } else if (isThisVersion) {
    stamp.textContent = `v${comment.anchor_version} · the version you are reading`;
  } else {
    stamp.textContent = `v${comment.anchor_version} · older version`;
  }
  block.appendChild(stamp);

  return block;
}

export async function loadComments() {
  const id = commentsState.artifactId;
  if (!id) return;
  const seq = commentsState.seq;
  try {
    const data = await api(commentsBase(id));
    if (seq !== commentsState.seq) return;
    commentsState.comments = data.comments || [];
    clearDrawerError();
    renderComments();
    renderDesktopCards();
    sendCommentsToFrame();
  } catch (error) {
    if (seq !== commentsState.seq) return;
    drawerError(error.message);
  }
}

// The hub caps a text anchor at 2000 characters and 2048 bytes, and a reader can
// select far more than that. Anchor the first slice, capped by bytes, rather
// than refusing the comment: the quote is a pointer into the document, not the
// whole passage, and the frame matches the prefix.
export function anchorQuote(text) {
  const encoder = new TextEncoder();
  let out = "";
  for (const ch of text) {
    if (encoder.encode(out + ch).length > 1500) break;
    out += ch;
  }
  return out;
}

export async function postComment(body, quote) {
  const id = commentsState.artifactId;
  const payload = { body };
  if (quote && !commentsState.isProtected) {
    payload.anchor = { mode: "text", quote: anchorQuote(quote) };
    payload.anchor_version = commentsState.shownVersion;
  }
  const created = await api(commentsBase(id), {
    method: "POST",
    body: JSON.stringify(payload),
  });
  commentsState.comments.push(created);
  renderComments();
  renderDesktopCards();
  sendCommentsToFrame();
  return created;
}

export async function toggleCommentDone(comment) {
  try {
    const updated = await api(
      `${commentsBase(commentsState.artifactId)}/${encodeURIComponent(comment.id)}`,
      { method: "PATCH", body: JSON.stringify({ done: !comment.done }) },
    );
    const index = commentsState.comments.findIndex((item) => item.id === comment.id);
    if (index >= 0) commentsState.comments[index] = updated;
    clearDrawerError();
    renderComments();
    renderDesktopCards();
    sendCommentsToFrame();
    return updated;
  } catch (error) {
    drawerError(error.message);
  }
}

export async function deleteComment(comment) {
  const confirmed = await confirmAction({
    title: "Delete this comment?",
    body: `It is removed for everyone who can read the artifact. Written by ${comment.author}.`,
    note: "Deleting a comment cannot be undone.",
    safe: "Keep",
    danger: "Delete comment",
  });
  if (!confirmed) return;
  try {
    await api(`${commentsBase(commentsState.artifactId)}/${encodeURIComponent(comment.id)}`, {
      method: "DELETE",
    });
    commentsState.comments = commentsState.comments.filter((item) => item.id !== comment.id);
    clearDrawerError();
    renderComments();
    renderDesktopCards();
    sendCommentsToFrame();
    const els = commentsState.elements;
    if (els) {
      const next = els.list.querySelector(".comment-actions button");
      (next || els.compose).focus();
    }
  } catch (error) {
    drawerError(error.message);
  }
}

// A comment is written in the one composer every screen uses. Here it sits in
// a wrapper that places it in the drawer or at the foot of a thread sheet.
// Comments carry the limit they always have.
const COMMENT_CHARS_MAX = 2000;

function commentComposer({ className, label, placeholder, send }) {
  const wrap = document.createElement("div");
  wrap.className = className;
  const made = composer({
    label,
    placeholder,
    action: "Post",
    empty: "Write a comment to post it.",
    maxLength: COMMENT_CHARS_MAX,
    send,
  });
  wrap.appendChild(made.element);
  return { wrap, field: made.field, focus: made.focus };
}

function commentRow(comment) {
  const item = document.createElement("div");
  item.className = "comment" + (comment.done ? " done" : "");
  item.dataset.commentId = String(comment.id);

  const head = document.createElement("div");
  head.className = "comment-head";
  head.append(authorNode(comment.author), timeNode(comment.created_at));

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

  // Quote block (3 appearances: tinted, outlined, hairline)
  const quoteEl = quoteBlockNode(comment, commentsState.shownVersion);
  if (quoteEl) {
    item.appendChild(quoteEl);
  }

  const body = document.createElement("p");
  body.className = "comment-body";
  body.textContent = comment.body;

  const actions = document.createElement("div");
  actions.className = "comment-actions";

  const resolve = document.createElement("button");
  resolve.type = "button";
  resolve.textContent = comment.done ? "Reopen" : "Resolve";
  resolve.setAttribute(
    "aria-label",
    `${comment.done ? "Reopen" : "Resolve"} comment by ${comment.author}`,
  );
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

// Render the drawer contents (list view, sheet view, or compose view)
export function renderComments() {
  const els = commentsState.elements;
  if (!els) return;

  refreshCommentsToggle();

  // The drawer's own compose form sits below the list and is part of its
  // fixed structure, so it was on screen in every view, including the two
  // that build a composer of their own. That put two textareas and two Post
  // buttons in front of the reader with no way to tell which one was about
  // the sentence they had selected. It belongs to the list and shows there.
  if (els.composeForm) {
    els.composeForm.hidden = commentsState.viewMode !== "list";
  }

  if (commentsState.viewMode === "sheet" && commentsState.activeThreadId) {
    renderPhoneSheet();
    return;
  }

  if (commentsState.viewMode === "compose") {
    renderComposeSheet();
    return;
  }

  // List view (Screen 06)
  renderListView();
}

function renderListView() {
  const els = commentsState.elements;
  if (!els) return;

  els.drawer.className = "comments-drawer hub-drawer-list";
  els.list.innerHTML = "";
  els.foot?.replaceChildren();

  // The drawer's own head names the list it holds. The two sheet views build a
  // head of their own, so this one is hidden while they are mounted rather than
  // stacked above them, which put two titles and two close buttons in one
  // sheet.
  if (els.head) els.head.hidden = false;

  const heading = els.drawer.querySelector(".comments-drawer-head h2");
  if (heading) heading.textContent = "Comments";

  if (!commentsState.comments.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "No comments yet. Be the first to leave one.";
    els.list.appendChild(empty);
    return;
  }

  const openComments = commentsState.comments
    .filter((c) => !c.done)
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  const resolvedComments = commentsState.comments
    .filter((c) => c.done)
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));

  // Render open comments
  for (const comment of openComments) {
    const row = renderListRow(comment);
    els.list.appendChild(row);
  }

  // Resolved section footer (Screen 06)
  if (resolvedComments.length > 0) {
    const resolvedBar = document.createElement("button");
    resolvedBar.type = "button";
    resolvedBar.className = "hub-resolved-toggle";
    resolvedBar.setAttribute("aria-expanded", String(commentsState.resolvedExpanded));
    resolvedBar.innerHTML = `
      ${glyphSvg("resolve", { size: 16 })}
      <span class="hub-resolved-title grow" style="text-align:left">${resolvedComments.length} resolved</span>
      ${glyphSvg("chevronDown", { size: 16 })}
    `;
    resolvedBar.addEventListener("click", () => {
      commentsState.resolvedExpanded = !commentsState.resolvedExpanded;
      renderComments();
    });
    els.list.appendChild(resolvedBar);

    if (commentsState.resolvedExpanded) {
      const resContainer = document.createElement("div");
      resContainer.className = "hub-resolved-list";
      for (const comment of resolvedComments) {
        resContainer.appendChild(renderListRow(comment));
      }
      els.list.appendChild(resContainer);
    }
  }

  // Helper note
  const helper = document.createElement("div");
  helper.className = "hub-comments-helper";
  helper.textContent =
    "A comment carries the version it was written against. Highlighted here and in the text means it belongs to the version you are reading; a comment stamped with an older version opens that version to show its quote in place.";
  els.list.appendChild(helper);
}

function renderListRow(comment) {
  const row = document.createElement("div");
  row.className = "hub-list-comment" + (comment.done ? " done" : "");
  row.dataset.commentId = String(comment.id);

  // Quote block
  if (comment.anchor) {
    const qb = quoteBlockNode(comment, commentsState.shownVersion);
    if (qb) row.appendChild(qb);
  }

  const body = document.createElement("p");
  body.className = "hub-list-body";
  body.textContent = comment.body;
  row.appendChild(body);

  const meta = document.createElement("div");
  meta.className = "hub-list-meta mono";

  const author = authorNode(comment.author);
  const time = timeNode(comment.created_at);
  meta.append(author, document.createTextNode(" · "), time);

  if (comment.anchor_version) {
    const dot = document.createTextNode(" · ");
    const verSpan = document.createElement("span");
    verSpan.className = "hub-ver-stamp";
    verSpan.textContent = `v${comment.anchor_version}`;
    if (comment.anchor_version === commentsState.shownVersion && !comment.done) {
      verSpan.className += " current";
    }
    meta.append(dot, verSpan);

    if (comment.anchor_version !== commentsState.shownVersion && !comment.done) {
      const openV = document.createElement("button");
      openV.type = "button";
      openV.className = "hub-open-version-btn";
      openV.textContent = `open v${comment.anchor_version}`;
      openV.addEventListener("click", (e) => {
        e.stopPropagation();
        closeCommentsDrawer();
        const base = `#/artifacts/${encodeURIComponent(commentsState.artifactId)}?version=${comment.anchor_version}`;
        location.hash = base;
      });
      meta.append(document.createTextNode(" · "), openV);
    }
  }

  if (comment.done) {
    const resNote = document.createElement("span");
    resNote.textContent = " · resolved";
    meta.appendChild(resNote);
  }

  row.appendChild(meta);

  // Clicking row opens sheet on phone
  row.addEventListener("click", () => {
    openCommentSheet(comment);
  });

  return row;
}

// Render phone sheet for a single comment thread (Screen 05)
function renderPhoneSheet() {
  const els = commentsState.elements;
  if (!els) return;

  const comment = commentsState.comments.find((c) => String(c.id) === String(commentsState.activeThreadId));
  if (!comment) {
    commentsState.viewMode = "list";
    renderListView();
    return;
  }

  els.drawer.className = "comments-drawer hub-comment-sheet";
  els.list.innerHTML = "";
  els.foot?.replaceChildren();

  if (els.head) els.head.hidden = true;

  // Grab handle
  const handle = document.createElement("div");
  handle.className = "hub-sheet-handle";
  els.list.appendChild(handle);

  // Sheet header: "Comment" · Resolve · Close
  const head = document.createElement("div");
  head.className = "hub-sheet-head";

  // A thread was a one-way door: close took the whole drawer and the
  // platform's back gesture left the artifact, so the only route from one
  // comment to the others was to dismiss everything and start again.
  const backBtn = document.createElement("button");
  backBtn.type = "button";
  backBtn.className = "hub-sheet-back";
  backBtn.setAttribute("aria-label", "Back to all comments");
  backBtn.innerHTML = glyphSvg("chevronBack", { size: 18 });
  backBtn.addEventListener("click", () => {
    commentsState.activeThreadId = null;
    commentsState.viewMode = "list";
    renderListView();
    const first = els.list.querySelector(".hub-list-comment, .comment");
    (first || els.drawer).focus();
  });

  const title = document.createElement("span");
  title.className = "hub-sheet-title";
  title.textContent = "Comment";

  const resolveBtn = document.createElement("button");
  resolveBtn.type = "button";
  resolveBtn.className = "hub-sheet-resolve";
  resolveBtn.innerHTML = `${glyphSvg("resolve", { size: 14 })}<span>${comment.done ? "Reopen" : "Resolve"}</span>`;
  resolveBtn.setAttribute("aria-label", `${comment.done ? "Reopen" : "Resolve"} comment`);
  resolveBtn.addEventListener("click", async () => {
    await toggleCommentDone(comment);
    renderPhoneSheet();
  });

  const closeBtn = document.createElement("button");
  closeBtn.type = "button";
  closeBtn.className = "hub-sheet-close";
  closeBtn.setAttribute("aria-label", "Close");
  closeBtn.innerHTML = glyphSvg("close", { size: 18 });
  closeBtn.addEventListener("click", closeCommentsDrawer);

  head.append(backBtn, title, resolveBtn, closeBtn);
  els.list.appendChild(head);

  // Quote block
  const qb = quoteBlockNode(comment, commentsState.shownVersion);
  if (qb) els.list.appendChild(qb);

  // Thread root
  const thread = document.createElement("div");
  thread.className = "hub-sheet-thread";

  const rootItem = document.createElement("div");
  rootItem.className = "hub-thread-root";
  const rootMeta = document.createElement("div");
  rootMeta.className = "hub-thread-meta";
  rootMeta.append(authorNode(comment.author), timeNode(comment.created_at));
  const rootBody = document.createElement("p");
  rootBody.className = "hub-thread-body";
  rootBody.textContent = comment.body;
  rootItem.append(rootMeta, rootBody);
  thread.appendChild(rootItem);

  // Sibling replies quoting same anchor
  const replies = commentsState.comments.filter(
    (c) =>
      c.id !== comment.id &&
      c.anchor &&
      comment.anchor &&
      c.anchor.quote === comment.anchor.quote &&
      c.anchor_version === comment.anchor_version,
  );
  for (const reply of replies) {
    const repItem = document.createElement("div");
    repItem.className = "hub-thread-reply";
    const repMeta = document.createElement("div");
    repMeta.className = "hub-thread-meta";
    repMeta.append(authorNode(reply.author), timeNode(reply.created_at));
    const repBody = document.createElement("p");
    repBody.className = "hub-thread-body";
    repBody.textContent = reply.body;
    repItem.append(repMeta, repBody);
    thread.appendChild(repItem);
  }
  els.list.appendChild(thread);

  // The reply composer is the same one the new comment is written in.
  const reply = commentComposer({
    label: "Reply",
    placeholder: "Reply",
    className: "hub-sheet-composer",
    send: async (text) => {
      await postComment(text, comment.anchor ? comment.anchor.quote : null);
      renderPhoneSheet();
      // The thread is drawn again with the reply in it, and a new composer
      // under it; the reader is put back in that one rather than left on the
      // page behind the sheet.
      commentsState.elements?.foot?.querySelector(".composer-field")?.focus();
    },
  });
  // Beside the thread, not in it: the thread is a log, and a log reads out
  // every change inside it, the composer's own line included.
  els.foot.appendChild(reply.wrap);

  // Footer: "Swipe the sheet down to keep reading" · 1 of N
  const openDocs = commentsState.comments.filter((c) => !c.done && c.anchor_version === commentsState.shownVersion);
  const curIdx = openDocs.findIndex((c) => String(c.id) === String(comment.id));
  const footer = document.createElement("div");
  footer.className = "hub-sheet-footer";

  const swipeText = document.createElement("span");
  swipeText.textContent = "Swipe the sheet down to keep reading";

  const navWrap = document.createElement("div");
  navWrap.className = "hub-sheet-nav mono";

  if (openDocs.length > 1 && curIdx >= 0) {
    const prevBtn = document.createElement("button");
    prevBtn.type = "button";
    prevBtn.className = "hub-nav-btn";
    prevBtn.textContent = "‹";
    prevBtn.setAttribute("aria-label", "Previous comment");
    prevBtn.addEventListener("click", () => {
      const nextIdx = (curIdx - 1 + openDocs.length) % openDocs.length;
      openCommentSheet(openDocs[nextIdx]);
    });

    const countText = document.createElement("span");
    countText.textContent = `${curIdx + 1} of ${openDocs.length}`;

    const nextBtn = document.createElement("button");
    nextBtn.type = "button";
    nextBtn.className = "hub-nav-btn";
    nextBtn.textContent = "›";
    nextBtn.setAttribute("aria-label", "Next comment");
    nextBtn.addEventListener("click", () => {
      const nextIdx = (curIdx + 1) % openDocs.length;
      openCommentSheet(openDocs[nextIdx]);
    });

    navWrap.append(prevBtn, countText, nextBtn);
  } else if (openDocs.length === 1) {
    navWrap.textContent = "1 of 1";
  }

  footer.append(swipeText, navWrap);
  els.list.appendChild(footer);
}

// Render compose sheet (opened from text selection or "Start a thread")
function renderComposeSheet() {
  const els = commentsState.elements;
  if (!els) return;

  els.drawer.className = "comments-drawer hub-comment-sheet";
  els.list.innerHTML = "";
  els.foot?.replaceChildren();

  if (els.head) els.head.hidden = true;

  const handle = document.createElement("div");
  handle.className = "hub-sheet-handle";
  els.list.appendChild(handle);

  const head = document.createElement("div");
  head.className = "hub-sheet-head";
  const title = document.createElement("span");
  title.className = "hub-sheet-title";
  title.textContent = "New Comment";

  const closeBtn = document.createElement("button");
  closeBtn.type = "button";
  closeBtn.className = "hub-sheet-close";
  closeBtn.setAttribute("aria-label", "Close");
  closeBtn.innerHTML = glyphSvg("close", { size: 18 });
  closeBtn.addEventListener("click", closeCommentsDrawer);
  head.append(title, closeBtn);
  els.list.appendChild(head);

  if (commentsState.composeQuote && !commentsState.isProtected) {
    const dummy = {
      anchor: { mode: "text", quote: commentsState.composeQuote },
      anchor_version: commentsState.shownVersion,
      done: false,
    };
    const qb = quoteBlockNode(dummy, commentsState.shownVersion);
    if (qb) els.list.appendChild(qb);
  }

  const made = commentComposer({
    label: "New comment",
    placeholder: commentsState.composeQuote
      ? "Write a comment on this text"
      : "Write a comment",
    className: "comments-compose",
    send: async (text) => {
      await postComment(text, commentsState.composeQuote);
      commentsState.composeQuote = null;
      closeCommentsDrawer();
    },
  });
  els.foot.appendChild(made.wrap);
  setTimeout(() => made.focus(), 50);
}

// Desktop Margin Column (Screen 07): from 900px
export function renderDesktopCards() {
  const container = document.querySelector(".hub-comments-cards-list") || commentsState.desktopContainer;
  if (!container) return;
  if (typeof window !== "undefined" && window.innerWidth < 900) {
    container.innerHTML = "";
    return;
  }
  container.innerHTML = "";

  const openCount = commentsState.comments.filter((c) => !c.done).length;


  if (!commentsState.comments.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "No comments yet.";
    container.appendChild(empty);
    return;
  }

  const openThisVer = commentsState.comments
    .filter((c) => !c.done && c.anchor_version === commentsState.shownVersion)
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));

  const olderVer = commentsState.comments
    .filter((c) => !c.done && c.anchor_version !== commentsState.shownVersion)
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));

  const resolved = commentsState.comments
    .filter((c) => c.done)
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));

  // Render open cards for this version
  for (const comment of openThisVer) {
    container.appendChild(renderDesktopCard(comment));
  }

  // Render older version cards
  for (const comment of olderVer) {
    container.appendChild(renderDesktopCard(comment));
  }

  // Resolved collapsed section
  if (resolved.length > 0) {
    const resBar = document.createElement("button");
    resBar.type = "button";
    resBar.className = "hub-col-resolved-toggle";
    resBar.innerHTML = `<svg class="resolved-glyph" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="var(--ok)" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-label="Resolved"><path d="M5 12.5l4.5 4.5L19 7.5"></path></svg><span>Resolved · ${resolved.length}</span>`;
    container.appendChild(resBar);

    const resWrap = document.createElement("div");
    resWrap.className = "hub-col-resolved-list";
    resWrap.hidden = !commentsState.resolvedExpanded;
    for (const comment of resolved) {
      resWrap.appendChild(renderDesktopCard(comment));
    }
    container.appendChild(resWrap);

    resBar.addEventListener("click", () => {
      commentsState.resolvedExpanded = !commentsState.resolvedExpanded;
      resWrap.hidden = !commentsState.resolvedExpanded;
    });
  }
}

export function renderDesktopCard(comment) {
  const card = document.createElement("div");
  card.className = "hub-comment-card" + (comment.done ? " resolved" : "");
  card.dataset.commentId = String(comment.id);

  if (String(comment.id) === String(commentsState.activeThreadId)) {
    card.classList.add("active");
  }

  // Resolved thread carries check glyph and the word Resolved (never dimming alone)
  if (comment.done) {
    const resHead = document.createElement("div");
    resHead.className = "hub-card-resolved-head mono";
    resHead.innerHTML = `<svg class="resolved-glyph" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="var(--ok)" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-label="Resolved"><path d="M5 12.5l4.5 4.5L19 7.5"></path></svg>`;
    const span = document.createElement("span");
    span.className = "hub-card-resolved-text";
    const quoteLabel = comment.anchor && comment.anchor.quote ? ` · “${comment.anchor.quote}”` : "";
    span.textContent = `Resolved${quoteLabel}`;
    resHead.appendChild(span);
    card.appendChild(resHead);
  } else if (comment.anchor) {
    // Tinted or outlined quote for open threads
    const qb = quoteBlockNode(comment, commentsState.shownVersion);
    if (qb) card.appendChild(qb);
  }

  const body = document.createElement("p");
  body.className = "hub-card-body";
  body.textContent = comment.body;
  card.appendChild(body);

  // Meta row: author · time · version; Reply; Resolve
  const meta = document.createElement("div");
  meta.className = "hub-card-meta mono";

  const author = authorNode(comment.author);
  const time = timeNode(comment.created_at);
  meta.append(author, document.createTextNode(" · "), time);

  if (comment.anchor_version) {
    const verSpan = document.createElement("span");
    verSpan.className = "hub-card-ver";
    verSpan.textContent = `v${comment.anchor_version}`;
    if (comment.anchor_version === commentsState.shownVersion && !comment.done) {
      verSpan.className += " current";
    }
    meta.append(document.createTextNode(" · "), verSpan);
  }

  const spacer = document.createElement("span");
  spacer.className = "grow";
  meta.appendChild(spacer);

  if (comment.anchor_version !== commentsState.shownVersion && !comment.done) {
    const openV = document.createElement("button");
    openV.type = "button";
    openV.className = "hub-card-open-v";
    openV.textContent = `Open v${comment.anchor_version}`;
    openV.addEventListener("click", () => {
      location.hash = `#/artifacts/${encodeURIComponent(commentsState.artifactId)}?version=${comment.anchor_version}`;
    });
    meta.appendChild(openV);
  } else if (!comment.done) {
    const replyBtn = document.createElement("button");
    replyBtn.type = "button";
    replyBtn.className = "hub-card-act";
    replyBtn.textContent = "Reply";
    replyBtn.addEventListener("click", () => {
      openCommentSheet(comment);
    });

    const resolveBtn = document.createElement("button");
    resolveBtn.type = "button";
    resolveBtn.className = "hub-card-act";
    resolveBtn.textContent = "Resolve";
    resolveBtn.addEventListener("click", () => toggleCommentDone(comment));

    meta.append(replyBtn, resolveBtn);
  }

  card.appendChild(meta);

  // Hover or focus on card brightens highlight in frame
  const setCardHover = (active) => {
    if (active) card.classList.add("active");
    else if (String(comment.id) !== String(commentsState.activeThreadId)) {
      card.classList.remove("active");
    }
    const frame = document.querySelector("#hub-frame");
    if (frame && frame.contentWindow) {
      frame.contentWindow.postMessage(
        { type: "hub:highlight-comment", commentId: comment.id, active },
        "*",
      );
    }
  };

  card.addEventListener("mouseenter", () => setCardHover(true));
  card.addEventListener("mouseleave", () => setCardHover(false));
  card.addEventListener("focusin", () => setCardHover(true));
  card.addEventListener("focusout", () => setCardHover(false));

  return card;
}

export function openCommentsDrawer() {
  const els = commentsState.elements;
  if (!els) return;
  commentsState.lastFocus = document.activeElement;
  commentsState.open = true;
  commentsState.viewMode = "list";
  clearDrawerError();
  els.backdrop.hidden = false;
  els.drawer.hidden = false;
  if (els.toggle) els.toggle.setAttribute("aria-expanded", "true");
  renderComments();
  loadComments();
}

export function openCommentSheet(comment) {
  const els = commentsState.elements;
  if (!els) return;
  commentsState.lastFocus = document.activeElement;
  commentsState.open = true;
  commentsState.activeThreadId = comment.id;
  commentsState.viewMode = "sheet";
  clearDrawerError();
  els.backdrop.hidden = false;
  els.drawer.hidden = false;
  if (els.toggle) els.toggle.setAttribute("aria-expanded", "true");
  renderComments();

  // Highlight and scroll in frame
  const frame = document.querySelector("#hub-frame");
  if (frame && frame.contentWindow) {
    frame.contentWindow.postMessage(
      { type: "hub:scroll-to-comment", commentId: comment.id },
      "*",
    );
  }
}

export function openCompose(quote) {
  const els = commentsState.elements;
  if (!els) return;
  commentsState.lastFocus = document.activeElement;
  commentsState.open = true;
  commentsState.composeQuote = quote || null;
  commentsState.viewMode = "compose";
  clearDrawerError();
  els.backdrop.hidden = false;
  els.drawer.hidden = false;
  if (els.toggle) els.toggle.setAttribute("aria-expanded", "true");
  renderComments();
}

export function closeCommentsDrawer() {
  const els = commentsState.elements;
  commentsState.open = false;
  commentsState.viewMode = "list";
  commentsState.composeQuote = null;
  if (!els) return;
  els.backdrop.hidden = true;
  els.drawer.hidden = true;
  if (els.toggle) els.toggle.setAttribute("aria-expanded", "false");
  if (commentsState.lastFocus && document.contains(commentsState.lastFocus)) {
    commentsState.lastFocus.focus();
  }
  commentsState.lastFocus = null;
}

export function startComments(id, shownVersion = 1, isProtected = false) {
  commentsState.artifactId = id;
  commentsState.shownVersion = Number(shownVersion) || 1;
  commentsState.isProtected = !!isProtected;
  commentsState.comments = [];
  commentsState.open = false;
  commentsState.activeThreadId = null;
  commentsState.composeQuote = null;
  commentsState.viewMode = "list";
  commentsState.lastFocus = null;
  commentsState.elements = null;
  commentsState.seq += 1;
}

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
    if (commentsState.open && commentsState.viewMode === "list") {
      closeCommentsDrawer();
    } else {
      openCommentsDrawer();
    }
  });
  return { toggle, badge };
}

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
  close.className = "hub-sheet-close";
  close.innerHTML = glyphSvg("close", { size: 18 });
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

  const made = commentComposer({
    label: "New comment",
    placeholder: "Write a comment",
    className: "comments-compose",
    send: (body) => postComment(body, null),
  });
  const form = made.wrap;

  // Where a sheet view puts its composer: after the list, outside the log.
  const foot = document.createElement("div");
  foot.className = "comments-foot";

  drawer.append(head, error, list, foot, form);

  // A phone keyboard covers the bottom of the layout viewport without moving
  // it, so the drawer is held above what is still visible: its foot is lifted
  // by the part of the window the keyboard covers, and a sheet is capped at
  // the visible height. A computed value, so it is the one style the script
  // writes.
  const viewport = globalThis.visualViewport;
  if (viewport) {
    const fit = () => {
      const covered = Math.max(0, window.innerHeight - viewport.height - viewport.offsetTop);
      drawer.style.setProperty("--keyboard-inset", `${Math.round(covered)}px`);
      drawer.style.setProperty("--visible-height", `${Math.round(viewport.height)}px`);
    };
    viewport.addEventListener("resize", fit);
    viewport.addEventListener("scroll", fit);
    fit();
  }

  drawer.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeCommentsDrawer();
      return;
    }
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

  commentsState.elements = {
    backdrop, drawer, head, toggle, badge, close, list, foot, error, compose: made.field, composeForm: form,
  };
  loadComments();
  return { backdrop, drawer };
}


// The comment control for a touch device: one fixed button in the corner the
// thumb is already near, rather than a callout beside the selection. The
// space beside a selection belongs to the platform's own menu on Android, and
// the hub cannot win it, so it stops asking for it.
//
// It lives here, in the shell, because the artifact is two frames down and
// each of them is sized to its own content: nothing inside either can be
// fixed to a viewport that scrolls. The frame posts the selection out, and
// this is what hears it.
let selectionFab = null;

// The button lives on the body, outside the region the router repaints, so
// nothing about a route change removes it on its own. Leaving an artifact
// with text still selected would carry a fixed button onto the next screen,
// where it sits over content that has nothing to comment on.
if (typeof window !== "undefined") {
  window.addEventListener("hashchange", () => removeSelectionButton());
}

function removeSelectionButton() {
  if (selectionFab) {
    leave(selectionFab);
    selectionFab = null;
  }
}

function selectionButton(quote) {
  const wanted = typeof quote === "string" ? quote.trim() : "";
  if (!wanted) {
    removeSelectionButton();
    return;
  }
  const coarse =
    typeof window.matchMedia === "function" && window.matchMedia("(pointer: coarse)").matches;
  if (!coarse) return;
  if (selectionFab) {
    selectionFab.dataset.quote = wanted;
    return;
  }
  const button = document.createElement("button");
  button.type = "button";
  button.className = "hub-comment-fab";
  button.dataset.quote = wanted;
  // The word as well as the glyph. A lone glyph arriving unannounced in a
  // corner is a generic compose button; the word says what it will do in the
  // half second it is on screen before a thumb reaches it.
  button.setAttribute("aria-label", "Comment on selected text");
  button.append(glyphNode(), labelNode());
  // pointerdown, never mousedown: a touch-drag selection never fires mouse
  // events, so the control that used to be here could not be pressed even
  // when nothing covered it. preventDefault keeps the selection alive through
  // the press, which is what the quote is taken from.
  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    const text = button.dataset.quote || "";
    removeSelectionButton();
    openCompose(text);
  });
  document.body.appendChild(button);
  selectionFab = button;
  arrive(button);
}

// The arrival is script, not CSS, and deliberately so. `app.css` may not carry
// the word `transition`, and the reduced-motion rule in `tokens.css` turns off
// every transition and animation with `!important`, which would have taken the
// drawn opacity-only variant with it. Both of those stay absolute: a rule that
// stops everything can be trusted without being read. The one control that has
// earned an exception pays for it here, where the exception is deliberate and
// somebody reviewing this file can see it.
function calmly() {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

function arrive(button) {
  if (typeof button.animate !== "function") return;
  const calm = calmly();
  button.animate(
    calm
      ? [{ opacity: 0 }, { opacity: 1 }]
      : [
          { opacity: 0, transform: "translateY(8px)" },
          { opacity: 1, transform: "none" },
        ],
    { duration: 120, easing: "ease-out", fill: "both" },
  );
}

// Leaving is the same call at 90ms and opacity only, with the element removed
// when it finishes rather than before, so it is not snatched away mid-fade.
function leave(button) {
  if (typeof button.animate !== "function") {
    button.remove();
    return;
  }
  const going = button.animate([{ opacity: 1 }, { opacity: 0 }], {
    duration: 90,
    easing: "ease-out",
    fill: "both",
  });
  going.addEventListener("finish", () => button.remove());
}

function glyphNode() {
  const holder = document.createElement("span");
  holder.className = "hub-comment-fab-glyph";
  holder.innerHTML = glyphSvg("comments", { size: 20, strokeWidth: 1.8 });
  return holder;
}

function labelNode() {
  const label = document.createElement("span");
  label.textContent = "Comment";
  return label;
}


// Global listener for frame messages
if (typeof window !== "undefined") {
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (!data || typeof data !== "object") return;
    if (data.type === "hub:selection-change") {
      selectionButton(data.quote);
    } else if (data.type === "hub:create-comment") {
      removeSelectionButton();
      openCompose(data.quote);
    } else if (data.type === "hub:open-comment") {
      const comment = commentsState.comments.find(
        (c) => String(c.id) === String(data.commentId),
      );
      if (comment) openCommentSheet(comment);
    } else if (data.type === "hub:hover-comment") {
      const card = document.querySelector(
        `.hub-comment-card[data-comment-id="${data.commentId}"]`,
      );
      if (card) {
        if (data.active) card.classList.add("active");
        else card.classList.remove("active");
      }
    } else if (data.type === "hub:frame-ready") {
      sendCommentsToFrame();
    }
  });
  window.addEventListener("resize", renderDesktopCards);
}
