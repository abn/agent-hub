// One inbox item at full size, and the verbs that clear an item wherever it is
// shown: answer, decide, resolve or clear an idea. The list of what waits is
// Now; the Inbox's address opens Now showing what needs the reader, and an
// item's own address, `#/inbox?open=<id>`, opens it here.
//
// The item is read around its answer: what it is and who asked, the agent's
// body, then the answer block, then the thread. Its receipts sit in a margin
// beside it on a wide stage and under its title on a narrow one. Previous and
// next walk the asks in Now's order, and answering one opens the next.

import { api } from "./api.mjs";
import { enqueue } from "./offline-queue.mjs";
import { composer } from "./composer.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, kindMark, kindName, main, optionsAttr, optionsFrom, paint, projectName, stale } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { registerPane } from "./keys.mjs";
import { expiredWords, subjectAndMessage } from "./feed.mjs";
import {
  askQueue,
  clip,
  eventSentenceHTML,
  placeTime,
  queuePlace,
  readSnoozes,
  relatedAsks,
  replySentenceHTML,
  snooze,
  threadReplies,
  unsnooze,
} from "./now-model.mjs";
import { render } from "./router.mjs";
import { renderMarkdown } from "./sessions.mjs";
import { installShellLayout, shellHTML } from "./shell-layout.mjs";
import { deadlineWords, relative, stamp } from "./time.mjs";
import { toast } from "./toast.mjs";

// Where closing an item goes: the stream it was opened from.
const BACK = "#/home";

const waits = (item) => item.status === "action" || item.status === "waiting";

const itemHref = (id) => `#/inbox?open=${encodeURIComponent(id)}`;

function openId() {
  return new URLSearchParams(location.hash.split("?")[1] || "").get("open") || "";
}

// The open item is what Esc closes, for as long as it is on screen. The pane
// goes when the item does and when the route leaves it, so Esc on another
// screen is never this screen's to answer.
let unregisterCard = null;

function dropCard() {
  if (!unregisterCard) return;
  unregisterCard();
  unregisterCard = null;
}

window.addEventListener("hashchange", () => {
  if (location.hash.startsWith("#/inbox?") && openId()) return;
  dropCard();
});

// The item on screen and where answering it goes, for the verbs below.
let shown = { id: "", landing: BACK };

const kbd = (key) => `<kbd aria-hidden="true">${esc(key)}</kbd>`;
const shortId = (id) => String(id || "").slice(-6);
const clock = (size = 14) => glyphSvg("clock", { size, strokeWidth: 1.8 });

// A time a reader can place, in the opened item's own style.
const itemStamp = (ts) => stamp(ts, placeTime, "item-time");

function bodyOf(item) {
  const body = item.payload && item.payload.body;
  return typeof body === "string" ? body.trim() : "";
}

// A string the asking agent put in its payload, or nothing.
function payloadText(item, key) {
  const value = item.payload && item.payload[key];
  return typeof value === "string" ? value.trim() : "";
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

// A receipt reference: a mono id that is a link to what it names.
const ref = (href, label, text) =>
  `<a class="item-ref" href="${esc(href)}" aria-label="${esc(label)}">${esc(text)}</a>`;

// What an item rests on, each a line the strip and the margin both read: the
// event, the session it was asked in, the artifact and commit its payload
// names, and the brain key it points at.
function receipts(item, session) {
  const project = encodeURIComponent(item.project_id);
  const lines = [
    [
      "Event",
      ref(
        `#/projects/${project}/feed?event=${encodeURIComponent(item.event_id)}`,
        `Event ${item.event_id}`,
        shortId(item.event_id),
      ),
    ],
  ];
  if (item.session_id) {
    const name = session?.session_name || shortId(item.session_id);
    const events = Number.isFinite(session?.events) ? ` <span class="item-mono">· ${session.events} events</span>` : "";
    lines.push([
      "Session",
      `${ref(
        `#/projects/${project}/sessions?id=${encodeURIComponent(item.session_id)}`,
        `Session ${name}`,
        name,
      )}${events}`,
    ]);
  }
  const artifact = payloadText(item, "artifact_id");
  if (artifact) {
    const title = payloadText(item, "title");
    lines.push([
      "Artifact",
      ref(
        `#/artifacts/${encodeURIComponent(artifact)}?project=${project}`,
        `Artifact ${title || artifact}`,
        title || shortId(artifact),
      ),
    ]);
  }
  const commit = payloadText(item, "commit");
  if (commit) lines.push(["Commit", `<span class="item-mono">${esc(commit.slice(0, 12))}</span>`]);
  const key = payloadText(item, "brain_key");
  if (key) lines.push(["Brain key", `<span class="item-mono">${esc(key)}</span>`]);
  return lines;
}

