// Home: the day at a glance. What waits on you, the newest events across every
// project, and what the node holds. Everything here is read off the one Home
// response, so painting the screen is one request however much it shows.

import { api } from "./api.mjs";
import { actionFor, esc, glyph, isOpen, main, paint, projectName } from "./dom.mjs";
import { emptyStateHTML, EMPTY_COPY } from "./empty.mjs";
import { registerScreen } from "./keys.mjs";
import { installShellLayout, shellHTML, shellStageHead } from "./shell-layout.mjs";
import { SLIVER, SLIVER_WORDS } from "./storage.mjs";
import { timeHTML } from "./time.mjs";

// How many waiting items the card carries before it hands over to the Inbox.
const WAITING_ROWS = 3;

const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

export function dayPart(now) {
  const hour = now.getHours();
  if (hour < 5 || hour >= 21) return "night";
  if (hour < 12) return "morning";
  return hour < 17 ? "afternoon" : "evening";
}

// The title is the reader's own clock. The small hours still belong to the
// night before, so one in the morning on a Wednesday reads "Tuesday night".
export function greeting(now = new Date()) {
  const day =
    now.getHours() < 5 ? new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1) : now;
  return `${DAYS[day.getDay()]} ${dayPart(now)}`;
}

// The response types these as integers. They are coerced anyway, so a count
// is a number by the time it is interpolated, whatever the payload held.
const int = (value) => Math.max(0, Math.trunc(Number(value)) || 0);

export const count = (n, one, many) => `${n} ${n === 1 ? one : many}`;

const agentsLine = (n, none = "No") =>
  n ? `${count(n, "agent", "agents")} active` : `${none} agents active`;

function statusSentence(data) {
  const waiting = int(data.waiting);
  const unread = int(data.unread);
  const agents = int(data.agents_active);

  const waitingPart = waiting
    ? `${count(waiting, "thing is", "things are")} waiting on you.`
    : "Nothing is waiting on you.";

  const updatesPart = unread
    ? `${count(unread, "update", "updates")} since you last looked`
    : "No updates since you last looked";

  const agentsPart = agents
    ? `${count(agents, "agent is", "agents are")} working right now.`
    : "no agents are working right now.";

  return `${waitingPart} ${updatesPart}, and ${agentsPart}`;
}

// A size in the unit it reads best in, to three figures and with no trailing
// zeros, which is how the design writes them: 5.6, 1.46, 32.
const UNITS = ["B", "KB", "MB", "GB", "TB"];

function scaled(bytes) {
  let value = int(bytes);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return { text: String(Number(value.toFixed(digits))), unit: UNITS[unit] };
}

const size = (bytes) => {
  const made = scaled(bytes);
  return `${made.text} ${made.unit}`;
};

// "5.6 / 32 GB" when both sides share a unit, as the design writes it, and
// each side with its own when they do not.
export function usedOfCapacity(used, capacity) {
  const a = scaled(used);
  const b = scaled(capacity);
  return a.unit === b.unit ? `${a.text} / ${b.text} ${b.unit}` : `${a.text} ${a.unit} / ${b.text} ${b.unit}`;
}

// How long ago, in words, for the one sentence that says it in prose. The
// rows carry the compact form; a sentence reads better with the long one.
function ago(ts) {
  const then = Date.parse(ts);
  if (!Number.isFinite(then)) return "";
  const minutes = Math.floor(Math.max(0, Date.now() - then) / 60000);
  if (minutes < 1) return "just now";
  const words = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
  if (minutes < 60) return words.format(-minutes, "minute");
  if (minutes < 60 * 24) return words.format(-Math.floor(minutes / 60), "hour");
  return words.format(-Math.floor(minutes / (60 * 24)), "day");
}

const CHEVRON = (px) =>
  `<svg aria-hidden="true" width="${px}" height="${px}" viewBox="0 0 24 24" fill="none"` +
  ` stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">` +
  `<path d="M 9 6l6 6-6 6"/></svg>`;

// The unseen count is per project and covers the newest events above that
// project's cursor, so walking the newest-first list and spending each
// project's count marks exactly the rows the feed would draw as unseen.
function unseenIds(recent, unseen) {
  const left = new Map(unseen.map((entry) => [entry.project_id, int(entry.events)]));
  const ids = new Set();
  for (const event of recent) {
    const remaining = left.get(event.project_id) || 0;
    if (!remaining) continue;
    ids.add(event.id);
    left.set(event.project_id, remaining - 1);
  }
  return ids;
}

