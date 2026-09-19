// Inbox: the queue the human acts on, what is unread, and what was read, with
// the verbs that clear each: answer, decide, and mark read.

import { api } from "./api.mjs";
import { composer } from "./composer.mjs";
import { confirmAction } from "./dialog.mjs";
import { actionFor, esc, glyph, main, paint, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { registerPane, registerScreen } from "./keys.mjs";
import { twoPane } from "./project.mjs";
import { render } from "./router.mjs";
import { fullStamp, relative } from "./time.mjs";
import { toast } from "./toast.mjs";

// The design's swipe geometry: a 112px tray under a right swipe, 88px per
// action under a left one. A drag is a swipe once it has moved this far along
// one axis, and a pull refreshes once it has come this far down.
const READ_TRAY = 112;
const ACTION_WIDTH = 88;
const SLOP = 8;
const PULL = 96;
const DESKTOP = "(min-width: 1100px)";
const CHECK_PATH = "M 5 12l5 5L20 7";

// When the inbox was last read from the hub, for the last-synced line. It is
// set by a fetch that succeeded and by nothing else.
let syncedAt = 0;
let syncTimer = 0;

const waits = (item) => item.status === "action" || item.status === "waiting";

// The filter and the open item live in the address, so a reload keeps both and
// Back closes a card.
function view() {
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  return { open: params.get("open") || "", unreadOnly: params.get("unread") === "1" };
}

function address({ open, unreadOnly }) {
  const params = new URLSearchParams();
  if (unreadOnly) params.set("unread", "1");
  if (open) params.set("open", open);
  const query = params.toString();
  return query ? `#/inbox?${query}` : "#/inbox";
}

// A plain time rather than the pressable one: a row already carries a link
// and its verbs, and a fourth target stacked over them would overlap theirs.
// The whole stamp is the title, and it is what a reader hears.
function stamp(ts) {
  const ms = Date.parse(ts);
  if (!Number.isFinite(ms)) return "";
  const full = esc(fullStamp(ms));
  return `<time class="inbox-time" datetime="${esc(new Date(ms).toISOString())}" title="${full}"><span aria-hidden="true">${esc(relative(ms))}</span><span class="sr-only">${full}</span></time>`;
}

function bodyOf(item) {
  const body = item.payload && item.payload.body;
  return typeof body === "string" ? body.trim() : "";
}

// What the row's right swipe and its visible control both do. An item that
// waits carries no read state, so it offers neither.
function readControl(item) {
  if (waits(item)) return "";
  const id = esc(item.event_id);
  return item.status === "unread"
    ? `<button type="button" class="inbox-quiet" data-action="inbox-read" data-id="${id}">Mark read</button>`
    : `<button type="button" class="inbox-quiet" data-action="inbox-unread" data-id="${id}">Mark unread</button>`;
}

function decline(item, action) {
  return `<button type="button" data-action="${action}" data-id="${esc(item.event_id)}" data-summary="${esc(item.summary)}">Decline</button>`;
}

function rowActions(item) {
  if (!waits(item)) return readControl(item);
  return (item.kind === "approval" ? decline(item, "inbox-decline") : "") + actionFor(item);
}

// The trays a swipe uncovers. They sit under the row and stay out of the tab
// ring and the accessibility tree until a swipe shows them; every action in
// them is also a control on the row itself.
function trays(item) {
  const id = esc(item.event_id);
  if (!waits(item)) {
    const label = item.status === "unread" ? "Mark read" : "Mark unread";
    return `<div class="swipe-tray swipe-tray-read" aria-hidden="true" hidden>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="${CHECK_PATH}"/></svg>${label}</div>`;
  }
  if (item.kind === "approval") {
    return `<div class="swipe-tray swipe-tray-actions" hidden>
      ${decline(item, "inbox-tray-decline")}
      <button type="button" class="action" data-action="inbox-tray-approve" data-id="${id}" data-summary="${esc(item.summary)}">Approve</button>
    </div>`;
  }
  if (item.kind === "question") {
    return `<div class="swipe-tray swipe-tray-actions" hidden>
      <button type="button" class="action" data-action="inbox-tray-answer" data-id="${id}">Reply</button>
    </div>`;
  }
  return "";
}

function inboxRow(item, state) {
  const body = waits(item) ? bodyOf(item) : "";
  const tone = waits(item) ? "is-waiting" : item.status === "unread" ? "is-unread" : "is-read";
  const swipe = waits(item) ? (trays(item) ? "actions" : "") : "read";
  const href = esc(address({ ...state, open: item.event_id }));
  const current = state.open === item.event_id ? ' aria-current="true"' : "";
  return `<div class="inbox-item" data-id="${esc(item.event_id)}" data-status="${esc(item.status)}" data-swipe="${swipe}"${current}>
    ${trays(item)}
    <div class="row inbox-row ${tone}">
      ${glyph(item.kind)}
      <div class="grow">
        <div class="inbox-head">
          <div class="title"><a href="${href}">${esc(item.summary)}</a></div>
          ${stamp(item.updated_at)}
        </div>
        ${body ? `<div class="inbox-body">${esc(body)}</div>` : ""}
        <div class="inbox-foot">
          <span class="inbox-project">${esc(item.project_id)}</span><span aria-hidden="true">·</span><span class="inbox-actor">${esc(item.actor)}</span>
          <span class="inbox-acts">${rowActions(item)}</span>
        </div>
      </div>
      ${item.status === "unread" ? '<span class="dot-unread" aria-hidden="true"></span><span class="sr-only">Unread</span>' : ""}
    </div>
  </div>`;
}

// Group open items by actor, preserving the incoming order, so one agent's
// queue reads as one block. The count makes a runaway agent visible without
// opening every row.
function byActor(items) {
  const groups = new Map();
  for (const item of items) {
    if (!groups.has(item.actor)) groups.set(item.actor, []);
    groups.get(item.actor).push(item);
  }
  return groups;
}

function actorGroups(items, state) {
  return [...byActor(items)]
    .map(
      ([actor, rows]) =>
        `<h3 class="group">${esc(actor)} <span class="group-n">${rows.length}</span></h3>
         <div class="inbox-rows">${rows.map((row) => inboxRow(row, state)).join("")}</div>`,
    )
    .join("");
}

function group(name, title, count, rowsHTML) {
  return `<h2 class="section-label inbox-label" data-group="${name}">${title} · ${count}</h2>
    <div class="inbox-group" data-group="${name}">${rowsHTML}</div>`;
}

// Earlier folds on the desktop, where the list shares the screen with a card,
// and stays open when the open item is one of its rows.
function earlier(items, state) {
  if (!items.length) return "";
  const folded =
    window.matchMedia(DESKTOP).matches && !items.some((item) => item.event_id === state.open);
  return `<details class="inbox-group inbox-earlier" data-group="earlier"${folded ? "" : " open"}>
    <summary class="section-label inbox-label" data-group="earlier">Earlier</summary>
    <div class="inbox-rows">${items.map((row) => inboxRow(row, state)).join("")}</div>
  </details>`;
}

function syncLine() {
  return `<p class="inbox-sync"><span class="mono inbox-sync-text"></span><span class="sr-only" role="status"></span>
    <button type="button" class="inbox-quiet" data-action="inbox-refresh">Refresh</button></p>`;
}

function syncedText() {
  const age = Date.now() - syncedAt;
  if (age < 45000) return "just now";
  return age < 86400000 ? `${relative(syncedAt)} ago` : relative(syncedAt);
}

// The line is built from nodes, so the one value on it that changes can be
// rewritten in place while the minutes pass.
function drawSync(note) {
  const line = main.querySelector(".inbox-sync-text");
  if (!line) {
    clearInterval(syncTimer);
    syncTimer = 0;
    return;
  }
  if (note) {
    line.textContent = note;
    return;
  }
  const time = document.createElement("time");
  time.dateTime = new Date(syncedAt).toISOString();
  time.textContent = syncedText();
  line.replaceChildren("last synced · ", time);
  if (!syncTimer) syncTimer = setInterval(() => drawSync(), 15000);
}

// The medium card: the whole of one item, with its answers at full size.
function detail(item, state) {
  const back = esc(address({ ...state, open: "" }));
  const id = esc(item.event_id);
  const body = bodyOf(item);
  let answers = "";
  if (waits(item) && item.kind === "approval") {
    answers = `<div class="inbox-answers">
      <button type="button" class="action" data-action="inbox-detail-approve" data-id="${id}" data-summary="${esc(item.summary)}">Approve</button>
      ${decline(item, "inbox-detail-decline")}
    </div>`;
  } else if (waits(item) && item.kind === "question") {
    answers = `<div class="inbox-reply" data-id="${id}"></div>`;
  }
  return `<article class="inbox-detail" aria-labelledby="inbox-detail-title">
    <a class="inbox-back" href="${back}">Back to inbox</a>
    <div class="inbox-detail-head">
      ${glyph(item.kind)}
      ${waits(item) ? '<span class="pill">Waiting on you</span>' : ""}
      <span class="inbox-detail-meta"><span class="inbox-project">${esc(item.project_id)}</span> · ${esc(item.actor)} · ${stamp(item.updated_at)}</span>
    </div>
    <h2 class="item-title" id="inbox-detail-title">${esc(item.summary)}</h2>
    ${body ? `<p class="inbox-detail-body">${esc(body)}</p>` : ""}
    ${answers}
  </article>`;
}

// A question's card holds its composer open, as the design draws it.
function mountReply(state) {
  const slot = main.querySelector(".inbox-reply");
  if (!slot) return;
  const reply = composer({
    label: "Your answer",
    send: async (body) => {
      await sendAnswer(slot.dataset.id, body);
      await leave(state);
      toast("Answer sent, recorded on the feed.");
    },
  });
  slot.appendChild(reply.element);
}

// Back to the list from a card whose item has just been resolved. The card's
// address is replaced rather than pushed, so Back does not return to a card
// for something that is gone.
async function leave(state) {
  history.replaceState(null, "", address({ ...state, open: "" }));
  await render();
}

export async function inbox(gen) {
  // Read the queue the human acts on apart from unread finished work. Unread
  // items accumulate without expiry, so a single shared page could crowd the
  // waiting items off the end; the sets are fetched independently. Each actor
  // in the waiting group gets its own block, so one agent's queue reads as a
  // block rather than a run of rows scattered through the list.
  const state = view();
  const [action, waitingItems, unread, read] = await Promise.all([
    api("/api/v1/inbox?status=action&limit=500"),
    api("/api/v1/inbox?status=waiting&limit=500"),
    api("/api/v1/inbox?unread_only=true&limit=500"),
    state.unreadOnly ? { items: [] } : api("/api/v1/inbox?status=read&limit=100"),
  ]);
  syncedAt = Date.now();
  const waiting = [...action.items, ...waitingItems.items].sort(
    (a, b) => new Date(b.updated_at) - new Date(a.updated_at),
  );

  // Opening an item is reading it. The row moves to Earlier in the same paint
  // rather than on the next fetch.
  let opened = [...waiting, ...unread.items, ...read.items].find(
    (item) => item.event_id === state.open,
  );
  if (opened && opened.status === "unread") {
    const marked = await api(`/api/v1/inbox/${encodeURIComponent(opened.event_id)}/read`, {
      method: "POST",
    });
    opened.status = marked.status;
    if (marked.status === "read") {
      unread.items.splice(unread.items.indexOf(opened), 1);
      read.items.unshift(opened);
    }
  }
  if (stale(gen)) return;
  if (state.open && !opened) {
    // Decided or answered elsewhere, or a link to something pruned.
    history.replaceState(null, "", address({ ...state, open: "" }));
    state.open = "";
  }

  const wide = window.matchMedia(DESKTOP).matches;
  const top = `<div class="inbox-top">
      <h1>Inbox</h1>
      <button type="button" class="inbox-filter" data-action="inbox-unread-only" aria-pressed="${state.unreadOnly}">Unread only</button>
      <button type="button" class="inbox-quiet" data-action="inbox-read-all"${unread.items.length ? "" : " disabled"}>Mark all read</button>
    </div>${syncLine()}`;
  const sections =
    (waiting.length ? group("waiting", "Waiting on you", waiting.length, actorGroups(waiting, state)) : "") +
    (unread.items.length
      ? group(
          "unread",
          "Unread",
          unread.items.length,
          `<div class="inbox-rows">${unread.items.map((row) => inboxRow(row, state)).join("")}</div>`,
        )
      : "");
  const clear = sections
    ? ""
    : emptyStateHTML(EMPTY_COPY.inbox, {}, state.unreadOnly ? { href: address({ open: "" }) } : null);
  const list = `<div class="inbox-screen">${top}${sections}${clear}${earlier(read.items, state)}</div>`;

  // On a phone a card takes the screen and Back returns to the list. On the
  // desktop the list keeps its pane and the card sits beside it.
  if (opened && !wide) paint(gen, `<div class="inbox-screen"><h1 class="sr-only">Inbox</h1>${detail(opened, state)}</div>`);
  else paint(gen, twoPane(list, opened ? detail(opened, state) : ""));
  if (stale(gen)) return;
  drawSync();
  mountReply(state);
  atTop();
}

const sendAnswer = (id, body) =>
  api(`/api/v1/questions/${encodeURIComponent(id)}/answer`, {
    method: "POST",
    body: JSON.stringify({ body }),
  });

// Reply opens a composer under the row it belongs to, so the question stays
// readable while the answer is written and a failed send keeps the words.
// Pressing Reply again on an open composer moves back into it rather than
// stacking a second one.
export async function answer(id, button) {
  const row = button.closest(".inbox-item") || button.closest(".row") || button;
  const open = row.nextElementSibling;
  if (open && open.classList.contains("composer")) {
    open.querySelector(".composer-field").focus();
    return;
  }
  const reply = composer({
    label: "Your answer",
    send: async (body) => {
      await sendAnswer(id, body);
      await render();
      toast("Answer sent, recorded on the feed.");
    },
  });
  row.after(reply.element);
  button.setAttribute("aria-expanded", "true");
  reply.focus();
}

async function decide(id, decision) {
  try {
    await api(`/api/v1/approvals/${encodeURIComponent(id)}/decision`, {
      method: "POST",
      body: JSON.stringify({ decision }),
    });
  } catch (error) {
    // The item may have been decided elsewhere, so the queue is reread before
    // the failure is reported.
    await render();
    throw error;
  }
  await render();
}

// An approval is a decision. It is recorded on the feed and leaves the waiting
// queue, so the dialog names what is approved and the toast states the result.
export async function approve(id, summary) {
  const confirmed = await confirmAction({
    title: summary ? `Approve "${summary}"?` : "Approve this action?",
    body: "Your decision is recorded on the feed and resolves the waiting item.",
    safe: "Not now",
    danger: "Approve",
    tone: "action",
  });
  if (!confirmed) return;
  await decide(id, "approve");
  toast("Approved, recorded on the feed.");
}

// The other answer to the same question, asked the same way.
async function declineApproval(id, summary) {
  const confirmed = await confirmAction({
    title: summary ? `Decline "${summary}"?` : "Decline this action?",
    body: "Your decision is recorded on the feed and resolves the waiting item.",
    safe: "Not now",
    danger: "Decline",
  });
  if (!confirmed) return;
  await decide(id, "decline");
  toast("Declined, recorded on the feed.");
}

// Read state moves one way and back, so the toast carries the way back and its
// live region says what a swipe did for a reader who did not see the row move.
async function setRead(id, read) {
  await api(`/api/v1/inbox/${encodeURIComponent(id)}/${read ? "read" : "unread"}`, {
    method: "POST",
  });
  await render();
  toast(read ? "Marked read." : "Marked unread.", () => setRead(id, !read).catch(failed));
}

async function readAll() {
  const { marked } = await api("/api/v1/inbox/read-all", { method: "POST" });
  await render();
  toast(marked === 1 ? "Marked 1 item read." : `Marked ${marked} items read.`);
}

async function refresh() {
  await render();
  const button = main.querySelector('[data-action="inbox-refresh"]');
  if (button) button.focus({ preventScroll: true });
  const status = main.querySelector('.inbox-sync [role="status"]');
  if (status) requestAnimationFrame(() => (status.textContent = "Inbox refreshed."));
}

const failed = (error) => toast(`Nothing changed: ${error.message}`);

// The actions this screen owns. The entry point routes Reply and Approve on a
// row; everything the inbox added is handled here, beside the code it calls.
main.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action^='inbox-']");
  if (!button) return;
  const { action, id, summary } = button.dataset;
  const state = view();
  let work = null;
  if (action === "inbox-read") work = setRead(id, true);
  else if (action === "inbox-unread") work = setRead(id, false);
  else if (action === "inbox-read-all") work = readAll();
  else if (action === "inbox-refresh") work = refresh();
  else if (action === "inbox-unread-only") {
    location.hash = address({ ...state, unreadOnly: !state.unreadOnly });
    // The router parks focus on the region; the toggle the reader pressed is
    // where they still are.
    const again = () =>
      main.querySelector('[data-action="inbox-unread-only"]')?.focus({ preventScroll: true });
    new MutationObserver((records, observer) => {
      observer.disconnect();
      requestAnimationFrame(again);
    }).observe(main, { childList: true });
  } else if (action === "inbox-decline" || action === "inbox-tray-decline" || action === "inbox-detail-decline") {
    work = declineApproval(id, summary);
  } else if (action === "inbox-tray-approve" || action === "inbox-detail-approve") {
    work = approve(id, summary);
  } else if (action === "inbox-tray-answer") {
    const item = button.closest(".inbox-item");
    conceal();
    const reply = item && item.querySelector('.inbox-row [data-action="answer"]');
    if (reply) work = answer(id, reply);
  }
  if (work) work.catch(failed);
});