const dl = (lines, cls, label) =>
  `<dl class="${cls}" aria-label="${esc(label)}">${lines.map(([term, value]) => `<dt>${esc(term)}</dt><dd>${value}</dd>`).join("")}</dl>`;

// The margin's ledger: the receipts, then when it was asked and what happens
// if the reader stays silent.
function marginReceipts(item, session, snoozedUntil) {
  const lines = receipts(item, session);
  lines.push(["Asked", `${itemStamp(item.created_at)} <span class="item-quiet">· ${esc(relative(item.created_at))}</span>`]);
  const due = deadlineLine(item);
  if (due) lines.push([waits(item) ? "If silent" : "Deadline", esc(due)]);
  if (snoozedUntil) lines.push(["Snoozed", `until ${esc(placeTime(snoozedUntil))}, on this device`]);
  return dl(lines, "item-rcpt", "Receipts");
}

// The other asks from the same session or project, and the artifact the item
// points at, each opened from here.
function relatedList(item, queue) {
  const rows = relatedAsks(item, queue).map(({ item: other }) => {
    const later = queue.later.includes(other);
    const mark = later ? `<span class="item-rel-k is-later" aria-hidden="true">${clock(16)}</span>` : `<span class="item-rel-k">${kindMark(other.kind)}</span>`;
    return `<li>
      ${mark}
      <a class="item-rel-a" href="${esc(itemHref(other.event_id))}">${esc(clip(subjectAndMessage(other.summary).subject, 48))}<span class="sr-only">, ${esc(later ? "can wait" : kindName(other.kind))}</span></a>
      ${itemStamp(other.created_at)}
      <span class="item-rel-s">${esc(other.actor)} · ${esc(projectName(other))}</span>
    </li>`;
  });
  const artifact = payloadText(item, "artifact_id");
  if (artifact) {
    const title = payloadText(item, "title") || `Artifact ${shortId(artifact)}`;
    rows.push(`<li>
      <span class="item-rel-k">${kindMark("artifact")}</span>
      <a class="item-rel-a" href="#/artifacts/${encodeURIComponent(artifact)}?project=${encodeURIComponent(item.project_id)}">${esc(title)}</a>
      <span></span>
      <span class="item-rel-s">linked artifact</span>
    </li>`);
  }
  if (!rows.length) return "";
  return `<section class="item-blk" aria-labelledby="item-related-label">
    <h3 class="section-label" id="item-related-label">Related</h3>
    <ul class="item-related">${rows.join("")}</ul>
  </section>`;
}

// The replies so far, on the shared row anatomy: glyph, the sentence, the time.
function threadHTML(item, replies) {
  if (!replies.length) return "";
  const row = (kind, sentence, at) => `<div class="item-trow">
      <span class="item-tk">${kindMark(kind)}</span>
      <p class="item-ts">${sentence}</p>
      ${itemStamp(at)}
    </div>`;
  return `<section class="item-thread" aria-labelledby="item-thread-label">
    <h3 class="section-label" id="item-thread-label">Thread</h3>
    ${row(item.kind, eventSentenceHTML({ kind: item.kind, actor: item.actor, summary: item.summary }), item.created_at)}
    ${replies.map((reply) => row(reply.kind, replySentenceHTML(reply), reply.created_at)).join("")}
  </section>`;
}

