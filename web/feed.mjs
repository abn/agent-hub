// The project feed: one project's events, day-grouped and filterable by kind.
// Today and Yesterday are open, older days sit behind the Earlier disclosure,
// and what landed since the reader last looked carries the unread mark.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { actionFor, esc, glyph, isOpen, main, waitingMarks, when } from "./dom.mjs";
import { emptyStateHTML, EMPTY_COPY } from "./empty.mjs";
import { registerPane } from "./keys.mjs";
import { focusAfterRender, render } from "./router.mjs";
import { byDay } from "./time.mjs";
import { toast } from "./toast.mjs";

// Feed chips cover the kinds a reader filters by. Anchors and scores are
// session-scoped, but they are ordinary feed events and read here too.
const FEED_KINDS = [
  { kind: "question", label: "Questions" },
  { kind: "approval", label: "Approvals" },
  { kind: "finished", label: "Finished" },
  { kind: "signal", label: "Signals" },
  { kind: "anchor", label: "Anchors" },
  { kind: "score", label: "Scores" },
];
const VALID_KINDS = new Set(FEED_KINDS.map((k) => k.kind));
const ALL = "all";
// One page of the feed. Older pages are asked for with the hub's own cursor.
const PAGE = 100;
const RECENT = ["Today", "Yesterday"];
// The width of the Dismiss action a left swipe uncovers, and how far a drag has
// to move before it counts as along one axis rather than a tap.
const ACTION_WIDTH = 88;
const SLOP = 8;
// The most ids one dismissal request may carry, matching FEED_DISMISS_MAX.
const DISMISS_MAX = 500;

// The kind filters are per project, so switching projects does not carry a
// filter across. They are view state, not a saved preference.
const projectFilters = new Map();

// The event the shell's stage is showing, so the index can mark its row. The
// shell sets it before it renders the list.
let selectedEvent = "";
export function setFeedSelection(id) {
  selectedEvent = id || "";
}

// What one stay on a project's feed holds. The baseline is the read cursor as
// it stood on arrival: the hub's cursor moves as soon as the feed is seen, and
// marking against the moving one would wipe the dots on the next repaint.
const visits = new Map();

// Selection mode: the events picked in the loaded page, and the project they
// belong to. It is view state, held here rather than in the address, because a
// pick is a step towards one action rather than a place the reader can return
// to. Leaving the screen ends it.
const picking = { active: false, project: "", ids: new Set(), opener: null };

function endPicking({ focus = false } = {}) {
  const opener = picking.opener;
  picking.active = false;
  picking.project = "";
  picking.ids.clear();
  picking.opener = null;
  const shell = main.querySelector(".shell[data-segment]");
  shell?.classList.remove("is-picking");
  for (const row of main.querySelectorAll(".shell-controls, .project-tools-mobile")) {
    row.classList.remove("is-picking", "mobile-filter-open");
  }
  for (const bar of main.querySelectorAll(".feed-select-bar")) bar.hidden = true;
  for (const box of main.querySelectorAll(".feed-pick")) box.checked = false;
  for (const row of main.querySelectorAll(".feed-row.picked")) row.classList.remove("picked");
  concealSwipe();
  if (!focus) return;
  const target =
    opener && opener.isConnected && opener.getClientRects().length
      ? opener
      : [...main.querySelectorAll(".proj-overflow-btn")].find((button) => button.getClientRects().length);
  target?.focus({ preventScroll: true });
}

window.addEventListener("hashchange", () => {
  if (picking.active && feedProject(location.hash) !== picking.project) endPicking();
});

