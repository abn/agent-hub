// The markup helpers every screen shares, and the element they paint into.

import { byDay, stamp } from "./time.mjs";

export const main = document.getElementById("main");

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

export const glyph = (kind) => `<span class="glyph" data-kind="${esc(kind)}" aria-hidden="true">${
  kind === "finished" ? "✓" : kind === "question" ? "?" : kind === "approval" ? "!" : "•"
}</span>`;

export const when = (ts) => esc(stamp(ts));

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

export function groupedEvents(events, row) {
  return byDay(events)
    .map(
      (group) =>
        `<h2 class="day">${esc(group.label)}</h2>
         <div class="card">${group.events.map(row).join("")}</div>`,
    )
    .join("");
}