// Snoozing is remembered on this device, the same snooze Now reads.
function snoozeButton(item, snoozedUntil, keyed) {
  const key = keyed ? ' data-screen-key="s"' : "";
  return snoozedUntil
    ? `<button type="button" class="item-ghost" data-item="unsnooze"${key}>Bring back${keyed ? kbd("s") : ""}</button>`
    : `<button type="button" class="item-ghost" data-item="snooze"${key}>Snooze 1h${keyed ? kbd("s") : ""}</button>`;
}

// The verbs at the head of the answer block. The keys are named on them and
// pressed through `data-screen-key`; the phone's thumb bar carries the same
// verbs without keys.
function verbs(item, snoozedUntil) {
  if (item.kind === "approval") {
    return `<button type="button" class="action" data-item="approve" data-screen-key="a">Approve${kbd("a")}</button>
      <button type="button" data-item="decline" data-screen-key="d">Decline${kbd("d")}</button>
      ${snoozeButton(item, snoozedUntil, true)}`;
  }
  if (item.kind === "idea") {
    return `<button type="button" class="action" data-item="resolve" data-screen-key="e">Resolve${kbd("e")}</button>
      <button type="button" data-item="clear">Clear</button>
      ${snoozeButton(item, snoozedUntil, true)}`;
  }
  return "";
}

// Where answering goes, and on a question the one verb its choices leave over.
function afterLine(after, trailing = "") {
  const said = after
    ? `After you answer, the next item opens: <a href="${esc(itemHref(after.event_id))}">${esc(clip(subjectAndMessage(after.summary).subject, 64))}</a>`
    : "After you answer, you go back to Now.";
  return `<div class="item-foot"><p class="item-after">${said}</p>${trailing}</div>`;
}

// The answer block: the verbs, the reply field and where answering goes.
function answerBlock(item, snoozedUntil, after) {
  if (!waits(item)) return "";
  const id = esc(item.event_id);
  let reply = "";
  if (item.kind === "question") reply = `<div class="inbox-reply" data-id="${id}"${optionsAttr(item.payload)}></div>`;
  else if (item.kind === "idea") reply = `<div class="inbox-reply" data-id="${id}" data-kind="idea"></div>`;
  // A question's answers are its choices, so they lead, and its one other
  // verb sits at the end of the line under the field.
  if (item.kind === "question") {
    return `<section class="item-answer is-question" aria-label="Your answer">
      ${reply}
      ${afterLine(after, snoozeButton(item, snoozedUntil, true))}
    </section>`;
  }
  return `<section class="item-answer" aria-label="Your answer">
    <div class="item-acts">${verbs(item, snoozedUntil)}</div>
    ${reply}
    ${afterLine(after)}
  </section>`;
}

// The phone's thumb bar: the deadline line with Snooze, and under it the
// primary verbs at 52px, fixed to the bottom of the sheet.
function thumbBar(item, snoozedUntil) {
  if (!waits(item)) return "";
  const due = deadlineLine(item);
  let mainVerbs = "";
  if (item.kind === "approval") {
    mainVerbs = `<button type="button" data-item="decline">Decline</button><button type="button" class="action" data-item="approve">Approve</button>`;
  } else if (item.kind === "idea") {
    mainVerbs = `<button type="button" data-item="clear">Clear</button><button type="button" class="action" data-item="resolve">Resolve</button>`;
  } else {
    mainVerbs = `<button type="button" class="action" data-item="reply" data-screen-key="r">Reply</button>`;
  }
  return `<div class="item-thumb" role="group" aria-label="Answer">
    <p class="item-thumb-due">${due ? `${clock(16)}<span>${esc(due)}</span>` : `<span>No deadline</span>`}${snoozeButton(item, snoozedUntil, false)}</p>
    <div class="item-thumb-main">${mainVerbs}</div>
  </div>`;
}