function feedProject(hash) {
  const parts = (hash.replace(/^#/, "").split("?")[0] || "").split("/");
  if (parts[1] !== "projects" || !parts[2]) return "";
  return !parts[3] || parts[3] === "feed" ? decodeURIComponent(parts[2]) : "";
}

// Leaving the feed ends the visit, so coming back marks against the cursor
// the last visit left behind.
window.addEventListener("hashchange", () => {
  const staying = feedProject(location.hash);
  for (const id of [...visits.keys()]) if (id !== staying) visits.delete(id);
});

export function activeKinds(projectId) {
  const kinds = projectFilters.get(projectId) || [];
  return new Set(kinds.filter((k) => VALID_KINDS.has(k)));
}

// The kind filter, folded into the index's Group menu. Round 5's rules hold:
// the trigger's label is a fixed word rather than the current value, and the
// value rides beside it as a pill. The menu items are the same control the
// chip row used, so one handler serves both.
const CHEVRON_DOWN = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M 6 9l6 6 6-6"></path></svg>`;

export function kindMenu(active, counts = {}) {
  const entries = [
    { kind: ALL, label: "All", count: counts.total },
    ...FEED_KINDS.map(({ kind, label }) => ({ kind, label, count: counts[kind] })),
  ];
  const items = entries
    .map(({ kind, label, count }) => {
      const pressed = kind === ALL ? active.size === 0 : active.has(kind);
      const n = count == null ? "" : ` <span class="mono shell-menu-n">${count}</span>`;
      return `<button type="button" role="menuitemradio" aria-checked="${pressed}" data-action="kind" data-kind="${kind}">${esc(label)}${n}</button>`;
    })
    .join("");
  const chosen = FEED_KINDS.filter(({ kind }) => active.has(kind)).map(({ label }) => label);
  const pill = chosen.length ? `<span class="pill shell-group-pill">${esc(chosen.join(", "))}</span>` : "";
  return `<div class="shell-group">
    <button type="button" class="hub-group-toggle" data-group-toggle aria-haspopup="menu" aria-expanded="false">Group${CHEVRON_DOWN}</button>
    ${pill}
    <div class="shell-group-menu" role="menu" aria-label="Filter by kind" data-group-menu hidden>${items}</div>
  </div>`;
}

export function kindChips(active, counts = {}) {
  const total = counts.total ?? 0;
  const chip = (kind, label, count, pressed) => {
    const dot = kind === ALL ? "" : `<span class="chip-dot" aria-hidden="true"></span>`;
    return `<button type="button" class="chip" data-action="kind" data-kind="${kind}" aria-pressed="${pressed}">${dot}${label} · ${count}</button>`;
  };
  const chips = FEED_KINDS
    .filter(({ kind }) => (counts[kind] ?? 0) > 0)
    .map(({ kind, label }) => chip(kind, label, counts[kind] ?? 0, active.has(kind)))
    .join("");
  const asideToggle = `<button type="button" class="aside-toggle" data-action="aside-toggle" aria-expanded="false" aria-label="Toggle details">Details</button>`;
  return `<div class="feed-chips" role="group" aria-label="Filter by kind">${chip(ALL, "All", total, active.size === 0)}${chips}<span class="feed-chips-spacer"></span>${asideToggle}</div>`;
}

// The row that replaces the filter's row while the reader is picking: the count
// they built, the verb that spends it, and the way out. The frame holds: it is
// the same control row, not a new band over the list.
export function feedSelectBar() {
  return `<div class="feed-select-bar" hidden>
    <span class="feed-select-count" role="status" aria-live="polite">0 selected</span>
    <button type="button" class="btn-outline" data-action="feed-dismiss-picked" disabled>Dismiss</button>
    <button type="button" class="btn-outline" data-action="feed-select-cancel">Cancel</button>
  </div>`;
}

// The dot is a colour and a shape, so the word sits beside it for a reader
// who gets neither.
const UNREAD = `<span class="dot-unread" aria-hidden="true"></span><span class="sr-only">Unread</span>`;

// The note a human left with a decision, on the row of the decision's own
// event. The word before it is this module's, picked by the payload's
// `decision`; the note is whatever was typed, so it goes through `esc`.
const DECIDED = { approved: "Approved", declined: "Declined" };

// A resolution the hub recorded because the asking agent's deadline passed.
// It is said in words, apart from a human's decision, wherever one is shown.
export function expiredWords(decision) {
  if (decision === "approved") return "Expired: approved itself, no one decided in time";
  if (decision === "declined") return "Expired: declined itself, no one decided in time";
  return "Expired: closed with no answer";
}

function decisionNote(event) {
  const payload = event.payload;
  if (event.kind === "answer" && payload && payload.expired === true) {
    return `<div class="feed-note">${esc(expiredWords(payload.decision))}</div>`;
  }
  const note = payload && typeof payload.note === "string" ? payload.note.trim() : "";
  if (!note) return "";
  const word = DECIDED[payload.decision] || "Note";
  return `<div class="feed-note">${word}: ${esc(note)}</div>`;
}

// Where a feed event leads, when it names an entity the app can show.
// An artifact event opens that artifact in the viewer in its project.
// Kinds without a destination or deleted artifacts return no address.
export function feedDestination(event) {
  if (
    event.kind === "artifact" &&
    event.payload &&
    typeof event.payload.artifact_id === "string" &&
    event.payload.artifact_id &&
    event.payload.action !== "deleted"
  ) {
    const project = event.project_id || "";
    return `#/artifacts/${encodeURIComponent(event.payload.artifact_id)}?project=${encodeURIComponent(project)}`;
  }
  return "";
}

// One rule for what an event's summary is, wherever it is read. A summary
// that is a single line of at most this many characters is a subject: it is
// what the item is called, and it renders as the item title with no body.
// Anything longer, or carrying a newline, is a message rather than a subject,
// and painting it as a heading is what turned a finished event's report into a
// wall of 600-weight text.
export const SUBJECT_MAX = 100;

// A summary, split into the subject a heading can carry and the message that
// belongs under it. The two are disjoint and together hold every character of
// the summary, so a long one loses nothing when it stops being a heading. The
// subject stops where the text itself offers a boundary, so it ends on a phrase
// rather than inside one, and it is marked with an ellipsis wherever it was cut
// rather than ended, so the reader can tell a shortened subject from a whole
// one.
export function subjectAndMessage(summary) {
  const text = String(summary == null ? "" : summary).trim();
  if (!text) return { subject: "", message: "" };
  if (text.length <= SUBJECT_MAX && !text.includes("\n")) return { subject: text, message: "" };

  // A summary written as more than one line already says which line is the
  // subject: the first one. The rest is its message, keeping its line breaks.
  if (text.includes("\n")) {
    const split = text.indexOf("\n");
    return { subject: clampSubject(text.slice(0, split).trim()), message: text.slice(split + 1).trim() };
  }

  // One line, too long to be a subject. It is titled by the last sentence that
  // ends inside the budget, or by the last clause boundary inside it, or failing
  // both by the opening words clamped at the budget. Whichever point it stops
  // at, the message picks up there, so no word is dropped or read twice.
  const stop = text.slice(0, SUBJECT_MAX).search(/[.!?](?=\s|$)/);
  if (stop !== -1) {
    return { subject: `${text.slice(0, stop + 1).trim()}…`, message: text.slice(stop + 1).trim() };
  }
  const clause = lastClauseBoundary(text.slice(0, SUBJECT_MAX));
  const space = text.lastIndexOf(" ", SUBJECT_MAX);
  const cut = clause > 0 ? clause : space > 0 ? space : SUBJECT_MAX;
  return { subject: `${text.slice(0, cut).trimEnd()}…`, message: text.slice(cut).trim() };
}

// The last point inside the window where the text itself pauses: a colon, a
// semicolon, a comma, or a dash standing on its own between spaces. A hyphen
// inside a word is not one of them, so the dash has to hold a space on each
// side. The cut is just past the punctuation, so the subject keeps it.
function lastClauseBoundary(window) {
  const marks = /[:;,]|\s[\u2013\u2014-]\s/g;
  let cut = 0;
  for (let at = marks.exec(window); at; at = marks.exec(window)) {
    // Just past the clause mark, so the subject keeps its punctuation and the
    // message starts after it; a spaced dash leaves a space that trimEnd drops.
    if (at.index > 0) cut = at.index + at[0].length;
  }
  return cut;
}

// A subject cut to the budget, on a word boundary where there is one.
function clampSubject(text) {
  if (text.length <= SUBJECT_MAX) return text;
  const space = text.lastIndexOf(" ", SUBJECT_MAX);
  return `${text.slice(0, space > 0 ? space : SUBJECT_MAX).trimEnd()}…`;
}

export function formatEventSummary(event) {
  const summary = event.summary || "";
  if (event.kind === "signal") {
    return summary;
  }
  if (event.kind === "question") {
    if (/^asked:\s*/i.test(summary)) return `asked: ${summary.replace(/^asked:\s*/i, "")}`;
    return `asked: ${summary}`;
  }
  if (event.kind === "answer") {
    if (event.payload && event.payload.expired === true) {
      return `expired: ${summary.replace(/^re:\s*/i, "")}`;
    }
    if (/^re:\s*/i.test(summary)) return `answered ${summary.replace(/^re:\s*/i, "")}`;
    if (/^answered\s+/i.test(summary)) return `answered ${summary.replace(/^answered\s+/i, "")}`;
    return `answered ${summary}`;
  }
  if (event.kind === "session") {
    const started = summary.match(/^session\s+(.+?)\s+started$/i);
    if (started) return `started ${started[1]}`;
    const ended = summary.match(/^session\s+(.+?)\s+ended$/i);
    if (ended) return `ended ${ended[1]}`;
    const picked = summary.match(/^session\s+(.+?)\s+picked up from\s+(.+)$/i);
    if (picked) return `picked up ${picked[1]} from ${picked[2]}`;
    const forked = summary.match(/^session\s+(.+?)\s+forked from\s+(.+)$/i);
    if (forked) return `forked ${forked[1]} from ${forked[2]}`;
    const reassigned = summary.match(/^session\s+(.+?)\s+reassigned to\s+(.+)$/i);
    if (reassigned) return `reassigned ${reassigned[1]} to ${reassigned[2]}`;
    const capVerb = summary.match(/^([A-Z][a-z]+ed)\s+(.*)$/);
    if (capVerb) return `${capVerb[1].toLowerCase()} ${capVerb[2]}`;
    return summary;
  }
  if (event.kind === "artifact") {
    const capVerb = summary.match(/^([A-Z][a-z]+ed)\s+(.*)$/);
    if (capVerb) return `${capVerb[1].toLowerCase()} ${capVerb[2]}`;
    return summary;
  }
  if (event.kind === "approval") {
    if (/^re:\s*/i.test(summary)) return `decided ${summary.replace(/^re:\s*/i, "")}`;
    const capVerb = summary.match(/^([A-Z][a-z]+ed)\s+(.*)$/);
    if (capVerb) return `${capVerb[1].toLowerCase()} ${capVerb[2]}`;
    return summary;
  }
  if (event.kind === "finished") {
    const capVerb = summary.match(/^([A-Z][a-z]+ed)\s+(.*)$/);
    if (capVerb) return `${capVerb[1].toLowerCase()} ${capVerb[2]}`;
    return summary;
  }
  return summary;
}

function isArtifactPublish(e) {
  return (
    e.kind === "artifact" &&
    (e.payload?.action === "published" || /^published\s+/i.test(e.summary || ""))
  );
}

function collapseDayEvents(events) {
  const result = [];
  let i = 0;
  while (i < events.length) {
    const event = events[i];
    if (
      event.kind === "finished" &&
      /artifacts?\s+published/i.test(event.summary || "") &&
      i + 1 < events.length &&
      isArtifactPublish(events[i + 1])
    ) {
      i++;
      continue;
    }
    if (isArtifactPublish(event)) {
      const cluster = [event];
      let j = i + 1;
      while (
        j < events.length &&
        isArtifactPublish(events[j]) &&
        events[j].actor === event.actor &&
        Math.abs(Date.parse(event.created_at) - Date.parse(events[j].created_at)) <= 120000
      ) {
        cluster.push(events[j]);
        j++;
      }
      if (cluster.length >= 3) {
        result.push({
          collapsed: true,
          count: cluster.length,
          kind: "artifact",
          actor: event.actor,
          created_at: event.created_at,
          id: event.id,
          project_id: event.project_id || cluster[0].project_id,
          events: cluster,
        });
        i = j;
        continue;
      }
    }
    result.push(event);
    i++;
  }
  return result;
}

// A row's meta line: the waiting pill when there is one, then the agent and
// the time. A long agent name is what gives way, so the time always shows.
function metaLine(actor, ts, pill = "") {
  return `<div class="meta">${pill}<span class="meta-actor">${esc(actor)}</span><span aria-hidden="true">·</span>${when(ts)}</div>`;
}

// The kind glyph gives way to the checkbox while the reader is picking, in the
// same slot so the title's left edge does not move. The box carries every event
// id the row stands for: a collapsed artifact row stands for its whole cluster.
function pickBox(ids, label) {
  return `<label class="feed-pick-wrap"><input type="checkbox" class="feed-pick" data-ids="${esc(ids)}" aria-label="Select ${esc(label)}"></label>`;
}

// The action a left swipe uncovers. It sits under the row and stays out of the
// tab ring and the accessibility tree until the swipe shows it; on a fine
// pointer the same verb is reached through Select items.
function dismissTray() {
  return `<div class="swipe-tray swipe-tray-actions swipe-tray-dismiss" hidden>
    <button type="button" class="action" data-action="feed-dismiss">Dismiss</button>
  </div>`;
}

// A row and the tray under it. Every feed row is dismissible, including the
// collapsed artifact row, so the item is what the gesture and the pick act on.
function feedItem(project, ids, rowHTML) {
  return `<div class="feed-item" data-id="${esc(String(ids).split(" ")[0])}" data-ids="${esc(ids)}" data-project="${esc(project)}">
    ${dismissTray()}
    ${rowHTML}
  </div>`;
}

function collapsedArtifactRow(item, baseline) {
  const unread = item.events.some((e) => e.id > baseline);
  const project = item.project_id || "";
  const href = `#/projects/${encodeURIComponent(project)}/artifacts`;
  const title = `<a class="title feed-link" href="${esc(href)}">published ${item.count} artifacts</a>`;
  const ids = item.events.map((event) => event.id).join(" ");
  const row = `<div class="row feed-row${unread ? " unread" : ""}">
    ${pickBox(ids, `published ${item.count} artifacts`)}
    ${glyph("artifact")}
    <div class="grow" data-id="${esc(item.id)}">
      ${title}
      ${metaLine(item.actor, item.created_at)}
    </div>
    ${unread ? UNREAD : ""}
  </div>`;
  return feedItem(project, ids, row);
}

// The feed's own row. Event ids sort by age, which is how the hub compares
// them to the cursor too, so an id above the baseline is an unseen event.
// A row opens the event in the stage; the artifact or session it is about is
// reached from the stage's own card, so the row is a door to the item and not
// straight past it.
function threadRepliesHTML(replies) {
  if (!replies.length) return "";
  return replies
    .slice()
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at))
    .map(
      (reply) => `
    <div class="feed-thread-reply">
      <div class="feed-thread-meta">${esc(reply.actor)} · ${when(reply.created_at)}</div>
      <div class="feed-thread-body">${esc(
        typeof reply.payload?.body === "string" && reply.payload.body.trim()
          ? reply.payload.body.trim()
          : formatEventSummary(reply),
      )}</div>
    </div>`,
    )
    .join("");
}