// One row for both cards. The title is the row's link, stretched over the row
// so the whole of it is the target, and the time stays above it so a press on
// the time still shows the full stamp rather than following the link.
function homeRow(event, href, { unseen = false, chevron = false, action = false } = {}) {
  const act = action ? actionFor(event) : "";
  return `<div class="row home-row${unseen ? " unread" : ""}">
    ${glyph(event.kind)}
    <div class="grow">
      <a class="title home-link" href="${esc(href)}">${esc(event.summary)}</a>
      <div class="home-meta">${esc(projectName(event))} · ${esc(event.actor)} · ${timeHTML(event.created_at)}</div>
    </div>
    ${unseen ? '<span class="dot-unread"></span><span class="sr-only">Unread</span>' : ""}
    ${act ? `<span class="home-actions">${act}</span>` : ""}
    ${chevron ? `<span class="home-chev">${CHEVRON(16)}</span>` : ""}
  </div>`;
}

// Flat rows: the waiting queue is a flat list under a group title, not a card.
function waitingCard(waiting, items) {
  if (!waiting) return "";
  const shown = items.slice(0, WAITING_ROWS);
  const rest = waiting - shown.length;
  const more =
    rest > 0
      ? `<a class="home-rest" href="#/inbox">${shown.length ? `${rest} more` : count(rest, "item", "items")} in the Inbox</a>`
      : "";
  return `<section class="home-waiting" aria-labelledby="home-waiting-title">
    <div class="home-card-head">
      <h2 id="home-waiting-title">Waiting on you · ${waiting}</h2>
      <a class="home-more" href="#/inbox">Inbox${CHEVRON(12)}</a>
    </div>
    ${shown.map((item) => homeRow(item, `#/inbox?open=${encodeURIComponent(item.event_id)}`, { chevron: true, action: true })).join("")}
    ${more}
  </section>`;
}

// Flat rows: newest across projects is a flat list without a card wrapper.
function newestCard(events, unseen) {
  if (!events.length) return "";
  const rows = events
    .map((event) =>
      homeRow(event, `#/projects/${encodeURIComponent(event.project_id)}/feed`, {
        unseen: unseen.has(event.id),
      }),
    )
    .join("");
  return `<section class="home-newest" aria-labelledby="home-newest-title">
    <h2 id="home-newest-title" class="home-section-title">Newest across projects</h2>
    <div class="home-flat-rows">${rows}</div>
  </section>`;
}

// The storage card is the only card on Home.
function storageCard(storage, prunable) {
  const used = int(storage.used_bytes);
  const capacity = storage.capacity_bytes == null ? 0 : int(storage.capacity_bytes);
  const share = capacity ? Math.min(1, used / capacity) : null;
  const sliver = share !== null && used > 0 && used < capacity * SLIVER;
  const hints = [];
  if (share !== null) hints.push(sliver ? SLIVER_WORDS : `${Math.round(share * 100)}% used`);
  const review = int(storage.knowledge_needs_review);
  if (review) hints.push(`${count(review, "page", "pages")} need review`);
  const sessions = int(prunable.sessions);
  if (sessions) {
    hints.push(
      `${count(sessions, "ended session", "ended sessions")} can be pruned · ${size(prunable.bytes)}`,
    );
  }
  return `<div class="home-storage-wrap">
    <a class="card home-card home-storage" href="#/storage">
      <span class="home-storage-head">
        <span class="home-storage-label">Storage</span>
        <span class="mono home-storage-n">${share === null ? `${size(used)} used` : usedOfCapacity(used, capacity)}</span>
      </span>
      ${
        share === null || sliver
          ? ""
          : `<span class="home-bar" aria-hidden="true"><span style="width: ${(share * 100).toFixed(1)}%"></span></span>`
      }
      ${hints.length ? `<span class="home-storage-hint">${hints.join(" · ")}</span>` : ""}
    </a>
  </div>`;
}

// Quiet is when nothing waits and nothing is new: no open item, nothing
// unread, and no event above any project's cursor.
function quietCard(data) {
  const last = data.last_event_at ? ago(data.last_event_at) : "";
  const agents = agentsLine(int(data.agents_active));
  return emptyStateHTML(
    {
      ...EMPTY_COPY.home,
      title: `Quiet ${dayPart(new Date())}.`,
      body: `${EMPTY_COPY.home.body} ${agents}${last ? `, last event ${last}` : ""}.`,
    },
    {},
    { action: "new-project" },
  );
}