const KIND_WORD = { approval: "Approval", question: "Question", idea: "Idea" };

function stateWord(item, outcome, isEnrol, later) {
  if (waits(item) && isEnrol) return `<span class="item-state is-wait"><span class="item-dot" aria-hidden="true"></span>Pending enrolment</span>`;
  if (waits(item) && item.kind === "idea") return `<span class="item-state">Open</span>`;
  // An ask that can wait is still the reader's, but nothing breaks before
  // tomorrow, so it says so in ink rather than copper.
  if (waits(item) && later) return `<span class="item-state">${clock()}Can wait</span>`;
  if (waits(item)) return `<span class="item-state is-wait"><span class="item-dot" aria-hidden="true"></span>Waiting on you</span>`;
  return outcome ? `<span class="item-state">${glyphSvg("resolve", { size: 14, strokeWidth: 2 })}${esc(outcome)}</span>` : "";
}

// The whole item: kind and state, title, byline, receipts, the agent's body,
// the answer block and the thread. An item's body is markdown, so it renders
// through the same renderer as a feed event's body.
async function detail(item, { session, replies, snoozedUntil, after, later }) {
  const body = bodyOf(item);
  const renderedBody = body ? await renderMarkdown(body) : "";
  const isResolved = item.status === "resolved";
  const outcome = isResolved ? outcomeOf(item) : "";
  const answered = replies.some((reply) => reply.kind === "answer");
  const note = isResolved && !answered ? noteOf(item) : "";
  const enrol = enrolOf(item);
  const isEnrol = enrol !== null;
  const due = deadlineLine(item);
  // One rule with the feed's stage: a summary that reads as a subject titles
  // the card, and a summary that is a whole message titles it with its leading
  // sentence and reads the rest as prose under it.
  const { subject, message } = subjectAndMessage(item.summary);
  const asked = Date.parse(item.created_at);
  const byline = [
    `<b>${esc(item.actor)}</b> in ${esc(projectName(item))}`,
    itemStamp(item.created_at),
    Number.isFinite(asked) ? esc(relative(asked)) : "",
  ]
    .filter(Boolean)
    .join(", ");
  const strip = receipts(item, session);
  return `<article class="inbox-detail item-main" aria-labelledby="inbox-detail-title">
    <p class="item-kindline">
      <span class="item-kind">${kindMark(item.kind)}${esc(KIND_WORD[item.kind] || kindName(item.kind))}</span>
      ${stateWord(item, outcome, isEnrol, later)}
      ${due ? `<span class="item-due">${clock()}${esc(due)}</span>` : ""}
    </p>
    <h2 class="item-title" id="inbox-detail-title">${esc(subject)}</h2>
    <p class="item-from inbox-detail-meta">${byline}</p>
    ${dl(strip, "item-rstrip", "Receipts")}
    ${message ? `<p class="inbox-detail-message">${esc(message)}</p>` : ""}
    ${isEnrol ? `<div class="inbox-detail-reason"><strong>Why they are asking:</strong> ${esc(enrol || "No reason was given.")}</div>` : ""}
    ${body ? `<div class="inbox-detail-body markdown-body">${renderedBody}</div>` : ""}
    ${note ? `<div class="inbox-detail-resolved-note"><strong>${outcome === "Approved" || outcome === "Declined" ? "Decision note" : "Answer"}:</strong> ${esc(note)}</div>` : ""}
    ${answerBlock(item, snoozedUntil, after)}
    ${threadHTML(item, replies)}
  </article>`;
}

