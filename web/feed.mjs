// The project feed: one project's events, day-grouped and filterable by kind.
// Today and Yesterday are open, older days sit behind the Earlier disclosure,
// and what landed since the reader last looked carries the unread mark.

import { api } from "./api.mjs";
import { actionFor, esc, glyph, isOpen, main, when } from "./dom.mjs";
import { emptyStateHTML, EMPTY_COPY } from "./empty.mjs";
import { focusAfterRender, render } from "./router.mjs";
import { byDay } from "./time.mjs";
import { toast } from "./toast.mjs";

// Feed chips cover question, approval, finished, signal only.
const FEED_KINDS = [
  { kind: "question", label: "Questions" },
  { kind: "approval", label: "Approvals" },
  { kind: "finished", label: "Finished" },
  { kind: "signal", label: "Signals" },
];
const VALID_KINDS = new Set(FEED_KINDS.map((k) => k.kind));
const ALL = "all";
// One page of the feed. Older pages are asked for with the hub's own cursor.
const PAGE = 100;
const RECENT = ["Today", "Yesterday"];

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

// The dot is a colour and a shape, so the word sits beside it for a reader
// who gets neither.
const UNREAD = `<span class="dot-unread" aria-hidden="true"></span><span class="sr-only">Unread</span>`;

// The note a human left with a decision, on the row of the decision's own
// event. The word before it is this module's, picked by the payload's
// `decision`; the note is whatever was typed, so it goes through `esc`.
const DECIDED = { approved: "Approved", declined: "Declined" };

function decisionNote(event) {
  const payload = event.payload;
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

function collapsedArtifactRow(item, baseline) {
  const unread = item.events.some((e) => e.id > baseline);
  const project = item.project_id || "";
  const href = `#/projects/${encodeURIComponent(project)}/artifacts`;
  const title = `<a class="title feed-link" href="${esc(href)}">published ${item.count} artifacts</a>`;
  return `<div class="row feed-row${unread ? " unread" : ""}">
    ${glyph("artifact")}
    <div class="grow" data-id="${esc(item.id)}">
      ${title}
      <div class="meta">${esc(item.actor)} · ${when(item.created_at)}</div>
    </div>
    ${unread ? UNREAD : ""}
  </div>`;
}

// The feed's own row. Event ids sort by age, which is how the hub compares
// them to the cursor too, so an id above the baseline is an unseen event.
// A row opens the event in the stage; the artifact or session it is about is
// reached from the stage's own card, so the row is a door to the item and not
// straight past it.
function feedRow(event, baseline, selected = false) {
  const unread = event.id > baseline;
  const project = event.project_id || "";
  const href = `#/projects/${encodeURIComponent(project)}/feed?event=${encodeURIComponent(event.id)}`;
  const summaryText = formatEventSummary(event);
  const open = isOpen(event);
  const title = `<a class="title feed-link" href="${esc(href)}">${esc(summaryText)}</a>`;
  const waitingPill = open ? `<span class="pill pill-waiting">Waiting on you</span>` : "";
  return `<div class="row feed-row${unread ? " unread" : ""}${selected ? " selected" : ""}" data-id="${esc(event.id)}">
    ${glyph(event.kind)}
    <div class="grow">
      <div class="feed-row-title-bar">
        ${title}
        ${waitingPill}
      </div>
      ${decisionNote(event)}
      <div class="meta">${esc(event.actor)} · ${when(event.created_at)}</div>
    </div>
    ${unread ? UNREAD : ""}
  </div>`;
}

function dayGroups(groups, baseline, selectedId = "") {
  return groups
    .map(
      (group) =>
        `<h2 class="day">${esc(group.label)}</h2>
         <div class="card feed-day">${collapseDayEvents(group.events)
           .map((item) =>
             item.collapsed
               ? collapsedArtifactRow(item, baseline)
               : feedRow(item, baseline, item.id === selectedId),
           )
           .join("")}</div>`,
    )
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
    counts = { total: stats?.events ?? allEvents.length };
    for (const k of FEED_KINDS) counts[k.kind] = 0;
    for (const event of allEvents) {
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
};

// What a feed event is, when it is opened in the stage: the kind, whether it
// still waits on the reader, its words, the verbs it offers, and a card naming
// the thing it is about. The feed stops being a wall you scroll and becomes a
// list you open, which is what the artifact view already was.
export function eventStage(event, projectId) {
  if (!event) {
    return `<div class="shell-prose"><p class="empty">Select an event from the list.</p></div>`;
  }
  const open = isOpen(event);
  const word = KIND_WORD[event.kind] || event.kind;
  const title = formatEventSummary(event);
  const body = typeof event.payload?.body === "string" ? event.payload.body.trim() : "";
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
        ${open ? `<span class="pill pill-waiting">Waiting on you</span>` : ""}
      </div>
      <h2 class="feed-stage-title">${esc(title)}</h2>
      <div class="meta mono">${esc(event.actor)} · ${when(event.created_at)}</div>
      ${body ? `<p class="feed-stage-body">${esc(body)}</p>` : ""}
      <div class="feed-stage-actions">${actionFor(event)}</div>
      ${pointsAt}
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
  const page = await api(feedPath(projectId, activeKinds(projectId), visit.before));
  if (visits.get(projectId) !== visit) return;
  visit.events = visit.events.concat(page.events);
  visit.before = page.next_before || visit.before;
  visit.more = page.events.length === PAGE;
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
// document gives them. The token is the agent's, issued in Settings, and is
// never the one this browser holds.
export function mcpSetup(projectId) {
  const origin = location.origin;
  return [
    `# Agent Hub MCP setup. The project slug is ${projectId}.`,
    "# Streamable HTTP:",
    `POST ${origin}/mcp`,
    "Authorization: Bearer <agent token>",
    "",
    "# A harness that speaks only stdio runs the proxy:",
    `HUB_URL=${origin} HUB_TOKEN=<agent token> agent-hub mcp`,
    "",
    `# The full guide: ${origin}/SKILL.md`,
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
// own; these three exist only here.
main.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const { action } = button.dataset;
  const fold = button.closest(".feed-fold");
  const failed = (error) => toast(`Nothing changed: ${error.message}`);
  if (action === "feed-earlier" && fold) toggleFold(button, fold.dataset.project).catch(failed);
  if (action === "feed-older" && fold) showOlder(fold.dataset.project).catch(failed);
  if (action === "feed-copy-setup") copySetup(button);
});
