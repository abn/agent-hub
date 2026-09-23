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

function summaryLine(data) {
  const waiting = int(data.waiting);
  return [
    waiting ? `${count(waiting, "thing", "things")} waiting on you` : "Nothing waiting on you",
    `${int(data.unread)} unread`,
    agentsLine(int(data.agents_active), "no"),
  ].join(" · ");
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

// The response carries the head of the waiting queue, newest first, so the
// card does not depend on a waiting item being among the newest events. It
// shows the first few and says how many more the Inbox holds. The count
// beside the title, and the one the rest is worked out from, is the whole
// queue's.
function waitingCard(waiting, items) {
  if (!waiting) return "";
  const shown = items.slice(0, WAITING_ROWS);
  const rest = waiting - shown.length;
  const more =
    rest > 0
      ? `<a class="home-rest" href="#/inbox">${shown.length ? `${rest} more` : count(rest, "item", "items")} in the Inbox</a>`
      : "";
  return `<section class="home-card home-waiting" aria-labelledby="home-waiting-title">
    <div class="home-card-head">
      <h2 id="home-waiting-title">Waiting on you · ${waiting}</h2>
      <a class="home-more" href="#/inbox">Inbox${CHEVRON(12)}</a>
    </div>
    ${shown.map((item) => homeRow(item, `#/inbox?open=${encodeURIComponent(item.event_id)}`, { chevron: true, action: true })).join("")}
    ${more}
  </section>`;
}

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
    <h2 id="home-newest-title">Newest across projects</h2>
    <div class="home-card">${rows}</div>
  </section>`;
}

// The bar is a drawing of the two numbers beside it, so it is hidden from a
// reader and the share it draws is written out underneath. A volume that
// could not be measured has no capacity, and then there is no bar to draw.
// Nor is there under the Storage screen's threshold: a fill to scale would be
// nothing to see, and Home holds one number, so a bar against what is used
// would always be full. The words say it instead, and no fill is ever widened.
function storageCard(storage, prunable) {
  const used = int(storage.used_bytes);
  const capacity = storage.capacity_bytes == null ? 0 : int(storage.capacity_bytes);
  const share = capacity ? Math.min(1, used / capacity) : null;
  const sliver = share !== null && used > 0 && used < capacity * SLIVER;
  const hints = [];
  if (share !== null) hints.push(sliver ? SLIVER_WORDS : `${Math.round(share * 100)}% used`);
  const sessions = int(prunable.sessions);
  if (sessions) {
    hints.push(
      `${count(sessions, "ended session", "ended sessions")} can be pruned · ${size(prunable.bytes)}`,
    );
  }
  return `<a class="home-card home-storage" href="#/storage">
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
  </a>`;
}

// The design's status strip names the node the reader is looking at. It is
// the same datum the top bar and the Storage screen print, carried on the
// Home response so the screen stays one request. The top bar already says it
// from the width it appears at, so the stylesheet draws this line below that.
function nodeLine(node) {
  const parts = [node?.host, node?.mode].filter((part) => typeof part === "string" && part);
  return parts.length ? `<p class="home-node mono">${parts.map(esc).join(" · ")}</p>` : "";
}

// Quiet is when nothing waits and nothing is new: no open item, nothing
// unread, and no event above any project's cursor.
function quietCard(data) {
  const last = data.last_event_at ? ago(data.last_event_at) : "";
  const agents = agentsLine(int(data.agents_active));
  return emptyStateHTML({
    ...EMPTY_COPY.home,
    title: `Quiet ${dayPart(new Date())}.`,
    body: `${EMPTY_COPY.home.body} ${agents}${last ? `, last event ${last}` : ""}.`,
  });
}

export async function home(gen) {
  const data = await api("/api/v1/home");
  const recent = data.recent || [];
  const unseen = unseenIds(recent, data.unseen || []);
  const waiting = int(data.waiting);
  const quiet = !waiting && !int(data.unread) && !unseen.size;
  const node = [data.node?.host, data.node?.mode].filter((part) => typeof part === "string" && part);
  const cards = quiet
    ? quietCard(data)
    : waitingCard(waiting, data.waiting_items || []) +
      newestCard(
        recent.filter((event) => !isOpen(event)),
        unseen,
      );
  // Home is a stage with no index, the same shape Settings has. It keeps the
  // reserved chrome every other pane has, so the frame does not move when the
  // reader arrives here.
  const gear = `<a class="home-gear" href="#/settings" aria-label="Settings">
      <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M 19 12a7 7 0 0 0-.2-1.6l2-1.5-2-3.4-2.3 1a7 7 0 0 0-2.8-1.6L13.3 2h-2.6l-.4 2.9a7 7 0 0 0-2.8 1.6l-2.3-1-2 3.4 2 1.5A7 7 0 0 0 5 12c0 .5.1 1.1.2 1.6l-2 1.5 2 3.4 2.3-1a7 7 0 0 0 2.8 1.6l.4 2.9h2.6l.4-2.9a7 7 0 0 0 2.8-1.6l2.3 1 2-3.4-2-1.5c.1-.5.2-1.1.2-1.6z"></path></svg>
    </a>`;
  paint(
    gen,
    shellHTML({
      noIndex: true,
      stageHead: shellStageHead(greeting(), node.join(" · "), gear),
      stageControls: `<div class="shell-controls"><span class="shell-meta">${esc(node.join(" · "))}</span></div>`,
      stageBody: `<div class="shell-pad home-pad"><div class="home"><p class="home-summary">${esc(
        summaryLine(data),
      )}</p>${cards}${data.storage ? storageCard(data.storage, data.prunable || {}) : ""}</div></div>`,
    }),
  );
  installShellLayout(main);
  // The router passes this to the badge, which counts the same payload.
  return data;
}

registerScreen("home", { rows: ".home-row" });