// Swipes and the pull. Touch and pen only: a mouse drag selects text, and the
// row's own controls are what a mouse uses.
let drag = null;
let revealed = null;
let unregister = null;

// A swipe that began on the row's title ends on it too, and the browser may
// call that a click. The row was swiped, not opened.
let swiped = false;
main.addEventListener(
  "click",
  (event) => {
    if (!swiped || !event.target.closest(".inbox-row")) return;
    event.preventDefault();
    event.stopPropagation();
  },
  true,
);

const still = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function conceal() {
  if (!revealed) return;
  const item = revealed;
  revealed = null;
  if (unregister) unregister();
  unregister = null;
  delete item.dataset.revealed;
  settle(item);
}

// Put a row back where its state says it rests, and hide the trays nothing is
// showing.
function settle(item) {
  const row = item.querySelector(".inbox-row");
  const open = item.dataset.revealed === "actions";
  const tray = item.querySelector(".swipe-tray-actions");
  if (row) row.style.transform = open && tray ? `translateX(${-tray.childElementCount * ACTION_WIDTH}px)` : "";
  for (const each of item.querySelectorAll(".swipe-tray")) {
    each.hidden = !(open && each === tray);
  }
}

function reveal(item) {
  conceal();
  item.dataset.revealed = "actions";
  revealed = item;
  unregister = registerPane(conceal);
  settle(item);
}

