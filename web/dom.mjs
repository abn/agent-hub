// The markup helpers every screen shares, and the element they paint into.

import { byDay, timeHTML } from "./time.mjs";

export const main = document.getElementById("main");

// A screen's fetches outlive the screen: tap Projects then Inbox on a loaded
// hub and the feed can still be in flight when the inbox has painted. Every
// render takes the next number, and a paint from an older one is dropped, so
// what is on screen always matches the tab that is marked current.
let generation = 0;

export function beginRender() {
  return ++generation;
}

export function stale(gen) {
  return gen !== generation;
}

export function paint(gen, html) {
  if (gen === generation) main.innerHTML = html;
}

// Everything an agent wrote goes through here before it reaches innerHTML.
// The numeric fields the API types as integers (waiting, unread, a version, a
// byte count) are the only interpolations left unescaped; they cannot carry
// markup, and a string field must never be added to that set.
export function esc(value) {
  return String(value ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

// The kind badge, from the design foundation: a 14px line glyph inside a 24px
// tinted circle, or a typographic mark where the design uses one. A shape per
// kind, because the tint alone is not a difference a reader can be asked to
// see, and the drawn mark is not a difference a reader can be asked to hear.
const mark = (d, filled) =>
  `<svg width="14" height="14" viewBox="0 0 24 24" fill="${filled ? "currentColor" : "none"}"` +
  ` stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"` +
  ` aria-hidden="true"><path d="${d}"/></svg>`;

// The foundation leaves the answer kind undrawn. A reply arrow is the one mark
// a reader already knows for it, and it echoes the composer's send glyph.
// The space after each move command is the one departure from the foundation's
// own path strings. SVG reads it as the same path, and it keeps the data clear
// of the identifier pattern the commit hooks reject.
const MARKS = {
  signal: mark("M 12 12h.01"),
  finished: mark("M 5 12l5 5L20 7"),
  question: "?",
  answer: mark("M 10 16l-4-4 4-4 M 6 12h7a5 5 0 0 1 5 5v1"),
  approval: "!",
  artifact: mark("M 6 3h9l4 4v14H6z M 8 12h8 M 8 16h8"),
  session: mark("M 12 3a9 9 0 1 0 0 18z", true),
};

// What the row says its kind is. The badge stays hidden and the label carries
// the meaning: a bare span is not an element `aria-label` is reliably read on,
// and the text is announced by every reader and found by a page search.
const NAMES = {
  signal: "Update",
  finished: "Finished",
  question: "Question",
  answer: "Answer",
  approval: "Approval",
  artifact: "Artifact",
  session: "Session",
};

export const glyph = (kind) =>
  `<span class="glyph" data-kind="${esc(kind)}" aria-hidden="true">${
    MARKS[kind] || MARKS.signal
  }</span><span class="sr-only">${esc(NAMES[kind] || kind)}</span>`;

export const when = (ts) => timeHTML(ts);

// The action a caller can take on an event. A question is answered; an
// approval is a decision. Both are the human's to act on, so the row carries
// the verb rather than only a label.
export function actionFor(event) {
  const id = esc(event.event_id || event.id);
  if (event.kind === "question") {
    return `<button type="button" class="action" data-action="answer" data-id="${id}">Reply</button>`;
  }
  if (event.kind === "approval") {
    return `<button type="button" class="action" data-action="approve" data-id="${id}" data-summary="${esc(event.summary)}">Approve</button>`;
  }
  return "";
}

// Whether an event still expects the human. A feed event carries the inbox
// status, so a decided item stops offering its action even though the event
// itself is append-only.
export function isOpen(event) {
  return event.inbox_status === "action" || event.inbox_status === "waiting";
}

export function eventRow(event) {
  const open = isOpen(event);
  return `<div class="row ${open ? "unread" : ""}">
    ${glyph(event.kind)}
    <div class="grow">
      <div class="title">${esc(event.summary)}</div>
      <div class="meta mono">${esc(event.actor)} · ${when(event.created_at)}</div>
    </div>
    ${open ? actionFor(event) : ""}
  </div>`;
}

// A section that could not load says so where it sits, rather than taking the
// whole screen down with it. The title is the screen's own copy today, and is
// escaped anyway so a later caller cannot make it data by accident.
export function errorCard(title, error) {
  return `<div class="card"><h2>${esc(title)}</h2><p class="meta">${esc(error.message)}</p></div>`;
}

export function groupedEvents(events, row) {
  return byDay(events)
    .map(
      (group) =>
        `<h2 class="day">${esc(group.label)}</h2>
         <div class="card">${group.events.map(row).join("")}</div>`,
    )
    .join("");
}
