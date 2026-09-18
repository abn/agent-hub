// Timestamps as the screens read them. The hub writes RFC 3339; a row shows
// how long ago it was, and the full local time is one press or one hover away,
// and is what a reader hears when the row is read.

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
// A minute is the finest thing the compact form says, so half of one is often
// enough. Every mounted element is visited on a tick, so the cost of the tick
// is what keeps a feed of several hundred rows cheap.
const TICK = 30 * SECOND;
const SELECTOR = "time.ts[datetime]";
// What a row shows when the field is missing or is not a time at all. Better
// an honest word than "NaN" or "Invalid Date".
const UNKNOWN = "unknown";

// An Intl formatter costs far more to build than to call, and the tick calls
// them once per mounted row. They are built on first use and kept.
const formatters = new Map();

function formatter(key, build) {
  let made = formatters.get(key);
  if (made === undefined) {
    try {
      made = build();
    } catch {
      made = null;
    }
    formatters.set(key, made);
  }
  return made;
}

// The reader's own locale throughout: `undefined` is what asks Intl for it.
function units(value, unit) {
  const made = formatter(`unit:${unit}`, () =>
    new Intl.NumberFormat(undefined, { style: "unit", unit, unitDisplay: "narrow" }),
  );
  return made ? made.format(value) : `${value}${unit.charAt(0)}`;
}

function relativeWords(value, unit, style) {
  const made = formatter(`rel:${style}`, () =>
    new Intl.RelativeTimeFormat(undefined, { numeric: "auto", style }),
  );
  return made ? made.format(value, unit) : "";
}

function capital(text) {
  return text ? text.charAt(0).toUpperCase() + text.slice(1) : text;
}

function shortDate(date, now) {
  const sameYear = date.getFullYear() === now.getFullYear();
  const made = formatter(`date:${sameYear}`, () =>
    new Intl.DateTimeFormat(
      undefined,
      sameYear
        ? { month: "short", day: "numeric" }
        : { year: "numeric", month: "short", day: "numeric" },
    ),
  );
  return made ? made.format(date) : date.toDateString();
}

// The whole local timestamp, spelled out: the accessible name, the hover
// title, and what a press swaps in for a reader who cannot hover.
export function fullStamp(ms) {
  const made = formatter("full", () =>
    new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" }),
  );
  const date = new Date(ms);
  return made ? made.format(date) : date.toString();
}

function parse(ts) {
  const ms = typeof ts === "number" ? ts : Date.parse(ts);
  return Number.isFinite(ms) ? ms : null;
}

// Whole days between two instants by the calendar, not by the clock: 23:50 and
// 00:10 are a day apart even though ten minutes passed.
function calendarGap(ms, nowMs) {
  const then = new Date(ms);
  const now = new Date(nowMs);
  const thenDay = new Date(then.getFullYear(), then.getMonth(), then.getDate());
  const nowDay = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  return Math.round((nowDay - thenDay) / DAY);
}

// The compact form the rows carry: now, 4m, 2h, Yesterday, then a short date.
// A timestamp in the future is a clock that disagrees rather than a thing that
// happened, so it is spelled out rather than compacted.
export function relative(ms, nowMs = Date.now()) {
  const gap = ms - nowMs;
  const away = Math.abs(gap);
  if (away < 45 * SECOND) return relativeWords(0, "second", "long") || "now";
  if (away < HOUR) {
    const minutes = Math.max(1, Math.round(away / MINUTE));
    return gap > 0 ? relativeWords(minutes, "minute", "narrow") : units(minutes, "minute");
  }
  if (away < DAY) {
    const hours = Math.max(1, Math.round(away / HOUR));
    return gap > 0 ? relativeWords(hours, "hour", "narrow") : units(hours, "hour");
  }
  const days = calendarGap(ms, nowMs);
  if (days === 1 || days === -1) return capital(relativeWords(-days, "day", "long"));
  return shortDate(new Date(ms), new Date(nowMs));
}

// Every mounted element, and the instant each one shows. The set is swept on
// every tick, so an element the screen painted over leaves it without anyone
// unmounting it by hand.
const live = new Set();
const shownAt = new WeakMap();
let timer = 0;