function isInbox() {
  return !!main.querySelector(".inbox-screen");
}

// Whether a downward drag is the reader's to pull or the page's to scroll is
// decided before the gesture starts, so the class follows the scroll position.
function atTop() {
  const screen = main.querySelector(".inbox-screen");
  if (screen) screen.classList.toggle("at-top", window.scrollY <= 0);
}
window.addEventListener("scroll", atTop, { passive: true });

main.addEventListener("pointerdown", (event) => {
  if (event.pointerType === "mouse" || !event.isPrimary || !isInbox()) return;
  const item = event.target.closest(".inbox-item");
  if (revealed && revealed !== item) conceal();
  if (event.target.closest("button, textarea, input, summary")) return;
  drag = {
    id: event.pointerId,
    x: event.clientX,
    y: event.clientY,
    dx: 0,
    dy: 0,
    axis: "",
    item,
    from: item && item === revealed ? -item.querySelector(".swipe-tray-actions").childElementCount * ACTION_WIDTH : 0,
    still: still(),
    pull: window.scrollY <= 0,
  };
});

function commitRead(item) {
  drag = null;
  settle(item);
  setRead(item.dataset.id, item.dataset.status === "unread").catch(failed);
}

window.addEventListener("pointermove", (event) => {
  if (!drag || event.pointerId !== drag.id) return;
  drag.dx = event.clientX - drag.x;
  drag.dy = event.clientY - drag.y;
  if (!drag.axis) {
    if (Math.abs(drag.dx) < SLOP && Math.abs(drag.dy) < SLOP) return;
    drag.axis = Math.abs(drag.dx) > Math.abs(drag.dy) ? "x" : "y";
  }
  if (drag.axis === "y") {
    if (drag.pull && drag.dy > 0) drawSync(drag.dy >= PULL ? "release to refresh" : "pull to refresh");
    return;
  }
  const { item } = drag;
  const kind = item && item.dataset.swipe;
  if (!kind) return;
  const row = item.querySelector(".inbox-row");
  let offset = drag.from + drag.dx;
  if (kind === "read") {
    offset = Math.max(0, Math.min(offset, row.offsetWidth));
    item.querySelector(".swipe-tray-read").hidden = offset === 0;
    // A full swipe commits without waiting for the release.
    if (offset >= row.offsetWidth * 0.6) {
      commitRead(item);
      return;
    }
  } else {
    const tray = item.querySelector(".swipe-tray-actions");
    offset = Math.min(0, Math.max(offset, -tray.childElementCount * ACTION_WIDTH));
    tray.hidden = offset === 0;
  }
  if (!drag.still) row.style.transform = offset ? `translateX(${offset}px)` : "";
});

