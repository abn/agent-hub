// Inbox: the queue the human acts on, what is unread, and what was read, with
// the verbs that clear each: answer, decide, and mark read.

import { api } from "./api.mjs";
import { composer } from "./composer.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, glyph, main, optionsAttr, optionsFrom, paint, projectName, stale, waitingMarks } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { registerPane, registerScreen } from "./keys.mjs";
import { expiredWords, subjectAndMessage } from "./feed.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellIndexControls, shellStageHead } from "./shell-layout.mjs";
import { deadlineWords, fullStamp, relative } from "./time.mjs";
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

// The open card is what Esc closes, for as long as it is on screen. The pane
// goes when the card does and when the route leaves the inbox, so Esc on
// another screen is never this screen's to answer.
let unregisterCard = null;
// The item whose card was last open, so closing it hands focus back to the
// row it was opened from.
let lastOpen = "";

function dropCard() {
  if (!unregisterCard) return;
  unregisterCard();
  unregisterCard = null;
}

window.addEventListener("hashchange", () => {
  if (location.hash.startsWith("#/inbox")) return;
  dropCard();
  lastOpen = "";
});

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

// An agent enrolling through the hub's self-service path posts an approval
// whose payload carries the action and the reason the agent gave. The decision
// is on the agent that asked, so the card shows the reason before Approve.
function enrolOf(item) {
  if (item.kind !== "approval") return null;
  const payload = item.payload || {};
  if (payload.action !== "enrol_request") return null;
  return typeof payload.why === "string" ? payload.why.trim() : "";
}

const SNOOZE_KEY = "hub.snooze";
const SNOOZE_DURATION_MS = 60 * 60 * 1000;