function tick() {
  const now = Date.now();
  for (const element of live) {
    if (!element.isConnected) {
      live.delete(element);
      continue;
    }
    if (element.dataset.full === "true") continue;
    const text = relative(shownAt.get(element), now);
    if (element.textContent !== text) element.textContent = text;
  }
  if (live.size === 0) stopTicking();
}

function startTicking() {
  if (timer || document.hidden || live.size === 0) return;
  timer = setInterval(tick, TICK);
}

function stopTicking() {
  clearInterval(timer);
  timer = 0;
}

function mount(element, ms) {
  shownAt.set(element, ms);
  live.add(element);
  startTicking();
}

function toggle(element) {
  const full = element.getAttribute("aria-label") || "";
  if (element.dataset.full === "true") {
    delete element.dataset.full;
    element.textContent = relative(shownAt.get(element) ?? Date.parse(element.dateTime));
    return;
  }
  element.dataset.full = "true";
  element.textContent = full;
}

function adopt(element) {
  if (live.has(element)) return;
  const ms = parse(element.dateTime);
  if (ms === null) return;
  mount(element, ms);
}

// Adopt every element under a root that the ticker does not hold yet.
export function upgradeTimes(root) {
  if (!root || root.nodeType !== 1) return;
  if (root.matches(SELECTOR)) adopt(root);
  for (const element of root.querySelectorAll(SELECTOR)) adopt(element);
}

let installed = false;

// The screens paint a whole card as one innerHTML string, so an element built
// by timeHTML never passes through a caller that could register it. One
// observer adopts them as they land, and one delegated listener gives every
// instance its press, so neither form needs a screen to remember anything.
function install() {
  if (installed || !document.body) return;
  installed = true;
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) upgradeTimes(node);
    }
  }).observe(document.body, { childList: true, subtree: true });
  document.addEventListener("click", (event) => {
    const element = event.target.closest?.(SELECTOR);
    if (element) toggle(element);
  });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      stopTicking();
      return;
    }
    tick();
    startTicking();
  });
}

// The element both forms are built from. It is a real `<time>` so the machine
// timestamp stays machine-readable, and it is not a stop of its own: a screen
// carries one per row, and a hundred identical date stops would be the whole
// tab ring. The full stamp reaches a reader through the accessible name, which
// is read wherever the row is read; a pointer gets it by pressing.
function build(ts) {
  install();
  const ms = parse(ts);
  if (ms === null) {
    const fallback = document.createElement("span");
    fallback.className = "ts-unknown";
    fallback.textContent = UNKNOWN;
    return { element: fallback, ms: null };
  }
  const element = document.createElement("time");
  element.className = "ts";
  element.dateTime = new Date(ms).toISOString();
  const full = fullStamp(ms);
  element.title = full;
  element.setAttribute("aria-label", full);
  element.textContent = relative(ms);
  return { element, ms };
}

// The DOM-node form, for a screen that builds its rows out of nodes.
export function timeNode(ts) {
  const { element, ms } = build(ts);
  if (ms !== null) mount(element, ms);
  return element;
}

// The string form, for the screens that paint a card as one innerHTML string.
// Serialising the element is what escapes it: the browser's own serialiser
// quotes the attributes and the text, and every value here is machine-made
// anyway (an ISO instant and Intl output), never a field an agent wrote.
export function timeHTML(ts) {
  return build(ts).element.outerHTML;
}

// A day bucket for grouping: Today, Yesterday, or the date.
export function dayOf(ts, now) {
  const date = new Date(ts);
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  const diff = Math.round((start - day) / 86400000);
  if (diff <= 0) return "Today";
  if (diff === 1) return "Yesterday";
  return date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

// Group events into day buckets, newest day first, preserving order inside a
// day. The feed and Home both use this, so their shape stays identical.
export function byDay(events) {
  const now = new Date();
  const groups = [];
  for (const event of events) {
    const label = dayOf(event.created_at, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.events.push(event);
    else groups.push({ label, events: [event] });
  }
  return groups;
}
