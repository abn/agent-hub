// The project feed: one project's events, day-grouped and filterable by kind.
// Today and Yesterday are open, older days sit behind the Earlier disclosure,
// and what landed since the reader last looked carries the unread mark.

import { api } from "./api.mjs";
import { actionFor, esc, glyph, isOpen, main, when } from "./dom.mjs";
import { emptyStateHTML, EMPTY_COPY } from "./empty.mjs";
import { focusAfterRender, render } from "./router.mjs";
import { byDay } from "./time.mjs";
import { toast } from "./toast.mjs";

// The design's chip labels are the kind names themselves.
const KINDS = ["signal", "finished", "question", "answer", "approval", "artifact", "session"];
const ALL = "all";
// One page of the feed. Older pages are asked for with the hub's own cursor.
const PAGE = 100;
const RECENT = ["Today", "Yesterday"];

// The kind filters are per project, so switching projects does not carry a
// filter across. They are view state, not a saved preference.
const projectFilters = new Map();

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
  return new Set(projectFilters.get(projectId) || []);
}

export function kindChips(active) {
  const chip = (kind, label, pressed) =>
    `<button type="button" class="chip" data-action="kind" data-kind="${kind}" aria-pressed="${pressed}">${label}</button>`;
  const chips = KINDS.map((kind) => chip(kind, kind, active.has(kind))).join("");
  return `<div class="feed-chips" role="group" aria-label="Filter by kind">${chip(ALL, "All", active.size === 0)}${chips}</div>`;
}

// The dot is a colour and a shape, so the word sits beside it for a reader
// who gets neither.
const UNREAD = `<span class="dot-unread" aria-hidden="true"></span><span class="sr-only">Unread</span>`;

// The feed's own row. Event ids sort by age, which is how the hub compares
// them to the cursor too, so an id above the baseline is an unseen event.
function feedRow(event, baseline) {
  const unread = event.id > baseline;
  return `<div class="row feed-row${unread ? " unread" : ""}">
    ${glyph(event.kind)}
    <div class="grow" data-id="${esc(event.id)}">
      <div class="title">${esc(event.summary)}</div>
      <div class="meta">${esc(event.actor)} · ${when(event.created_at)}</div>
    </div>
    ${isOpen(event) ? actionFor(event) : ""}
    ${unread ? UNREAD : ""}
  </div>`;
}

function dayGroups(groups, baseline) {
  return groups
    .map(
      (group) =>
        `<h2 class="day">${esc(group.label)}</h2>
         <div class="card feed-day">${group.events.map((event) => feedRow(event, baseline)).join("")}</div>`,
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
  const painted = () => feedProject(location.hash) === projectId && !!main.querySelector(".feed-chips");
  const send = () => {
    if (visits.get(projectId) !== visit || !painted() || newest <= visit.sent) return;
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
export async function feedSection(current, stats = null) {
  const active = activeKinds(current);
  // The kind filter runs in the query, before the limit, so a chip finds
  // the newest events of its kind rather than only those inside a fetched
  // window.
  const page = await api(feedPath(current, active));
  const held = visits.get(current);
  const visit = {
    baseline: held ? held.baseline : page.last_seen || "",
    sent: held ? held.sent : page.last_seen || "",
    open: held ? held.open : undefined,
    events: page.events,
    before: page.next_before,
    more: page.events.length === PAGE,
    filtered: active.size > 0,
    total: stats && stats.events != null ? stats.events : null,
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
    return `${kindChips(active)}${emptyStateHTML(copy, { project: current }, where)}`;
  }
  markSeen(current, visit);
  const recent = byDay(visit.events).filter((group) => RECENT.includes(group.label));
  return `${kindChips(active)}${dayGroups(recent, visit.baseline)}${foldHTML(current, visit)}`;
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