function loadSnoozes() {
  try {
    const raw = localStorage.getItem(SNOOZE_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch (_) {
    return {};
  }
}

function saveSnoozes(map) {
  try {
    localStorage.setItem(SNOOZE_KEY, JSON.stringify(map));
  } catch (_) {}
}

function isSnoozed(id, now = Date.now()) {
  const map = loadSnoozes();
  const until = map[id];
  if (!until) return false;
  if (now > until) {
    delete map[id];
    saveSnoozes(map);
    return false;
  }
  return true;
}

function addSnooze(id, durationMs = SNOOZE_DURATION_MS) {
  const map = loadSnoozes();
  map[id] = Date.now() + durationMs;
  saveSnoozes(map);
}

function removeSnooze(id) {
  const map = loadSnoozes();
  delete map[id];
  saveSnoozes(map);
}

const expiredOf = (item) =>
  item.status === "resolved" &&
  Boolean((item.decision && item.decision.expired) || (item.answer && item.answer.expired));

// The deadline the asking agent set, in words: what it will do to an item that
// still waits, and what it did to one the hub resolved. Empty otherwise.
function deadlineLine(item) {
  if (waits(item) && item.expires_at) return deadlineWords(item.expires_at, item.on_expiry);
  if (expiredOf(item)) return expiredWords(item.decision ? item.decision.decision : "");
  return "";
}

function outcomeOf(item) {
  if (item.status !== "resolved") return "";
  if (expiredOf(item)) {
    if (!item.decision) return "Closed unanswered";
    return item.decision.decision === "approved" ? "Approved at deadline" : "Declined at deadline";
  }
  if (item.decision && item.decision.decision) {
    const d = String(item.decision.decision).toLowerCase();
    if (d === "approved" || d === "approve") return "Approved";
    if (d === "declined" || d === "decline") return "Declined";
    return item.decision.decision;
  }
  const payload = item.payload || {};
  if (payload.outcome) {
    const out = String(payload.outcome).toLowerCase();
    if (out === "approved" || out === "approve") return "Approved";
    if (out === "declined" || out === "decline") return "Declined";
    return payload.outcome;
  }
  if (item.kind === "approval") return "Decided";
  if (item.kind === "question") return "Answered";
  return "Resolved";
}

function noteOf(item) {
  if (item.decision && item.decision.note) {
    return String(item.decision.note).trim();
  }
  if (item.answer && item.answer.body) {
    return String(item.answer.body).trim();
  }
  const payload = item.payload || {};
  if (payload.answer) return String(payload.answer).trim();
  if (payload.note) return String(payload.note).trim();
  if (payload.decision_note) return String(payload.decision_note).trim();
  return "";
}

function decline(item, action) {
  return `<button type="button" data-action="${action}" data-id="${esc(item.event_id)}" data-summary="${esc(item.summary)}">Decline</button>`;
}

function snoozeButton(item) {
  if (!waits(item)) return "";
  return `<button type="button" class="inbox-quiet inbox-snooze-btn" data-action="inbox-snooze" data-id="${esc(item.event_id)}" data-summary="${esc(item.summary)}" title="Snooze for 1 hour">Snooze 1h</button>`;
}

function bringBackButton(item) {
  return `<button type="button" class="inbox-quiet" data-action="inbox-unsnooze" data-id="${esc(item.event_id)}" data-summary="${esc(item.summary)}">Bring back</button>`;
}

// The trays a swipe uncovers. They sit under the row and stay out of the tab
// ring and the accessibility tree until a swipe shows them; on a fine pointer
// the same verbs live on the stage the row opens.
function trays(item) {
  if (item.status === "resolved") return "";
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
      <button type="button" class="action" data-action="inbox-tray-answer" data-id="${id}"${optionsAttr(item.payload)}>Reply</button>
    </div>`;
  }
  return "";
}

function inboxRow(item, state, options = {}) {
  const isResolved = item.status === "resolved";
  const body = (waits(item) || isResolved) ? bodyOf(item) : "";
  const deadline = deadlineLine(item);
  const tone = waits(item) ? "is-waiting" : item.status === "unread" ? "is-unread" : isResolved ? "is-resolved" : "is-read";
  const swipe = waits(item) ? (trays(item) ? "actions" : "") : isResolved ? "" : "read";
  const href = esc(address({ ...state, open: item.event_id }));
  const current = state.open === item.event_id ? ' aria-current="true"' : "";
  const waiting = waits(item) ? waitingMarks(item.event_id) : null;
  const snoozeBar = options.snoozed
    ? `<div class="inbox-snooze-bar">${bringBackButton(item)}</div>`
    : "";
  return `<div class="inbox-item" data-id="${esc(item.event_id)}" data-status="${esc(item.status)}" data-swipe="${swipe}"${current}>
    ${trays(item)}
    <div class="row inbox-row ${tone}">
      ${glyph(item.kind)}
      <div class="grow">
        <div class="inbox-head">
          <div class="title${waits(item) || item.status === "unread" ? " inbox-title-strong" : ""}"><a href="${href}"${waiting ? waiting.described : ""}>${esc(item.summary)}</a></div>
        </div>
        ${body ? `<div class="inbox-body">${esc(body)}</div>` : ""}
        ${deadline ? `<div class="inbox-deadline">${esc(deadline)}</div>` : ""}
        <div class="inbox-foot">
          ${waiting && options.snoozed ? waiting.pill : ""}<span class="inbox-project">${esc(projectName(item))}</span><span aria-hidden="true">·</span><span class="inbox-actor">${esc(item.actor)}</span><span aria-hidden="true">·</span>${stamp(item.updated_at)}
        </div>
      </div>
      ${waiting ? waiting.dot : ""}
      ${item.status === "unread" ? '<span class="dot-unread" aria-label="Unread" aria-hidden="true"></span><span class="sr-only">Unread</span>' : ""}
    </div>
    ${snoozeBar}
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
  return `<details class="inbox-group inbox-earlier" data-group="earlier" open>
    <summary class="section-label inbox-label" data-group="earlier">Earlier · ${items.length}</summary>
    <div class="inbox-rows">${items.map((row) => inboxRow(row, state)).join("")}</div>
  </details>`;
}

function snoozedSection(items, state) {
  if (!items.length) return "";
  return `<section class="inbox-group inbox-snoozed" data-group="snoozed">
    <h2 class="section-label inbox-label">Snoozed · ${items.length}</h2>
    <p class="inbox-snooze-info">Snoozed for 1 hour (remembered on this device).</p>
    <div class="inbox-rows">${items.map((row) => inboxRow(row, state, { snoozed: true })).join("")}</div>
  </section>`;
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
  const isResolved = item.status === "resolved";
  const outcome = isResolved ? outcomeOf(item) : "";
  const note = isResolved ? noteOf(item) : "";
  const enrol = enrolOf(item);
  const isEnrol = enrol !== null;
  const deadline = deadlineLine(item);
  // One rule with the feed's stage: a summary that reads as a subject titles
  // the card, and a summary that is a whole message titles it with its leading
  // sentence and reads the rest as prose under it.
  const { subject, message } = subjectAndMessage(item.summary);
  let answers = "";
  if (waits(item) && item.kind === "approval") {
    answers = `<div class="inbox-answers">
      <button type="button" class="action" data-action="inbox-detail-approve" data-id="${id}" data-summary="${esc(item.summary)}">Approve</button>
      ${decline(item, "inbox-detail-decline")}
    </div>`;
  } else if (waits(item) && item.kind === "question") {
    answers = `<div class="inbox-reply" data-id="${id}"${optionsAttr(item.payload)}></div>`;
  }
  return `<article class="inbox-detail" aria-labelledby="inbox-detail-title">
    <a class="inbox-back" href="${back}"><span class="inbox-back-arrow" aria-hidden="true">&larr; </span><span class="inbox-back-label">Back to inbox</span><span class="inbox-close-label">Close</span></a>
    <div class="inbox-detail-head">
      ${glyph(item.kind)}
      ${waits(item) ? '<span class="pill pill-status"><span class="pill-dot" aria-hidden="true"></span>Waiting on you</span>' : ""}
      ${waits(item) && isEnrol ? '<span class="pill pill-enrol">Pending enrolment</span>' : ""}
      ${outcome ? `<span class="pill pill-outcome">${esc(outcome)}</span>` : ""}
      <span class="inbox-detail-meta"><span class="inbox-project">${esc(projectName(item))}</span> · ${esc(item.actor)} · ${stamp(item.updated_at)}</span>
    </div>
    <h2 class="item-title" id="inbox-detail-title">${esc(subject)}</h2>
    ${deadline ? `<p class="inbox-detail-deadline">${esc(deadline)}</p>` : ""}
    ${message ? `<p class="inbox-detail-message">${esc(message)}</p>` : ""}
    ${isEnrol ? `<div class="inbox-detail-reason"><strong>Why they are asking:</strong> ${esc(enrol || "No reason was given.")}</div>` : ""}
    ${body ? `<p class="inbox-detail-body">${esc(body)}</p>` : ""}
    ${note ? `<div class="inbox-detail-resolved-note"><strong>${outcome === "Approved" || outcome === "Declined" ? "Decision note" : "Answer"}:</strong> ${esc(note)}</div>` : ""}
    ${answers}
    ${waits(item) ? `<div class="inbox-detail-snooze-wrap">${isSnoozed(item.event_id) ? bringBackButton(item) : snoozeButton(item)}</div>` : ""}
  </article>`;
}

// A question's card holds its composer open, as the design draws it.
function mountReply(state) {
  const slot = main.querySelector(".inbox-reply");
  if (!slot) return;
  const reply = composer({
    label: "Your answer",
    options: optionsFrom(slot.dataset.options),
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
  dropCard();
  history.replaceState(history.state, "", address({ ...state, open: "" }));
  await render();
}

export async function inbox(gen) {
  // Read the queue the human acts on apart from unread finished work. Unread
  // items accumulate without expiry, so a single shared page could crowd the
  // waiting items off the end; the sets are fetched independently. Each actor
  // in the waiting group gets its own block, so one agent's queue reads as a
  // block rather than a run of rows scattered through the list.
  const state = view();
  const [action, waitingItems, unread, read, resolved] = await Promise.all([
    api("/api/v1/inbox?status=action&limit=500"),
    api("/api/v1/inbox?status=waiting&limit=500"),
    api("/api/v1/inbox?unread_only=true&limit=500"),
    state.unreadOnly ? { items: [] } : api("/api/v1/inbox?status=read&limit=100"),
    state.unreadOnly ? { items: [] } : api("/api/v1/inbox?status=resolved&limit=100"),
  ]);
  syncedAt = Date.now();
  const allWaiting = [...action.items, ...waitingItems.items].sort(
    (a, b) => new Date(b.updated_at) - new Date(a.updated_at),
  );
  const waiting = [];
  const snoozed = [];
  for (const item of allWaiting) {
    if (isSnoozed(item.event_id)) snoozed.push(item);
    else waiting.push(item);
  }

  const seenEarlier = new Set();
  const earlierItems = [];
  for (const item of [...read.items, ...resolved.items].sort(
    (a, b) => new Date(b.updated_at) - new Date(a.updated_at),
  )) {
    if (!seenEarlier.has(item.event_id)) {
      seenEarlier.add(item.event_id);
      earlierItems.push(item);
    }
  }

  // Opening an item is reading it. The row moves to Earlier in the same paint
  // rather than on the next fetch.
  let opened = [...waiting, ...snoozed, ...unread.items, ...earlierItems].find(
    (item) => item.event_id === state.open,
  );
  if (opened && opened.status === "unread") {
    const marked = await api(`/api/v1/inbox/${encodeURIComponent(opened.event_id)}/read`, {
      method: "POST",
    });
    opened.status = marked.status;
    if (marked.status === "read") {
      unread.items.splice(unread.items.indexOf(opened), 1);
      earlierItems.unshift(opened);
    }
  }
  if (stale(gen)) return;
  dropCard();
  if (state.open && !opened) {
    // Decided or answered elsewhere, or a link to something pruned.
    history.replaceState(history.state, "", address({ ...state, open: "" }));
    state.open = "";
  }
  if (opened) {
    // Replaced rather than pushed, as `leave` does it, so Back after Esc does
    // not reopen the card.
    // Esc closes the card unless an answer is half written in it. The words are
    // the card's, so it does not matter whether focus is in the field or on the
    // Send button beside it: closing would throw them away either way.
    unregisterCard = registerPane(() => {
      const draft = main.querySelector(".inbox-detail .composer-field");
      if (draft && draft.value.trim()) return;
      leave(state).catch(failed);
    });
  }
  const closed = opened ? "" : lastOpen;
  lastOpen = opened ? opened.event_id : "";

  const isDesktop = typeof window !== "undefined" && window.matchMedia && window.matchMedia(DESKTOP).matches;
  const unreadCount = unread.items.length;
  const metaText = waiting.length
    ? `${waiting.length} waiting · ${unreadCount} unread`
    : `nothing waiting · ${unreadCount} unread`;

  const indexHead = isDesktop
    ? `<div class="shell-head">
        <span class="shell-slot" aria-hidden="true"></span>
        <div class="shell-title"><h1 class="shell-title-line">Inbox</h1></div>
        <button type="button" class="inbox-filter" data-action="inbox-unread-only" aria-pressed="${state.unreadOnly}">Unread only</button>
        <button type="button" class="inbox-quiet" data-action="inbox-read-all"${unreadCount ? "" : " disabled"}>Mark all read</button>
      </div>`
    : `<style>
@media (max-width: 1099px) {
  .shell[data-segment="inbox"] .shell-controls .shell-filter,
  .shell-index:has(.inbox-rows) .shell-controls .shell-filter {
    flex: 1 !important;
    min-width: 0 !important;
  }
  .shell[data-segment="inbox"] .shell-controls .shell-filter input,
  .shell-index:has(.inbox-rows) .shell-controls .shell-filter input {
    height: 34px !important;
    border: 1px solid var(--line-strong) !important;
    border-radius: var(--r-pill) !important;
    box-sizing: border-box !important;
  }
  .shell[data-segment="inbox"] .shell-controls .chip,
  .shell-index:has(.inbox-rows) .shell-controls .chip {
    flex: none !important;
    white-space: nowrap !important;
    height: 32px !important;
    font-size: 13px !important;
    font-weight: 600 !important;
  }
}
</style><div class="shell-head">
        <span class="shell-slot" aria-hidden="true"></span>
        <div class="shell-title">
          <h1 class="shell-title-line">Inbox</h1>
          <span class="shell-meta mono">${esc(metaText)}</span>
        </div>
        <button type="button" class="inbox-quiet shell-trailing-btn" data-action="inbox-read-all" aria-label="Mark all read"${unreadCount ? "" : " disabled"}>
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 2 12.5l4.5 4.5L15 8.5"></path><path d="M 11 16l1 1 9.5-9.5"></path></svg>
        </button>
        <div class="inbox-overflow-wrap">
          <button type="button" class="inbox-overflow-btn shell-trailing-btn" data-action="inbox-overflow-toggle" aria-label="More actions" aria-haspopup="menu" aria-expanded="false">
            ${glyphSvg("overflow", { size: 19 })}
          </button>
          <div class="inbox-overflow-menu" role="menu" hidden>
            <button type="button" class="inbox-menu-item" data-action="inbox-refresh" role="menuitem">
              Refresh
            </button>
          </div>
        </div>
      </div>`;
  const sections =
    (waiting.length
      ? group(
          "waiting",
          "Waiting on you",
          waiting.length,
          `<div class="inbox-rows">${waiting.map((row) => inboxRow(row, state)).join("")}</div>`,
        )
      : "") +
    snoozedSection(snoozed, state) +
    (unread.items.length
      ? group(
          "unread",
          "Unread",
          unread.items.length,
          `<div class="inbox-rows">${unread.items.map((row) => inboxRow(row, state)).join("")}</div>`,
        )
      : "");
  const clear = sections ? "" : `<p class="empty-hint inbox-quiet-line">Nothing waiting on you.</p>`;
  const indexBody = `${sections}${clear}${earlier(earlierItems, state)}`;

  // Same shell as a project: the index lists the queue, the stage is the item.
  // On a phone the shell shows one zone at a time. The sync line rides in the
  // control row rather than under it, so the pane keeps the reserved two rows
  // every other pane has.
  const indexControls = isDesktop
    ? shellIndexControls("Filter inbox", "", syncLine())
    : `<div class="shell-controls">
        <label class="shell-filter">
          <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><path d="M16 16l4 4"></path></svg>
          <input type="search" data-index-filter placeholder="Filter inbox" aria-label="Filter inbox">
        </label>
        <button type="button" class="chip" data-action="inbox-unread-only" aria-pressed="${state.unreadOnly}">Unread</button>
      </div>`;

  const shell = shellHTML({
    segment: "inbox",
    indexHead,
    indexControls,
    indexBody,
    stageHead: opened
      ? shellStageHead(
          opened.summary,
          `${projectName(opened)} · ${opened.actor} · ${relative(opened.updated_at)}`,
          "",
          address({ ...state, open: "" }),
        )
      : shellStageHead("Inbox", ""),
    stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(
      opened ? `${projectName(opened)} / inbox / ${opened.kind}` : "no item selected",
    )}</span></div>`,
    stageBody: opened
      ? detail(opened, state)
      : sections
        ? `<div class="shell-pad"><p class="empty">Select an item from the list.</p></div>`
        : `<div class="shell-pad">${emptyStateHTML(
            EMPTY_COPY.inbox,
            {},
            state.unreadOnly ? { href: address({ open: "" }) } : null,
          )}</div>`,
    hasSelection: Boolean(opened),
  });
  paint(gen, shell);
  if (stale(gen)) return;
  installShellLayout(main);
  drawSync();
  mountReply(state);
  atTop();
  if (closed) returnFocus(closed);
}

// A card that has just closed hands focus back to the row it was opened from,
// or to the Earlier disclosure when that row is folded away. The router parks
// focus on the region after every paint, so this waits for the frame after.
function returnFocus(id) {
  const item = main.querySelector(`.inbox-item[data-id="${CSS.escape(id)}"]`);
  if (!item) return;
  const row = item.querySelector(".inbox-row");
  // A row under a folded disclosure still reports a box, so the disclosure is
  // asked whether it is folded. `checkVisibility` would say the same, but an
  // older Safari has no such call and the close would throw.
  const folded = item.closest("details:not([open])");
  const target = folded ? folded.querySelector("summary") : row;
  if (!target) return;
  requestAnimationFrame(() => {
    if (target.isConnected) target.focus({ preventScroll: true });
  });
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
  // A stage's action line is the row there: the composer goes under the line,
  // not into it beside the button.
  const row =
    button.closest(".inbox-item") || button.closest(".row") || button.closest(".feed-stage-actions") || button;
  const open = row.nextElementSibling;
  if (open && open.classList.contains("composer")) {
    open.querySelector(".composer-field").focus();
    return;
  }
  const reply = composer({
    label: "Your answer",
    options: optionsFrom(button.dataset.options),
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

// The ids a decision is on its way for. The row stays drawn until the hub
// answers, so a second press in that moment would ask and send again, and
// the refusal of the second would be the last thing the reader is told.
const deciding = new Set();

async function decide(id, decision, note) {
  deciding.add(id);
  try {
    await send(id, decision, note);
  } finally {
    deciding.delete(id);
  }
}

// The note a decision may carry, in the design's words for it. The limit is
// the hub's, which counts the trimmed note's characters and is the one that
// refuses; here it only places the count.
const NOTE = { label: "Add guidance with your decision", limit: 2000 };

// A blank note is no note, so the key is left out rather than sent empty.
async function send(id, decision, note) {
  try {
    await api(`/api/v1/approvals/${encodeURIComponent(id)}/decision`, {
      method: "POST",
      body: JSON.stringify(note ? { decision, note } : { decision }),
    });
  } catch (error) {
    // A note past the limit is refused whole: nothing was decided and nothing
    // moved, so the dialog says so beside the field and the list stays put.
    if (error.status === 413) {
      throw new Error(`The note is over the ${NOTE.limit} character limit, so nothing was decided.`);
    }
    // The item may have been decided elsewhere, so the queue is reread before
    // the failure is reported.
    await render();
    throw new Error(`Nothing changed: ${error.message}`);
  }
  await render();
}

// A decision is recorded on the feed and leaves the waiting queue, so the
// dialog names what is decided and the toast states the result. The request
// is sent with the dialog still open: a refusal lands beside the note rather
// than after the words are gone.
async function ask(id, decision, { title, danger, tone, done }) {
  if (deciding.has(id)) return;
  let noted = false;
  const confirmed = await confirmAction({
    title,
    body: "Your decision is recorded on the feed and resolves the waiting item.",
    safe: "Not now",
    danger,
    tone,
    field: NOTE,
    commit: async (note) => {
      noted = !!note;
      await decide(id, decision, note);
    },
  });
  if (confirmed) toast(`${done}${noted ? " with your note" : ""}, recorded on the feed.`);
}

export const approve = (id, summary) =>
  ask(id, "approve", {
    title: summary ? `Approve "${summary}"?` : "Approve this action?",
    danger: "Approve",
    tone: "action",
    done: "Approved",
  });

// The other answer to the same question, asked the same way.
const declineApproval = (id, summary) =>
  ask(id, "decline", {
    title: summary ? `Decline "${summary}"?` : "Decline this action?",
    danger: "Decline",
    tone: "danger",
    done: "Declined",
  });

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

async function snoozeItem(id, summary) {
  addSnooze(id);
  await render();
  toast(
    `Snoozed "${summary || id}" for 1 hour (remembered on this device).`,
    () => unsnoozeItem(id, summary).catch(failed),
  );
}

async function unsnoozeItem(id, summary) {
  removeSnooze(id);
  await render();
  toast(`Returned "${summary || id}" to Waiting on you.`);
}

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
  else if (action === "inbox-overflow-toggle") {
    const wrap = button.closest(".inbox-overflow-wrap");
    const menu = wrap?.querySelector(".inbox-overflow-menu");
    if (menu) {
      const open = menu.hidden;
      menu.hidden = !open;
      button.setAttribute("aria-expanded", open ? "true" : "false");
      if (open) {
        menu.querySelector("button")?.focus();
      }
    }
  } else if (action === "inbox-refresh") {
    const menu = button.closest(".inbox-overflow-menu");
    if (menu) {
      menu.hidden = true;
      const btn = menu.closest(".inbox-overflow-wrap")?.querySelector(".inbox-overflow-btn");
      if (btn) btn.setAttribute("aria-expanded", "false");
    }
    work = refresh();
  }
  else if (action === "inbox-snooze") work = snoozeItem(id, summary);
  else if (action === "inbox-unsnooze") work = unsnoozeItem(id, summary);
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
    // The tray is the row's own Reply: a question row carries no second one, so
    // the button that was pressed opens the composer under the row it belongs to.
    conceal();
    work = answer(id, button);
  }
  if (work) work.catch(failed);
});

// Close the overflow menu on outside click or Escape.
document.addEventListener("click", (event) => {
  if (!event.target.closest(".inbox-overflow-wrap")) {
    const menus = document.querySelectorAll(".inbox-overflow-menu:not([hidden])");
    for (const m of menus) {
      m.hidden = true;
      const b = m.closest(".inbox-overflow-wrap")?.querySelector(".inbox-overflow-btn");
      if (b) b.setAttribute("aria-expanded", "false");
    }
  }
});

main.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    const menu = main.querySelector(".inbox-overflow-menu:not([hidden])");
    if (menu) {
      event.stopPropagation();
      menu.hidden = true;
      const btn = menu.closest(".inbox-overflow-wrap")?.querySelector(".inbox-overflow-btn");
      if (btn) {
        btn.setAttribute("aria-expanded", "false");
        btn.focus();
      }
    }
  }
});

// The card's close control leaves the way Esc does: the card's address is
// replaced, so Back does not walk into a card the reader closed. A press meant
// for another tab or window is the browser's.
main.addEventListener("click", (event) => {
  if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  if (!event.target.closest(".inbox-detail a.inbox-back")) return;
  event.preventDefault();
  leave(view()).catch(failed);
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
  return !!inboxScreen();
}

// The inbox paints on the one shell; there is no separate screen element. It
// is the shell whose segment says inbox.
function inboxScreen() {
  return main.querySelector('.shell[data-segment="inbox"]');
}

// Whether a downward drag is the reader's to pull or the page's to scroll is
// decided before the gesture starts, so the class follows the scroll position.
function atTop() {
  const screen = inboxScreen();
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