function feedRow(event, baseline, selected = false, replies = []) {
  const unread = event.id > baseline;
  const project = event.project_id || "";
  const href = `#/projects/${encodeURIComponent(project)}/feed?event=${encodeURIComponent(event.id)}`;
  const summaryText = formatEventSummary(event);
  // Waiting outranks unread, so a waiting row ends in the action dot alone.
  const waiting = isOpen(event) ? waitingMarks(event.id) : null;
  const title = `<a class="title feed-link" href="${esc(href)}"${waiting ? waiting.described : ""}>${esc(summaryText)}</a>`;
  const row = `<div class="row feed-row${unread ? " unread" : ""}${waiting ? " is-waiting" : ""}${selected ? " selected" : ""}" data-id="${esc(event.id)}">
    ${pickBox(event.id, summaryText)}
    ${glyph(event.kind)}
    <div class="grow">
      ${title}
      ${decisionNote(event)}
      ${metaLine(event.actor, event.created_at, waiting ? waiting.pill : "")}
      ${threadRepliesHTML(replies)}
    </div>
    ${waiting ? waiting.dot : unread ? UNREAD : ""}
  </div>`;
  return feedItem(project, event.id, row);
}

function dayGroups(groups, baseline, selectedId = "") {
  // One reply map for the whole loaded set, so a reply and its root are
  // matched even if they land on different days inside the feed.
  const loaded = groups.flatMap((group) => collapseDayEvents(group.events));
  const repliesByRoot = new Map();
  for (const item of loaded) {
    if (
      (item.kind === "answer" || item.kind === "idea") &&
      item.thread_id &&
      item.thread_id !== item.id
    ) {
      const key = item.thread_id;
      if (!repliesByRoot.has(key)) repliesByRoot.set(key, []);
      repliesByRoot.get(key).push(item);
    }
  }
  // A root that is actually present on the loaded page owns its replies; any
  // answer whose root is not on the loaded page falls back to its own row.
  const rootIds = new Set(
    loaded
      .filter(
        (item) =>
          !(
            (item.kind === "answer" || item.kind === "idea") &&
            item.thread_id &&
            item.thread_id !== item.id
          ),
      )
      .map((item) => item.id),
  );
  return groups
    .map((group) => {
      const events = collapseDayEvents(group.events);
      const repliesFor = (id) =>
        repliesByRoot.has(id)
          ? repliesByRoot.get(id).sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at))
          : [];
      const visible = events
        .filter(
          (item) =>
            !(
              (item.kind === "answer" || item.kind === "idea") &&
              item.thread_id &&
              item.thread_id !== item.id
            ),
        )
        .map((item) =>
          item.collapsed
            ? collapsedArtifactRow(item, baseline)
            : feedRow(item, baseline, item.id === selectedId, repliesFor(item.id)),
        );
      const fallback = events
        .filter((item) => item.kind === "answer" || item.kind === "idea")
        .filter((item) => item.thread_id && item.thread_id !== item.id)
        .filter((item) => !rootIds.has(item.thread_id))
        .map((item) => feedRow(item, baseline, item.id === selectedId));
      const rows = visible.concat(fallback);
      // A day with nothing to show (for instance every row below was a reply)
      // must not paint an empty header and an empty card.
      if (!rows.length) return "";
      return `<h2 class="day">${esc(group.label)}</h2>
         <div class="card feed-day">${rows.join("")}</div>`;
    })
    .filter((html) => html !== "")
    .join("");
}