function chipsHTML(waiting, unread) {
  return `<button type="button" class="chip" data-home-chip="all" aria-pressed="true">All</button>
    <button type="button" class="chip" data-home-chip="waiting" aria-pressed="false">Waiting <span class="mono">${waiting}</span></button>
    <button type="button" class="chip" data-home-chip="unread" aria-pressed="false">Unread <span class="mono">${unread}</span></button>`;
}

const HOME_STYLE = `<style>
@media (max-width: 719px) {
  /* RULE 12.1 on Home: the bar is always in layout, and its title is the
     greeting. The same string shrinks from 22/600 to 15/600 while the bar's
     height goes 76 to 52, so there is something to interpolate. Home has no
     bar at rest by having a transparent one over the greeting, not by
     removing it from layout: a display toggle cannot animate and shifts the
     content under it. Sticky, not fixed, so the bar keeps its place in the
     flow and content never slides underneath it. */
  .shell:has(.home-pad) .shell-head {
    position: sticky;
    top: 0;
    height: 76px;
    background: transparent;
    border-bottom: 0;
  }
  .shell:has(.home-pad) .shell-head .shell-title-line {
    opacity: 0;
    transition:
      opacity 120ms ease-out,
      font-size 120ms ease-out;
  }
  .shell:has(.home-pad) .shell-head .shell-meta {
    opacity: 0;
    transition: opacity 120ms ease-out;
  }
  .shell:has(.home-pad) .shell-head.is-compressed,
  .shell:has(.home-pad).is-compressed .shell-head {
    height: 52px;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }
  .shell:has(.home-pad) .shell-head.is-compressed .shell-title-line,
  .shell:has(.home-pad).is-compressed .shell-head .shell-title-line {
    opacity: 1;
  }
  .shell:has(.home-pad) .shell-head.is-compressed .shell-meta,
  .shell:has(.home-pad).is-compressed .shell-head .shell-meta {
    opacity: 0;
  }
  /* The bar is in the flow (sticky), so the body starts under it and nothing
     is drawn over anything. The greeting inside the bar fades in as the copy
     in the body fades out: one string, handed over. */
  .shell.is-compressed .home-greeting {
    opacity: 0;
    transition: opacity 120ms ease-out;
  }
  .shell:has(.home-pad) .shell-controls {
    display: flex;
    align-items: center;
    padding: 0 16px;
    box-sizing: border-box;
    position: sticky;
    top: 52px;
    z-index: 9;
    transition: height 120ms ease-out;
  }
  /* At rest the tools row holds the flow chips, which is Home's resting tools
     row. Once compressed the pinned row carries them and the flow copy hides,
     so the row's height is the only thing that changes. */
  .shell:has(.home-pad) .shell-controls {
    height: 44px;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }
  .shell:has(.home-pad) .shell-controls:not(.is-compressed) {
    height: 0;
    min-height: 0;
    padding-top: 0;
    padding-bottom: 0;
    border-bottom: 0;
    background: transparent;
    overflow: hidden;
  }
  .shell:has(.home-pad) .shell-controls:not(.is-compressed) .chip {
    display: none;
  }
  .shell.is-compressed .home-chips-flow {
    display: none !important;
  }
  .home-pad {
    padding: 0 0 48px 0;
    background: var(--bg);
  }
  .home {
    gap: 0 !important;
  }
  .home-welcome {
    padding: 0 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .home-node-phone {
    font-family: var(--font-mono);
    font-size: var(--t-12);
    font-weight: 500;
    line-height: 1;
    color: var(--ink-3);
  }
  .home-greeting {
    margin: 0;
    font-size: 28px;
    font-weight: 600;
    line-height: 1.2;
    letter-spacing: -0.02em;
    text-wrap: pretty;
    color: var(--ink);
  }
  .home-status {
    margin: 8px 0 20px !important;
    padding: 0 16px !important;
    font-size: var(--t-15) !important;
    font-weight: 400 !important;
    line-height: 1.5 !important;
    color: var(--ink-2) !important;
    text-wrap: pretty;
  }
  .home-chips-flow {
    display: flex;
    gap: 8px;
    padding: 0 16px 14px;
    overflow-x: auto;
  }
  .home-storage-wrap {
    padding: 16px;
  }
  .home-waiting .home-card-head,
  .home-newest .home-section-title {
    padding: 16px 16px 8px;
    margin: 0;
    font-size: var(--t-13);
    font-weight: 600;
    line-height: 1.3;
    color: var(--ink-2);
  }
  .home-waiting .home-card-head {
    border-bottom: 0;
  }
}
@media (min-width: 720px) {
  /* The desktop keeps its header ("Home") and reserves its control row. The
     greeting, if shown, is the first line of stage content at the stage
     gutter, never in the header. The phone's welcome-only node line and its
     flow chips stay phone chrome, so they go. */
  .home-node-phone,
  .home-chips-flow,
  .shell-controls .chip {
    display: none !important;
  }
  .home-welcome {
    display: flex;
    flex-direction: column;
    gap: 0;
  }
  .home-greeting {
    margin: 0;
    font-size: var(--t-22);
    font-weight: 600;
    line-height: 1.3;
    letter-spacing: -0.01em;
    color: var(--ink);
  }
  .home-status {
    margin: 0 !important;
    padding: 0 !important;
  }
  .home-storage-wrap {
    padding: 0;
  }
  .home-section-title {
    margin: 0;
    font-size: var(--t-13);
    font-weight: 600;
    line-height: 1.3;
    color: var(--ink-2);
    padding-bottom: var(--s-2);
  }
}
.home-storage.card {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  padding: 14px var(--s-4);
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--r-2);
  color: inherit;
  text-decoration: none;
}
.home-waiting .row:first-of-type,
.home-newest .row:first-of-type {
  border-top: 1px solid var(--line);
}
</style>`;