function release(event) {
  if (!drag || event.pointerId !== drag.id) return;
  const { item, axis, dx, dy, from, pull } = drag;
  const cancelled = event.type === "pointercancel";
  drag = null;
  if (axis === "y") {
    if (pull && dy > 0) {
      drawSync();
      if (!cancelled && dy >= PULL) refresh().catch(failed);
    }
    return;
  }
  // A tap on a row whose actions are showing puts them away.
  if (!axis && item && item === revealed) conceal();
  const kind = item && item.dataset.swipe;
  if (axis !== "x" || !kind) return;
  swiped = true;
  setTimeout(() => (swiped = false), 50);
  if (kind === "read") {
    if (!cancelled && dx >= READ_TRAY / 2) commitRead(item);
    else settle(item);
    return;
  }
  // Past half of one action the tray opens; a swipe back the same way shuts it.
  const offset = from + dx;
  if (!cancelled && offset <= -ACTION_WIDTH / 2) reveal(item);
  else if (item === revealed) conceal();
  else settle(item);
}
window.addEventListener("pointerup", release);
window.addEventListener("pointercancel", release);

// Rows under a folded Earlier are not on screen, so the selection skips them.
registerScreen("inbox", { rows: ".inbox-row:not(details:not([open]) .inbox-row)" });