// The header: a close control, where the item sits among the asks, and the
// previous and next controls that walk them.
function head(item, place) {
  const step = (target, label, drawn, key) =>
    target
      ? `<a class="item-step" href="${esc(itemHref(target.event_id))}" aria-label="${label}" data-screen-key="${key}">${drawn}</a>`
      : `<button type="button" class="item-step" aria-label="${label}" disabled>${drawn}</button>`;
  const title = place.place || outcomeOf(item) || KIND_WORD[item.kind] || kindName(item.kind);
  return `<div class="shell-head item-head">
    <a class="shell-slot shell-back item-close" href="${BACK}" aria-label="Close, back to Now" data-item-close>${glyphSvg("close", { size: 20, strokeWidth: 1.8 })}</a>
    <div class="shell-title">
      <h1 class="shell-title-line">${esc(title)}</h1>
      <span class="shell-meta">${esc(`${projectName(item)} · ${(KIND_WORD[item.kind] || kindName(item.kind)).toLowerCase()}`)}</span>
    </div>
    <div class="item-steps">
      ${step(place.previous, "Previous item", glyphSvg("chevronUp", { size: 18, strokeWidth: 1.8 }), "k")}
      ${step(place.next, "Next item", glyphSvg("chevronDown", { size: 18, strokeWidth: 1.8 }), "j")}
    </div>
  </div>`;
}

// A step is named only where there is an item to step to: the first ask has no
// previous and the last has no next, and j or k on them does nothing.
const controls = (place) => `<div class="shell-controls item-controls">
  <a class="item-back" href="${BACK}" data-item-close>${glyphSvg("chevronBack", { size: 16, strokeWidth: 1.8 })}Now${kbd("esc")}</a>
  <span class="item-hints" aria-hidden="true">${place.previous ? `<span>${kbd("k")} previous</span>` : ""}${place.next ? `<span>${kbd("j")} next</span>` : ""}</span>
</div>`;

// A question's card holds its composer open, as the design draws it.
function mountReply() {
  const slot = main.querySelector(".inbox-reply");
  if (!slot) return;
  const isIdea = /** @type {HTMLElement} */ (slot).dataset.kind === "idea";
  const reply = composer({
    label: isIdea ? "Add an addendum" : "Your answer",
    options: optionsFrom(/** @type {HTMLElement} */ (slot).dataset.options),
    numbered: true,
    send: async (body) => {
      if (isIdea) {
        // An addendum adds to the idea and leaves it open, so the item stays.
        await sendIdeaReply(/** @type {HTMLElement} */ (slot).dataset.id, body);
        await render();
        toast("Addendum logged on the idea's thread.");
      } else {
        await sendAnswer(/** @type {HTMLElement} */ (slot).dataset.id, body);
        await leave(shown.landing);
        toast("Answer sent, recorded on the feed.");
      }
    },
  });
  slot.appendChild(reply.element);
}

// A literal command in the body is lifted out whole: each code block gets a
// row of its own above it, and the copy glyph sits on that row.
function mountCopies() {
  for (const pre of main.querySelectorAll(".inbox-detail-body pre")) {
    if (pre.previousElementSibling?.classList.contains("item-code-row")) continue;
    const lang = (pre.querySelector("code")?.className.match(/language-([\w-]+)/) || [])[1] || "";
    const row = document.createElement("div");
    row.className = "item-code-row";
    const label = document.createElement("span");
    label.className = "item-code-lang";
    label.textContent = lang || "Code";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "hub-btn-glyph item-copy";
    copy.dataset.item = "copy";
    copy.setAttribute("aria-label", "Copy code");
    copy.innerHTML = glyphSvg("copy", { size: 16 });
    row.append(label, copy);
    pre.before(row);
  }
}

// Back to the stream from a card whose item has just been resolved, or that
// the reader closed, or on to the next ask. The card's address is replaced
// rather than pushed, so Back does not return to a card for something that is
// gone.
async function leave(to = BACK) {
  dropCard();
  history.replaceState(history.state, "", to);
  await render();
}

