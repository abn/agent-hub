// One inbox item at full size, and the verbs that clear an item wherever it is
// shown: answer, decide, resolve or clear an idea. The list of what waits is
// Now; the Inbox's address opens Now showing what needs the reader, and an
// item's own address, `#/inbox?open=<id>`, opens it here.

import { api } from "./api.mjs";
import { enqueue } from "./offline-queue.mjs";
import { composer } from "./composer.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, glyph, main, optionsAttr, optionsFrom, paint, projectName, stale } from "./dom.mjs";
import { registerPane } from "./keys.mjs";
import { expiredWords, subjectAndMessage } from "./feed.mjs";
import { readSnoozes, snooze, unsnooze } from "./now-model.mjs";
import { render } from "./router.mjs";
import { renderMarkdown } from "./sessions.mjs";
import { installShellLayout, shellHTML, shellStageHead } from "./shell-layout.mjs";
import { deadlineWords, fullStamp, relative } from "./time.mjs";
import { toast } from "./toast.mjs";

// Where closing an item goes: the stream it was opened from.
const BACK = "#/home";

const waits = (item) => item.status === "action" || item.status === "waiting";

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

// A plain time rather than the pressable one: the whole stamp is the title,
// and it is what a reader hears.
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

// Snoozing is remembered on this device, the same snooze Now reads.
function snoozeControl(item) {
  const id = esc(item.event_id);
  const summary = esc(item.summary);
  return readSnoozes()[item.event_id]
    ? `<button type="button" class="inbox-quiet" data-action="inbox-unsnooze" data-id="${id}" data-summary="${summary}">Bring back</button>`
    : `<button type="button" class="inbox-quiet" data-action="inbox-snooze" data-id="${id}" data-summary="${summary}" title="Snooze for 1 hour">Snooze 1h</button>`;
}

// The medium card: the whole of one item, with its answers at full size. An
// item's body is markdown, so it renders through the same renderer as a feed
// event's body rather than showing the syntax the agent wrote it in.
async function detail(item) {
  const id = esc(item.event_id);
  const body = bodyOf(item);
  const renderedBody = body ? await renderMarkdown(body) : "";
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
      <button type="button" data-action="inbox-detail-decline" data-id="${id}" data-summary="${esc(item.summary)}">Decline</button>
    </div>`;
  } else if (waits(item) && item.kind === "question") {
    answers = `<div class="inbox-reply" data-id="${id}"${optionsAttr(item.payload)}></div>`;
  } else if (waits(item) && item.kind === "idea") {
    answers = `<div class="inbox-answers">
      <button type="button" class="action" data-action="inbox-detail-resolve" data-id="${id}" data-summary="${esc(item.summary)}">Resolve</button>
      <button type="button" data-action="inbox-detail-clear" data-id="${id}" data-summary="${esc(item.summary)}">Clear</button>
    </div>
    <div class="inbox-reply" data-id="${id}" data-kind="idea"></div>`;
  }
  return `<article class="inbox-detail" aria-labelledby="inbox-detail-title">
    <a class="inbox-back" href="${BACK}"><span class="inbox-back-arrow" aria-hidden="true">&larr; </span>Back to Now</a>
    <div class="inbox-detail-head">
      ${glyph(item.kind)}
      ${waits(item) && item.kind !== "idea" ? '<span class="pill pill-status"><span class="pill-dot" aria-hidden="true"></span>Waiting on you</span>' : ""}
      ${item.kind === "idea" ? '<span class="pill pill-status"><span class="pill-dot" aria-hidden="true"></span>Idea</span>' : ""}
      ${waits(item) && isEnrol ? '<span class="pill pill-enrol">Pending enrolment</span>' : ""}
      ${outcome ? `<span class="pill pill-outcome">${esc(outcome)}</span>` : ""}
      <span class="inbox-detail-meta"><span class="inbox-project">${esc(projectName(item))}</span> · ${esc(item.actor)} · ${stamp(item.updated_at)}</span>
    </div>
    <h2 class="item-title" id="inbox-detail-title">${esc(subject)}</h2>
    ${deadline ? `<p class="inbox-detail-deadline">${esc(deadline)}</p>` : ""}
    ${message ? `<p class="inbox-detail-message">${esc(message)}</p>` : ""}
    ${isEnrol ? `<div class="inbox-detail-reason"><strong>Why they are asking:</strong> ${esc(enrol || "No reason was given.")}</div>` : ""}
    ${body ? `<div class="inbox-detail-body markdown-body">${renderedBody}</div>` : ""}
    ${note ? `<div class="inbox-detail-resolved-note"><strong>${outcome === "Approved" || outcome === "Declined" ? "Decision note" : "Answer"}:</strong> ${esc(note)}</div>` : ""}
    ${answers}
    ${waits(item) ? `<div class="inbox-detail-snooze-wrap">${snoozeControl(item)}</div>` : ""}
  </article>`;
}

// A question's card holds its composer open, as the design draws it.
function mountReply() {
  const slot = main.querySelector(".inbox-reply");
  if (!slot) return;
  const isIdea = slot.dataset.kind === "idea";
  const reply = composer({
    label: isIdea ? "Add an addendum" : "Your answer",
    options: optionsFrom(slot.dataset.options),
    send: async (body) => {
      if (isIdea) {
        await sendIdeaReply(slot.dataset.id, body);
        await leave();
        toast("Addendum logged on the idea's thread.");
      } else {
        await sendAnswer(slot.dataset.id, body);
        await leave();
        toast("Answer sent, recorded on the feed.");
      }
    },
  });
  slot.appendChild(reply.element);
}

// Back to the stream from a card whose item has just been resolved, or that
// the reader closed. The card's address is replaced rather than pushed, so
// Back does not return to a card for something that is gone.
async function leave() {
  dropCard();
  history.replaceState(history.state, "", BACK);
  await render();
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
  let opened = sets.flatMap((set) => set.items || []).find((item) => item.event_id === id);
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
  // Esc closes the card unless an answer is half written in it. The words are
  // the card's, so it does not matter whether focus is in the field or on the
  // Send button beside it: closing would throw them away either way.
  unregisterCard = registerPane(() => {
    const draft = main.querySelector(".inbox-detail .composer-field");
    if (draft && draft.value.trim()) return;
    leave().catch(failed);
  });

  paint(
    gen,
    shellHTML({
      noIndex: true,
      segment: "inbox",
      stageHead: shellStageHead(
        opened.summary,
        `${projectName(opened)} · ${opened.actor} · ${relative(opened.updated_at)}`,
        "",
        BACK,
      ),
      stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(
        `${projectName(opened)} / inbox / ${opened.kind}`,
      )}</span></div>`,
      stageBody: await detail(opened),
    }),
  );
  if (stale(gen)) return;
  installShellLayout(main);
  mountReply();
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