// How many events the fold holds, when that is a number the hub gave rather
// than a guess. Unfiltered it is the project's event count less the recent
// rows, once the loaded pages are known to hold every recent row. Filtered,
// the hub counts nothing, so only a feed read to its end has a number.
function earlierCount(visit, recent, earlier) {
  const loaded = earlier.reduce((sum, group) => sum + group.events.length, 0);
  if (!visit.more) return loaded;
  if (visit.filtered || visit.total == null || !loaded) return null;
  const shown = recent.reduce((sum, group) => sum + group.events.length, 0);
  return Math.max(loaded, visit.total - shown);
}

const CHEVRON = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 9 6l6 6-6 6"></path></svg>`;

function foldHTML(projectId, visit) {
  const groups = byDay(visit.events);
  const recent = groups.filter((group) => RECENT.includes(group.label));
  const earlier = groups.filter((group) => !RECENT.includes(group.label));
  if (!earlier.length && !visit.more) return "";
  // A feed with nothing recent would be a lone disclosure over an empty
  // screen, so it opens on its older days until the reader folds them.
  const open = visit.open ?? recent.length === 0;
  const count = earlierCount(visit, recent, earlier);
  const label = count == null ? "Earlier" : `Earlier · ${count} ${count === 1 ? "event" : "events"}`;
  const older = visit.more
    ? `<button type="button" class="feed-older" data-action="feed-older">Show older</button>`
    : "";
  const region = open ? `${dayGroups(earlier, visit.baseline)}${older}` : "";
  return `<div class="feed-fold" data-project="${esc(projectId)}">
    <button type="button" class="feed-earlier" data-action="feed-earlier" aria-expanded="${open}" aria-controls="feed-earlier">${label}${CHEVRON}</button>
    <div id="feed-earlier"${open ? "" : " hidden"}>${region}</div>
  </div>`;
}

function feedPath(projectId, active, before) {
  const kinds = [...active].map((kind) => `&kinds=${encodeURIComponent(kind)}`).join("");
  const cursor = before ? `&before=${encodeURIComponent(before)}` : "";
  return `/api/v1/projects/${encodeURIComponent(projectId)}/feed?limit=${PAGE}${cursor}${kinds}`;
}

// The read cursor moves once the feed has been seen: painted, in a visible
// tab. A filtered page skips events, so reading one says nothing about the
// events it left out and leaves the cursor alone.
function markSeen(projectId, visit) {
  const newest = visit.events[0]?.id;
  if (visit.filtered || !newest || newest <= visit.sent) return;
  // The feed is painted once its body is in the index: the day groups, the
  // older-events fold, or the empty state. A chip row is not the sentinel: the
  // one shell draws its own control row and renders the feed with chips:false,
  // so waiting for `.feed-chips` waited for a row that never came.
  const painted = () => {
    const body = main.querySelector("#ah-index");
    return !!body && !!body.querySelector(".feed-day, .feed-fold, [data-action='feed-copy-setup']");
  };
  const send = () => {
    if (visits.get(projectId) !== visit || newest <= visit.sent) return;
    // The reader left this project's feed: stop retrying rather than loop.
    if (feedProject(location.hash) !== projectId) return;
    if (!painted()) {
      setTimeout(send, 50);
      return;
    }
    if (document.visibilityState !== "visible") {
      document.addEventListener("visibilitychange", send, { once: true });
      return;
    }
    visit.sent = newest;
    api(`/api/v1/projects/${encodeURIComponent(projectId)}/feed/seen`, {
      method: "POST",
      body: JSON.stringify({ event_id: newest }),
    }).catch(() => {
      // An unmoved cursor costs a dot that stays one visit longer.
      visit.sent = "";
    });
  };
  // After the paint this section is on its way into.
  setTimeout(send, 0);
}