// The session an item was asked in, by name and event count, when the hub
// still has it. The project's list says whether it does, so a pruned session
// is not asked for by id; it leaves the receipt on its id instead.
async function sessionOf(item) {
  if (!item.session_id) return null;
  try {
    const list = await api(`/api/v1/sessions?project=${encodeURIComponent(item.project_id)}`);
    const listed = (list.sessions || []).find((session) => session.id === item.session_id);
    if (!listed) return null;
    return await api(`/api/v1/sessions/${encodeURIComponent(item.session_id)}`).catch(() => listed);
  } catch {
    return null;
  }
}

// The replies on the item's thread. They come after the item, so the project
// feed is read forward from it.
async function repliesOf(item) {
  try {
    const page = await api(
      `/api/v1/projects/${encodeURIComponent(item.project_id)}/feed?since=${encodeURIComponent(item.event_id)}&limit=500`,
    );
    return threadReplies(item.event_id, page.events || []);
  } catch {
    return [];
  }
}

export async function inbox(gen) {
  const id = openId();
  // An item is read from the lists it can be in. Unread items accumulate
  // without expiry, so each set is fetched on its own rather than from one
  // page that could cut the item off the end.
  const sets = await Promise.all(
    ["status=action&limit=500", "status=waiting&limit=500", "unread_only=true&limit=500", "status=read&limit=100", "status=resolved&limit=100"].map(
      (query) => api(`/api/v1/inbox?${query}`),
    ),
  );
  const items = sets.flatMap((set) => set.items || []);
  let opened = items.find((item) => item.event_id === id);
  // Opening an item is reading it.
  if (opened && opened.status === "unread") {
    const marked = await api(`/api/v1/inbox/${encodeURIComponent(opened.event_id)}/read`, { method: "POST" });
    opened = { ...opened, status: marked.status };
  }
  if (stale(gen)) return;
  dropCard();
  if (!opened) {
    // Decided or answered elsewhere, or a link to something pruned.
    history.replaceState(history.state, "", BACK);
    await render();
    return;
  }
  const [session, replies] = await Promise.all([sessionOf(opened), repliesOf(opened)]);
  if (stale(gen)) return;
  const at = Date.now();
  const snoozes = readSnoozes(at);
  const snoozedUntil = snoozes[opened.event_id] || 0;
  const queue = askQueue(items, { now: at, snoozes });
  const place = queuePlace(queue, opened.event_id);
  shown = { id: opened.event_id, summary: opened.summary, landing: place.after ? itemHref(place.after.event_id) : BACK };

  // Esc closes the card unless an answer is half written in it. The words are
  // the card's, so it does not matter whether focus is in the field or on the
  // Send button beside it: closing would throw them away either way.
  unregisterCard = registerPane(() => {
    const draft = /** @type {HTMLInputElement} */ (main.querySelector(".inbox-detail .composer-field"));
    if (draft && draft.value.trim()) return;
    leave().catch(failed);
  });

  const later = queue.later.some((other) => other.event_id === opened.event_id);
  const article = await detail(opened, { session, replies, snoozedUntil, after: place.after, later });
  if (stale(gen)) return;
  const thumb = thumbBar(opened, snoozedUntil);
  paint(
    gen,
    shellHTML({
      noIndex: true,
      segment: "inbox",
      stageHead: head(opened, place),
      stageControls: controls(place),
      stageBody: `<div class="shell-pad item-pad item-sheet${thumb ? " has-thumb" : ""}">
        <div class="item"><div class="item-grid">
          ${article}
          <aside class="item-mar" aria-label="Receipts and related">
            <section class="item-blk item-blk-rcpt" aria-labelledby="item-rcpt-label">
              <h3 class="section-label" id="item-rcpt-label">Receipts</h3>
              ${marginReceipts(opened, session, snoozedUntil)}
            </section>
            ${relatedList(opened, queue)}
          </aside>
        </div></div>
      </div>${thumb}`,
    }),
  );
  if (stale(gen)) return;
  installShellLayout(main);
  mountReply();
  mountCopies();
}

