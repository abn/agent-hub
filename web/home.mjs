// Home: the day at a glance. What waits on you, the newest events across every
// project, and what the node holds. Everything here is read off the one Home
// response, so painting the screen is one request however much it shows.

import { api } from "./api.mjs";
import { esc, glyph, isOpen, paint } from "./dom.mjs";
import { emptyStateHTML, EMPTY_COPY } from "./empty.mjs";
import { registerScreen } from "./keys.mjs";
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

const count = (n, one, many) => `${n} ${n === 1 ? one : many}`;

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
function homeRow(event, href, { unseen = false, chevron = false } = {}) {
  return `<div class="row home-row${unseen ? " unread" : ""}">
    ${glyph(event.kind)}
    <div class="grow">
      <a class="title home-link" href="${esc(href)}">${esc(event.summary)}</a>
      <div class="home-meta">${esc(event.project_id)} · ${esc(event.actor)} · ${timeHTML(event.created_at)}</div>
    </div>
    ${unseen ? '<span class="dot-unread"></span><span class="sr-only">Unread</span>' : ""}
    ${chevron ? `<span class="home-chev">${CHEVRON(16)}</span>` : ""}
  </div>`;
}

// The response carries the newest events rather than the waiting queue, so
// the card lists the waiting items among them and says how many more the
// Inbox holds. The count beside the title is the whole queue either way.
function waitingCard(waiting, open) {
  if (!waiting) return "";
  const shown = open.slice(0, WAITING_ROWS);
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
    ${shown.map((event) => homeRow(event, "#/inbox", { chevron: true })).join("")}
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
function storageCard(storage, prunable) {
  const used = int(storage.used_bytes);
  const capacity = storage.capacity_bytes == null ? 0 : int(storage.capacity_bytes);
  const share = capacity ? Math.min(1, used / capacity) : null;
  const hints = [];
  if (share !== null) {
    hints.push(used && share < 0.01 ? "Less than 1% used" : `${Math.round(share * 100)}% used`);
  }
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
      share === null
        ? ""
        : `<span class="home-bar" aria-hidden="true"><span style="width: ${(share * 100).toFixed(1)}%"></span></span>`
    }
    ${hints.length ? `<span class="home-storage-hint">${hints.join(" · ")}</span>` : ""}
  </a>`;
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
  paint(
    gen,
    `<div class="home">
      <header>
        <h1>${esc(greeting())}</h1>
        <p class="home-summary">${summaryLine(data)}</p>
      </header>
      ${
        quiet
          ? quietCard(data)
          : waitingCard(waiting, recent.filter(isOpen)) +
            newestCard(
              recent.filter((event) => !isOpen(event)),
              unseen,
            )
      }
      ${data.storage ? storageCard(data.storage, data.prunable || {}) : ""}
    </div>`,
  );
  // The router passes this to the badge, which counts the same payload.
  return data;
}

registerScreen("home", { rows: ".home-row" });