// The chip row and the day-grouped events, or the empty state. `stats` is the
// project's counts when the caller already holds them. The feed's empty state
// names the project, so the id (the slug) is what fills its copy.
//
// `chips` is false when the caller draws its own control row (the one shell
// folds the kind filter into the Group menu), so the rows are returned alone.
export async function feedSection(current, stats = null, { chips = true } = {}) {
  const active = activeKinds(current);
  // The kind filter runs in the query, before the limit, so a chip finds
  // the newest events of its kind rather than only those inside a fetched
  // window.
  const page = await api(feedPath(current, active));
  const held = visits.get(current);

  let counts = held?.counts;
  if (!counts) {
    let allEvents = page.events;
    if (active.size > 0) {
      try {
        const unfiltered = await api(feedPath(current, new Set()));
        allEvents = unfiltered.events;
      } catch {}
    }
    counts = { total: 0 };
    for (const k of FEED_KINDS) counts[k.kind] = 0;
    for (const event of allEvents) {
      if (event.kind === "answer") continue;
      counts.total++;
    }
    for (const event of allEvents) {
      // An answer is a reply on a thread: it is not an entry of its own, so
      // it neither counts nor feeds the "All" total.
      if (event.kind === "answer") continue;
      if (counts[event.kind] != null) counts[event.kind]++;
    }
  }

  const visit = {
    baseline: held ? held.baseline : page.last_seen || "",
    sent: held ? held.sent : page.last_seen || "",
    open: held ? held.open : undefined,
    events: page.events,
    before: page.next_before,
    more: page.events.length === PAGE,
    filtered: active.size > 0,
    total: stats && stats.events != null ? stats.events : null,
    counts,
  };
  visits.set(current, visit);
  if (!visit.events.length) {
    // Nothing to set up when the project has events and the filter hides them.
    const copy = active.size
      ? {
          ...EMPTY_COPY.feed,
          title: "No events match this filter.",
          body: "Clear a filter to see everything in this project.",
          link: null,
        }
      : EMPTY_COPY.feed;
    const where = { action: "feed-copy-setup", id: current };
    const body = emptyStateHTML(copy, { project: current }, where);
    return chips ? `${kindChips(active, counts)}${body}` : body;
  }
  markSeen(current, visit);
  const recent = byDay(visit.events).filter((group) => RECENT.includes(group.label));
  const body = `${dayGroups(recent, visit.baseline, selectedEvent)}${foldHTML(current, visit)}`;
  return chips ? `${kindChips(active, counts)}${body}` : body;
}

// The events the last index render holds, for the stage that shows one of
// them. The shell reads the list it just painted rather than refetching.
export function feedVisit(projectId) {
  return visits.get(projectId) || null;
}

// The kind counts the last index render worked out, for the Group menu.
export function feedCounts(projectId) {
  return visits.get(projectId)?.counts || {};
}

// One event by id, from the held visit. An event that has scrolled out of the
// loaded page is not found; the stage then says so rather than inventing it.
export function feedEvent(projectId, eventId) {
  const visit = visits.get(projectId);
  if (!visit || !eventId) return null;
  return visit.events.find((event) => event.id === eventId) || null;
}

const KIND_WORD = {
  signal: "Update",
  finished: "Finished",
  question: "Question",
  answer: "Answer",
  approval: "Approval",
  artifact: "Artifact",
  session: "Session",
  anchor: "Anchor",
  score: "Score",
};

// The events an anchor cites, or the anchors a score grades, as rows that open
// the cited event in the feed. The id is the link's text, so a pruned source
// still reads as a citation rather than as an empty row.
function linkList(label, ids, projectId) {
  if (!ids.length) return "";
  const rows = ids
    .map(
      (id) =>
        `<a class="points-row" href="#/projects/${encodeURIComponent(projectId)}/feed?event=${encodeURIComponent(id)}"><span class="points-name mono">${esc(id)}</span><span class="points-chev" aria-hidden="true">${CHEVRON}</span></a>`,
    )
    .join("");
  return `<section class="points-card" aria-label="${esc(label)}"><div class="points-head mono">${esc(label)}</div>${rows}</section>`;
}

// An anchor's stage: the phase it closes, the plan it leaves, and the events it
// rests on. The summary is the title; this is the detail a resumed reader uses.
function anchorDetail(event, projectId) {
  const phase = typeof event.payload?.phase === "string" ? event.payload.phase.trim() : "";
  const steps = Array.isArray(event.payload?.next_steps) ? event.payload.next_steps : [];
  const sources = Array.isArray(event.payload?.source_ids) ? event.payload.source_ids : [];
  const stepsHTML = steps.length
    ? `<p class="feed-stage-label mono">NEXT STEPS</p><ol class="feed-stage-list">${steps
        .map((step) => `<li>${esc(step)}</li>`)
        .join("")}</ol>`
    : "";
  return `${phase ? `<p class="feed-stage-body meta mono">Phase ${esc(phase)}</p>` : ""}${stepsHTML}${linkList("SOURCES", sources, projectId)}`;
}

// A score's stage: the value, the words that justify it, and the anchors it
// grades. A score never edits the run, so the slice is a set of links back to
// the anchors rather than a rewritten summary.
function scoreDetail(event, projectId) {
  const score = event.payload?.score;
  const rationale = typeof event.payload?.rationale === "string" ? event.payload.rationale : "";
  const slice = Array.isArray(event.payload?.slice) ? event.payload.slice : [];
  return `${score != null ? `<p class="feed-stage-body"><span class="pill" data-kind="score">Score ${esc(score)}/5</span></p>` : ""}${rationale ? `<p class="feed-stage-body">${esc(rationale)}</p>` : ""}${linkList("SLICE", slice, projectId)}`;
}