export function installHomeChips(root = document) {
  const syncChips = (filter) => {
    root.querySelectorAll("[data-home-chip]").forEach((c) => {
      c.setAttribute("aria-pressed", String(c.dataset.homeChip === filter));
    });
    const rows = root.querySelectorAll(".home-row");
    rows.forEach((row) => {
      if (filter === "all") {
        row.hidden = false;
      } else if (filter === "waiting") {
        const isWaiting = row.closest(".home-waiting") !== null;
        row.hidden = !isWaiting;
      } else if (filter === "unread") {
        row.hidden = !row.classList.contains("unread");
      }
    });
    for (const sec of root.querySelectorAll(".home-waiting, .home-newest")) {
      const anyVisible = sec.querySelector(".home-row:not([hidden])") !== null;
      sec.hidden = filter !== "all" && !anyVisible;
    }
  };

  root.querySelectorAll("[data-home-chip]").forEach((chip) => {
    if (chip.dataset.wired === "on") return;
    chip.dataset.wired = "on";
    chip.addEventListener("click", () => {
      syncChips(chip.dataset.homeChip);
    });
  });
}

export async function home(gen) {
  const data = await api("/api/v1/home");
  const recent = data.recent || [];
  const unseen = unseenIds(recent, data.unseen || []);
  const waiting = int(data.waiting);
  const unread = int(data.unread);
  const quiet = !waiting && !unread && !unseen.size;
  const node = [data.node?.host, data.node?.mode].filter((part) => typeof part === "string" && part);
  const cards = quiet
    ? quietCard(data)
    : waitingCard(waiting, data.waiting_items || []) +
      newestCard(
        recent.filter((event) => !isOpen(event)),
        unseen,
      );
  const status = statusSentence(data);
  const chips = chipsHTML(waiting, unread);
  // On the phone the bar carries the greeting itself, so the same string
  // shrinks from 22/600 to 15/600 as the bar collapses (RULE 12.1); the body's
  // copy of it fades out underneath. On a desktop the header title is "Home",
  // not the greeting, and the control row reserves its 40px without repeating
  // the node line under the header that already carries it.
  const welcome = `<div class="home-welcome">
    <span class="home-node-phone mono">${esc(node.join(" · "))}</span>
    <h1 class="home-greeting">${esc(greeting())}</h1>
  </div>`;

  const isPhone = !window.matchMedia("(min-width: 720px)").matches;

  paint(
    gen,
    shellHTML({
      noIndex: true,
      stageHead: shellStageHead(isPhone ? greeting() : "Home", node.join(" · ")),
      stageControls: `<div class="shell-controls">${chips}</div>`,
      stageBody: `${HOME_STYLE}<div class="shell-pad home-pad"><div class="home">${welcome}<p class="home-summary home-status">${esc(
        status,
      )}</p><div class="home-chips-flow">${chips}</div>${cards}${data.storage ? storageCard(data.storage, data.prunable || {}) : ""}</div></div>`,
    }),
  );
  installShellLayout(main);
  installHomeChips(main);
  // The router passes this to the badge, which counts the same payload.
  return data;
}

registerScreen("home", { rows: ".home-row" });