const sendAnswer = (id, body) =>
  api(`/api/v1/questions/${encodeURIComponent(id)}/answer`, {
    method: "POST",
    body: JSON.stringify({ body }),
  });

const sendIdeaReply = (id, summary) =>
  api(`/api/v1/ideas/${encodeURIComponent(id)}/reply`, {
    method: "POST",
    body: JSON.stringify({ summary }),
  });

// Resolve or clear an idea. `landing` is where the screen goes once it has:
// the opened item hands on to the next ask, a row on Now stays where it is.
export async function actOnIdea(id, status, summary, landing = "") {
  await api(`/api/v1/ideas/${encodeURIComponent(id)}/${status === "cleared" ? "clear" : "resolve"}`, {
    method: "POST",
  });
  if (landing) {
    dropCard();
    history.replaceState(history.state, "", landing);
  }
  await render();
  toast(`${status === "cleared" ? "Cleared" : "Resolved"} "${summary || id}".`);
}

function isOfflineError(error) {
  return !navigator.onLine || !error || error.status == null;
}

window.addEventListener("agent-hub:queue-flushed", () => {
  const screen = location.hash.replace(/^#\/?/, "").split(/[/?]/)[0];
  if (screen === "" || screen === "home" || screen === "inbox") render();
});

// Send an answer, or queue it when the hub cannot be reached. True means it
// was queued rather than sent.
export async function answerWith(id, body) {
  try {
    await sendAnswer(id, body);
    return false;
  } catch (error) {
    if (isOfflineError(error)) {
      await enqueue({
        method: "POST",
        url: `/api/v1/questions/${encodeURIComponent(id)}/answer`,
        body: JSON.stringify({ body }),
      });
      return true;
    }
    throw error;
  }
}

// Reply opens a composer under the row it belongs to, so the question stays
// readable while the answer is written and a failed send keeps the words.
// Pressing Reply again on an open composer moves back into it rather than
// stacking a second one. `onSent` runs once the screen has repainted.
/**
 * @param {string} id
 * @param {HTMLElement} button
 * @param {{ onSent?: Function }} [options]
 */
export async function answer(id, button, { onSent } = {}) {
  // A stage's action line is the row there: the composer goes under the line,
  // not into it beside the button.
  const row = button.closest(".row") || button.closest(".feed-stage-actions") || button;
  const open = row.nextElementSibling;
  if (open && open.classList.contains("composer")) {
    /** @type {HTMLElement} */ (open.querySelector(".composer-field")).focus();
    return;
  }
  const reply = composer({
    label: "Your answer",
    options: optionsFrom(button.dataset.options),
    send: async (body) => {
      const queued = await answerWith(id, body);
      if (queued) {
        toast("Queued, will send when online.");
        return;
      }
      await render();
      toast("Answer sent, recorded on the feed.");
      onSent?.();
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

async function decide(id, decision, note, landing) {
  deciding.add(id);
  try {
    return await send(id, decision, note, landing);
  } finally {
    deciding.delete(id);
  }
}

// The note a decision may carry, in the design's words for it. The limit is
// the hub's, which counts the trimmed note's characters and is the one that
// refuses; here it only places the count.
const NOTE = { label: "Add guidance with your decision", limit: 2000 };

// A blank note is no note, so the key is left out rather than sent empty.
async function send(id, decision, note, landing) {
  const body = JSON.stringify(note ? { decision, note } : { decision });
  const url = `/api/v1/approvals/${encodeURIComponent(id)}/decision`;
  try {
    await api(url, { method: "POST", body });
  } catch (error) {
    if (isOfflineError(error)) {
      await enqueue({ method: "POST", url, body });
      return true;
    }
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
  // A decision made on an opened item hands on to the next ask.
  if (landing) {
    dropCard();
    history.replaceState(history.state, "", landing);
  }
  await render();
  return false;
}

// A decision is recorded on the feed and leaves the waiting queue, so the
// dialog names what is decided and the toast states the result. The request
// is sent with the dialog still open: a refusal lands beside the note rather
// than after the words are gone.
async function ask(id, decision, { title, danger, tone, done, landing }) {
  if (deciding.has(id)) return;
  let noted = false;
  let queued = false;
  const confirmed = await confirmAction({
    title,
    body: "Your decision is recorded on the feed and resolves the waiting item.",
    safe: "Not now",
    danger,
    tone,
    field: NOTE,
    commit: async (note) => {
      noted = !!note;
      queued = await decide(id, decision, note, landing);
    },
  });
  if (confirmed) {
    if (queued) toast("Queued, will send when online.");
    else toast(`${done}${noted ? " with your note" : ""}, recorded on the feed.`);
  }
}

export const approve = (id, summary, { landing = "" } = {}) =>
  ask(id, "approve", {
    title: summary ? `Approve "${summary}"?` : "Approve this action?",
    danger: "Approve",
    tone: "action",
    done: "Approved",
    landing,
  });

// The other answer to the same question, asked the same way.
export const declineApproval = (id, summary, { landing = "" } = {}) =>
  ask(id, "decline", {
    title: summary ? `Decline "${summary}"?` : "Decline this action?",
    danger: "Decline",
    tone: "danger",
    done: "Declined",
    landing,
  });

const failed = (error) => toast(`Nothing changed: ${error.message}`);

// Put a code block's text on the clipboard. The glyph is the confirmation: it
// becomes a check for a moment and the control says it copied.
async function copyCode(button) {
  const pre = button.closest(".item-code-row")?.nextElementSibling;
  if (!pre) return;
  try {
    await navigator.clipboard?.writeText(pre.textContent);
  } catch {}
  button.innerHTML = glyphSvg("resolve", { size: 16, strokeWidth: 2 });
  button.classList.add("is-copied");
  button.setAttribute("aria-label", "Copied");
  setTimeout(() => {
    button.innerHTML = glyphSvg("copy", { size: 16 });
    button.classList.remove("is-copied");
    button.setAttribute("aria-label", "Copy code");
  }, 1400);
}

// The card's verbs, on the answer block and the phone's thumb bar alike.
main.addEventListener("click", (event) => {
  const button = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("button[data-item]"));
  if (!button || !main.querySelector(".item-sheet")) return;
  const { id, summary, landing } = shown;
  const verb = button.dataset.item;
  let work = null;
  if (verb === "copy") work = copyCode(button);
  else if (verb === "snooze") {
    snooze(id);
    work = leave(landing).then(() =>
      toast(`Snoozed "${summary || id}" for 1 hour, on this device.`, () => {
        unsnooze(id);
        render().catch(failed);
      }),
    );
  } else if (verb === "unsnooze") {
    unsnooze(id);
    work = render().then(() => toast(`Brought "${summary || id}" back.`));
  } else if (verb === "approve") work = approve(id, summary, { landing });
  else if (verb === "decline") work = declineApproval(id, summary, { landing });
  else if (verb === "resolve") work = actOnIdea(id, "resolved", summary, landing);
  else if (verb === "clear") work = actOnIdea(id, "cleared", summary, landing);
  else if (verb === "reply") {
    const field = /** @type {HTMLElement} */ (main.querySelector(".inbox-detail .composer-field"));
    field?.scrollIntoView({ block: "center" });
    field?.focus();
  }
  if (work) work.catch(failed);
});

// The close controls leave the way Esc does: the card's address is replaced,
// so Back does not walk into a card the reader closed. A press meant for
// another tab or window is the browser's.
main.addEventListener("click", (event) => {
  if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  if (!/** @type {HTMLElement} */ (event.target).closest("a[data-item-close]")) return;
  event.preventDefault();
  leave().catch(failed);
});