// What a feed event is, when it is opened in the stage: the kind, whether it
// still waits on the reader, its words, the verbs it offers, and a card naming
// the thing it is about. The feed stops being a wall you scroll and becomes a
// list you open, which is what the artifact view already was.
export function eventStage(event, projectId) {
  if (!event) {
    return `<div class="shell-prose"><p class="empty">Select an event from the list.</p></div>`;
  }
  // An answer is a reply: reading it reads its whole thread, Gmail-style, so
  // the main question and every reply render together rather than one at a
  // time.
  const visit = feedVisit(projectId);
  const rootId =
    event.kind === "answer" && event.thread_id ? event.thread_id : event.id || event.event_id;
  let rootEvent = event;
  if (event.kind === "answer" && visit?.events) {
    const found = visit.events.find((candidate) => candidate.id === rootId || candidate.event_id === rootId);
    if (found) rootEvent = found;
  }
  event = rootEvent;
  const threadReplies = visit?.events
    ? visit.events
        .filter(
          (candidate) =>
            candidate.kind === "answer" &&
            (candidate.thread_id === rootId || candidate.thread_id === event.id || candidate.thread_id === event.event_id) &&
            (candidate.id || candidate.event_id) !== (event.id || event.event_id),
        )
        .slice()
        .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at))
    : [];
  const open = isOpen(event);
  const word = KIND_WORD[event.kind] || event.kind;
  // A finished event's summary is a report, not a subject, so the stage titles
  // it with its leading sentence and reads the rest as prose. A question's
  // short subject keeps the title it always had.
  const { subject, message } = subjectAndMessage(formatEventSummary(event));
  const body = typeof event.payload?.body === "string" ? event.payload.body.trim() : "";
  const detail =
    event.kind === "anchor"
      ? anchorDetail(event, projectId)
      : event.kind === "score"
        ? scoreDetail(event, projectId)
        : "";
  const dest = feedDestination(event);
  const sessionId = event.kind === "session" ? event.payload?.session_id : null;
  let pointsAt = "";
  if (dest) {
    pointsAt = pointsCard("Artifact", event.payload?.title || "Open artifact", dest);
  } else if (sessionId) {
    pointsAt = pointsCard(
      "Session",
      sessionId,
      `#/projects/${encodeURIComponent(projectId)}/sessions?id=${encodeURIComponent(sessionId)}`,
    );
  }
  return `
    <article class="shell-prose feed-stage">
      <div class="feed-stage-pills">
        <span class="pill" data-kind="${esc(event.kind)}">${esc(word)}</span>
        ${open ? `<span class="pill pill-status"><span class="pill-dot" aria-hidden="true"></span>Waiting on you</span>` : ""}
      </div>
      <h2 class="feed-stage-title">${esc(subject)}</h2>
      <div class="meta mono">${esc(event.actor)} · ${when(event.created_at)}</div>
      ${message ? `<p class="feed-stage-message">${esc(message)}</p>` : ""}
      ${body ? `<p class="feed-stage-body">${esc(body)}</p>` : ""}
      ${detail}
      <div class="feed-stage-actions">${actionFor(event)}</div>
      ${pointsAt}
      ${
        threadReplies.length
          ? `<div class="feed-stage-thread">${threadReplies
              .map(
                (reply) => `
        <div class="feed-stage-reply">
          <div class="meta mono">${esc(reply.actor)} · ${when(reply.created_at)}</div>
          <p class="feed-stage-body">${esc(
            typeof reply.payload?.body === "string" && reply.payload.body.trim()
              ? reply.payload.body.trim()
              : formatEventSummary(reply),
          )}</p>
        </div>`,
              )
              .join("")}</div>`
          : ""
      }
    </article>`;
}

function pointsCard(label, name, href) {
  return `
    <section class="points-card" aria-label="Points at">
      <div class="points-head mono">POINTS AT</div>
      <a class="points-row" href="${esc(href)}">
        <span class="sr-only">${esc(label)}: </span>
        <span class="points-name">${esc(name)}</span>
        <span class="points-chev" aria-hidden="true">${CHEVRON}</span>
      </a>
    </section>`;
}

// The kind chips are per project. The toggle keeps focus on the chip it
// pressed rather than dropping a keyboard user back at the top of the page.
export function toggleKind(kind, projectId) {
  if (!projectId) return;
  const active = new Set(projectFilters.get(projectId) || []);
  if (kind === ALL) active.clear();
  else if (active.has(kind)) active.delete(kind);
  else active.add(kind);
  projectFilters.set(projectId, [...active]);
  focusAfterRender(kind);
  render();
}

// Put the boxes and the count back in step with the picks held in memory. The
// rows are repainted for the fold and for a stream update, and a selection that
// survives a repaint is what lets a reader pick across it.
function syncPicks() {
  for (const item of main.querySelectorAll(".feed-item")) {
    const box = item.querySelector(".feed-pick");
    if (!box) continue;
    const ids = (box.dataset.ids || "").split(" ").filter(Boolean);
    const checked = ids.length > 0 && ids.every((id) => picking.ids.has(id));
    box.checked = checked;
    item.querySelector(".feed-row")?.classList.toggle("picked", checked);
  }
  for (const bar of main.querySelectorAll(".feed-select-bar")) {
    const said = bar.querySelector(".feed-select-count");
    if (said) said.textContent = picking.ids.size === 1 ? "1 selected" : `${picking.ids.size} selected`;
    const go = bar.querySelector('[data-action="feed-dismiss-picked"]');
    if (go) go.disabled = picking.ids.size === 0;
  }
}

// Selection mode after a repaint: the router redraws the control row, so the
// state that hid the filter and showed the bar has to be put back.
export function resumePicking() {
  if (!picking.active) return;
  const shell = main.querySelector(".shell[data-segment]");
  shell?.classList.add("is-picking");
  // A phone's bar lives in its one tools row; a desktop's sits in the index
  // control row. Both are painted, and the one the width draws is the one the
  // reader sees.
  for (const row of main.querySelectorAll(".shell-controls, .project-tools-mobile")) {
    // A phone keeps the filter panel behind its glyph; picking is a mode, and
    // it must not leave that panel's second bar on screen with the tools row.
    row.classList.remove("mobile-filter-open");
    row.classList.add("is-picking");
  }
  for (const bar of main.querySelectorAll(".feed-select-bar")) bar.hidden = false;
  syncPicks();
}

function startPicking(projectId, opener = null) {
  concealSwipe();
  picking.active = true;
  picking.project = projectId;
  picking.ids.clear();
  picking.opener = opener?.closest(".proj-overflow-wrap")?.querySelector(".proj-overflow-btn") || null;
  resumePicking();
  requestAnimationFrame(() => {
    const first = main.querySelector(".feed-pick");
    const target = first || main.querySelector(".feed-select-bar button");
    target?.focus({ preventScroll: true });
  });
}

function applyPick(item, on) {
  const box = item.querySelector(".feed-pick");
  const ids = (box?.dataset.ids || "").split(" ").filter(Boolean);
  for (const id of ids) {
    if (on) picking.ids.add(id);
    else picking.ids.delete(id);
  }
  syncPicks();
}