export async function actOnIdea(id, status, summary) {
  await api(`/api/v1/ideas/${encodeURIComponent(id)}/${status === "cleared" ? "clear" : "resolve"}`, {
    method: "POST",
  });
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
export async function answer(id, button, { onSent } = {}) {
  // A stage's action line is the row there: the composer goes under the line,
  // not into it beside the button.
  const row = button.closest(".row") || button.closest(".feed-stage-actions") || button;
  const open = row.nextElementSibling;
  if (open && open.classList.contains("composer")) {
    open.querySelector(".composer-field").focus();
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

async function decide(id, decision, note) {
  deciding.add(id);
  try {
    return await send(id, decision, note);
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
  await render();
  return false;
}

// A decision is recorded on the feed and leaves the waiting queue, so the
// dialog names what is decided and the toast states the result. The request
// is sent with the dialog still open: a refusal lands beside the note rather
// than after the words are gone.
async function ask(id, decision, { title, danger, tone, done }) {
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
      queued = await decide(id, decision, note);
    },
  });
  if (confirmed) {
    if (queued) toast("Queued, will send when online.");
    else toast(`${done}${noted ? " with your note" : ""}, recorded on the feed.`);
  }
}

export const approve = (id, summary) =>
  ask(id, "approve", {
    title: summary ? `Approve "${summary}"?` : "Approve this action?",
    danger: "Approve",
    tone: "action",
    done: "Approved",
  });

// The other answer to the same question, asked the same way.
export const declineApproval = (id, summary) =>
  ask(id, "decline", {
    title: summary ? `Decline "${summary}"?` : "Decline this action?",
    danger: "Decline",
    tone: "danger",
    done: "Declined",
  });

const failed = (error) => toast(`Nothing changed: ${error.message}`);

// The card's verbs. The entry point routes Reply and Approve on a feed row;
// what the card adds is handled here, beside the code it calls.
main.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action^='inbox-']");
  if (!button) return;
  const { action, id, summary } = button.dataset;
  let work = null;
  if (action === "inbox-snooze") {
    snooze(id);
    work = leave().then(() =>
      toast(`Snoozed "${summary || id}" for 1 hour, on this device.`, () => {
        unsnooze(id);
        render().catch(failed);
      }),
    );
  } else if (action === "inbox-unsnooze") {
    unsnooze(id);
    work = render().then(() => toast(`Brought "${summary || id}" back.`));
  } else if (action === "inbox-detail-decline") {
    work = declineApproval(id, summary);
  } else if (action === "inbox-detail-approve") {
    work = approve(id, summary);
  } else if (action === "inbox-detail-resolve") {
    work = actOnIdea(id, "resolved", summary);
  } else if (action === "inbox-detail-clear") {
    work = actOnIdea(id, "cleared", summary);
  }
  if (work) work.catch(failed);
});

// The card's close control leaves the way Esc does: the card's address is
// replaced, so Back does not walk into a card the reader closed. A press meant
// for another tab or window is the browser's.
main.addEventListener("click", (event) => {
  if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  if (!event.target.closest(".inbox-detail a.inbox-back")) return;
  event.preventDefault();
  leave().catch(failed);
});