async function dismissEvents(projectId, ids) {
  // A collapsed artifact row can stand for more events than one request may
  // name, so the ids go in server-sized batches and the count is summed.
  let dismissed = 0;
  for (let at = 0; at < ids.length; at += DISMISS_MAX) {
    const answer = await api(
      `/api/v1/projects/${encodeURIComponent(projectId)}/feed/dismiss`,
      { method: "POST", body: JSON.stringify({ event_ids: ids.slice(at, at + DISMISS_MAX) }) },
    );
    dismissed += answer.dismissed;
  }
  await render();
  if (!dismissed) toast("Already dismissed.");
  else toast(dismissed === 1 ? "Dismissed 1 event." : `Dismissed ${dismissed} events.`);
}

async function dismissPicked(projectId) {
  const ids = [...picking.ids];
  if (!ids.length) return;
  const noun = ids.length === 1 ? "event" : "events";
  const confirmed = await confirmAction({
    title: `Dismiss ${ids.length} ${noun}?`,
    body: "They leave this project's feed. Agents still read them, and the inbox and search keep them.",
    safe: "Keep",
    tone: "primary",
    danger: "Dismiss",
  });
  if (!confirmed) return;
  endPicking();
  await dismissEvents(projectId, ids);
}

async function dismissRead(projectId) {
  const confirmed = await confirmAction({
    title: "Dismiss read events?",
    body: "Every event the feed has marked seen leaves it, including the ones below the fold. Events written after this page was opened stay.",
    safe: "Keep",
    tone: "primary",
    danger: "Dismiss read",
  });
  if (!confirmed) return;
  const { dismissed } = await api(
    `/api/v1/projects/${encodeURIComponent(projectId)}/feed/dismiss-read`,
    { method: "POST" },
  );
  await render();
  if (!dismissed) toast("Nothing read to dismiss.");
  else toast(dismissed === 1 ? "Dismissed 1 read event." : `Dismissed ${dismissed} read events.`);
}

// Swipes and the pull. Touch and pen only: a mouse drag selects text, and a
// mouse reaches Dismiss through Select items.
let feedDrag = null;
let feedRevealed = null;
let feedUnregister = null;

// A swipe that began on the row's title ends on it too, and the browser may
// call that a click. The row was swiped, not opened.
let feedSwiped = false;
main.addEventListener(
  "click",
  (event) => {
    if (!feedSwiped || !event.target.closest(".feed-row")) return;
    event.preventDefault();
    event.stopPropagation();
  },
  true,
);

const still = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function concealSwipe() {
  if (!feedRevealed) return;
  const item = feedRevealed;
  feedRevealed = null;
  if (feedUnregister) feedUnregister();
  feedUnregister = null;
  delete item.dataset.revealed;
  settleSwipe(item);
}

// Put a row back where its state says it rests, and hide the tray nothing is
// showing.
function settleSwipe(item) {
  const row = item.querySelector(".feed-row");
  const open = item.dataset.revealed === "dismiss";
  const tray = item.querySelector(".swipe-tray-dismiss");
  if (row) row.style.transform = open && tray ? `translateX(${-ACTION_WIDTH}px)` : "";
  if (tray) tray.hidden = !open;
}

function revealSwipe(item) {
  concealSwipe();
  item.dataset.revealed = "dismiss";
  feedRevealed = item;
  feedUnregister = registerPane(concealSwipe);
  settleSwipe(item);
}

main.addEventListener("pointerdown", (event) => {
  if (event.pointerType === "mouse" || !event.isPrimary || picking.active) return;
  if (!feedProject(location.hash)) return;
  const item = event.target.closest(".feed-item");
  if (feedRevealed && feedRevealed !== item) concealSwipe();
  if (event.target.closest("button, input, textarea, summary")) return;
  feedDrag = {
    id: event.pointerId,
    x: event.clientX,
    y: event.clientY,
    dx: 0,
    dy: 0,
    axis: "",
    item,
    from: item && item === feedRevealed ? -ACTION_WIDTH : 0,
    still: still(),
  };
});

window.addEventListener("pointermove", (event) => {
  if (!feedDrag || event.pointerId !== feedDrag.id) return;
  feedDrag.dx = event.clientX - feedDrag.x;
  feedDrag.dy = event.clientY - feedDrag.y;
  if (!feedDrag.axis) {
    if (Math.abs(feedDrag.dx) < SLOP && Math.abs(feedDrag.dy) < SLOP) return;
    feedDrag.axis = Math.abs(feedDrag.dx) > Math.abs(feedDrag.dy) ? "x" : "y";
  }
  if (feedDrag.axis !== "x") return;
  const { item } = feedDrag;
  if (!item) return;
  const row = item.querySelector(".feed-row");
  const tray = item.querySelector(".swipe-tray-dismiss");
  const offset = Math.min(0, Math.max(feedDrag.from + feedDrag.dx, -ACTION_WIDTH));
  if (tray) tray.hidden = offset === 0;
  if (!feedDrag.still && row) row.style.transform = offset ? `translateX(${offset}px)` : "";
});

function releaseSwipe(event) {
  if (!feedDrag || event.pointerId !== feedDrag.id) return;
  const { item, axis, dx, from } = feedDrag;
  const cancelled = event.type === "pointercancel";
  feedDrag = null;
  // A tap on a row whose action is showing puts it away.
  if (!axis && item && item === feedRevealed) {
    concealSwipe();
    return;
  }
  if (axis !== "x" || !item) return;
  feedSwiped = true;
  setTimeout(() => (feedSwiped = false), 50);
  // Past half of the action the tray opens; a swipe back the same way shuts it.
  const offset = from + dx;
  if (!cancelled && offset <= -ACTION_WIDTH / 2) revealSwipe(item);
  else if (item === feedRevealed) concealSwipe();
  else settleSwipe(item);
}
window.addEventListener("pointerup", releaseSwipe);
window.addEventListener("pointercancel", releaseSwipe);

// The fold is replaced whole, as a child of the screen region, so the keyboard
// map sees the rows change the way it sees any repaint. Focus goes back onto
// the control that was pressed, or the disclosure once that control is gone.
function repaintFold(projectId, visit, action) {
  const fold = main.querySelector(".feed-fold");
  if (!fold || fold.dataset.project !== projectId) return;
  // A later page can hold more of today and yesterday, when those days are
  // longer than one page. Those rows belong above the fold, so the recent days
  // are redrawn from the held visit along with it.
  const chips = main.querySelector(".feed-chips");
  if (chips) {
    let node = chips.nextElementSibling;
    while (node && node !== fold) {
      const gone = node;
      node = node.nextElementSibling;
      gone.remove();
    }
    const recent = byDay(visit.events).filter((group) => RECENT.includes(group.label));
    chips.insertAdjacentHTML("afterend", dayGroups(recent, visit.baseline));
  }
  fold.insertAdjacentHTML("afterend", foldHTML(projectId, visit));
  const next = fold.nextElementSibling;
  fold.remove();
  syncPicks();
  // The fold is gone once everything was recent and nothing older is left, so
  // focus goes to the last row that was just drawn rather than nowhere.
  const control =
    next?.querySelector(`[data-action="${action}"]`) ||
    next?.querySelector(".feed-earlier") ||
    [...main.querySelectorAll(".feed-row")].pop() ||
    main;
  if (!control.hasAttribute("tabindex")) control.setAttribute("tabindex", "-1");
  control.focus({ preventScroll: true });
}

async function loadOlder(projectId, visit) {
  // A second call while the first is in flight would fetch the same cursor and
  // append the page twice. H16 lower-severity list.
  if (visit.loading) return;
  visit.loading = true;
  try {
    const page = await api(feedPath(projectId, activeKinds(projectId), visit.before));
    if (visits.get(projectId) !== visit) return;
    visit.events = visit.events.concat(page.events);
    visit.before = page.next_before || visit.before;
    visit.more = page.events.length === PAGE;
  } finally {
    visit.loading = false;
  }
}

async function toggleFold(button, projectId) {
  const visit = visits.get(projectId);
  if (!visit) return;
  visit.open = button.getAttribute("aria-expanded") !== "true";
  const nothingLoaded = byDay(visit.events).every((group) => RECENT.includes(group.label));
  if (visit.open && nothingLoaded && visit.more) await loadOlder(projectId, visit);
  repaintFold(projectId, visit, "feed-earlier");
}

async function showOlder(projectId) {
  const visit = visits.get(projectId);
  if (!visit || !visit.more) return;
  await loadOlder(projectId, visit);
  repaintFold(projectId, visit, "feed-older");
}

// What an agent needs to reach this hub, in the forms the hub's own skill
// document gives them. The token is the agent's, issued under Agents and
// tokens, and is never the one this browser holds. When the caller holds the
// token already it is embedded, so a reader never splices it by hand; when it
// does not, the snippet names where the token comes from.
export function mcpSetup(projectId, token = "<agent token>") {
  // Resolve against the document, not the origin: a hub served behind a
  // path-stripping proxy lives at a prefix, and the origin alone loses it. H8.
  const base = new URL(".", document.baseURI).href;
  const hasToken = token !== "<agent token>";
  const where = hasToken
    ? ""
    : `# Create an agent at ${new URL("#/access", base).href} and copy the token it shows once.`;
  return [
    projectId
      ? `# Agent Hub MCP setup. The project slug is ${projectId}.`
      : "# Agent Hub MCP setup.",
    ...(where ? [where] : []),
    "# Streamable HTTP:",
    `POST ${new URL("mcp", base).href}`,
    `Authorization: Bearer ${token}`,
    "",
    "# A harness that speaks only stdio runs the proxy:",
    `HUB_URL=${base} HUB_TOKEN=${token} agent-hub mcp`,
    "",
    `# The setup guide: ${new URL("bootstrap/SKILL.md", base).href}`,
    "",
  ].join("\n");
}

// A hub on a LAN address is not a secure context, and there the browser has
// no clipboard to write to. The setup is then put on screen, selected, so the
// reader's own copy shortcut finishes the job.
function showSetup(button, text) {
  const state = button.closest(".empty-state");
  let box = state.querySelector(".feed-setup");
  if (!box) {
    box = document.createElement("div");
    box.className = "feed-setup";
    const said = document.createElement("p");
    said.id = "feed-setup-why";
    said.setAttribute("role", "status");
    said.textContent = "This browser would not copy it. Select the text below and copy it.";
    const field = document.createElement("textarea");
    field.readOnly = true;
    field.rows = 9;
    field.setAttribute("aria-label", "MCP setup");
    field.setAttribute("aria-describedby", said.id);
    field.wrap = "off";
    field.spellcheck = false;
    box.append(said, field);
    state.appendChild(box);
  }
  const field = box.querySelector("textarea");
  field.value = text;
  field.focus({ preventScroll: true });
  field.select();
}

async function copySetup(button) {
  const text = mcpSetup(button.dataset.id);
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    showSetup(button, text);
    return;
  }
  button.closest(".empty-state").querySelector(".feed-setup")?.remove();
  toast("MCP setup copied.");
}

// The feed's own controls. The shared handler routes the actions other screens
// own; these exist only here.
main.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const { action } = button.dataset;
  const fold = button.closest(".feed-fold");
  const failed = (error) => toast(`Nothing changed: ${error.message}`);
  const partly = (error) => toast(`Some of it may not have been saved: ${error.message}`);
  let work = null;
  if (action === "feed-earlier" && fold) work = toggleFold(button, fold.dataset.project);
  if (action === "feed-older" && fold) work = showOlder(fold.dataset.project);
  if (action === "feed-copy-setup") copySetup(button);
  if (action === "feed-select") startPicking(button.dataset.project || "", button);
  if (action === "feed-dismiss-read") work = dismissRead(button.dataset.project || "");
  if (action === "feed-select-cancel") endPicking({ focus: true });
  if (action === "feed-dismiss-picked") work = dismissPicked(picking.project);
  if (action === "feed-dismiss") {
    const item = button.closest(".feed-item");
    const ids = (item?.dataset.ids || "").split(" ").filter(Boolean);
    const project = item?.dataset.project || "";
    concealSwipe();
    if (ids.length && project) work = dismissEvents(project, ids);
  }
  const partial =
    action === "feed-dismiss-picked" || action === "feed-dismiss" || action === "feed-dismiss-read";
  if (work) work.catch(partial ? partly : failed);
});

// While picking, the row is a checkbox and not a door: a press anywhere on it
// turns the pick on or off, and the link the row carries does not navigate.
main.addEventListener(
  "click",
  (event) => {
    if (!picking.active) return;
    const item = event.target.closest(".feed-item");
    if (!item) return;
    const box = item.querySelector(".feed-pick");
    if (!box || event.target === box) return;
    event.preventDefault();
    event.stopPropagation();
    box.checked = !box.checked;
    applyPick(item, box.checked);
  },
  true,
);

main.addEventListener("change", (event) => {
  if (!picking.active) return;
  const box = event.target.closest?.(".feed-pick");
  const item = box?.closest(".feed-item");
  if (box && item) applyPick(item, box.checked);
});
