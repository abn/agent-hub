// What Now says, worked out from the one Home response: which asks need the
// reader now and which can wait, what is new since they looked and what is
// quiet, and the sentences that open and close the stream. Pure functions over
// plain data, so the words are held by unit tests rather than by a page.

import { esc } from "./dom.mjs";
import { placeTime } from "./time.mjs";

export { clockTime, placeTime } from "./time.mjs";

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

// An ask whose deadline is more than a day away can wait: it is still the
// reader's, but nothing breaks before tomorrow.
export const LATER_MS = DAY;

// Snoozing is remembered on this device only, under one key that Now, an opened
// item and the badge all read, so a snooze holds in every one of them.
export const SNOOZE_KEY = "hub.snooze";
export const SNOOZE_MS = HOUR;

function store() {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

// The live snoozes, by item id. An expired one is dropped as it is read.
export function readSnoozes(now = Date.now(), storage = store()) {
  let map = {};
  try {
    map = JSON.parse(storage?.getItem(SNOOZE_KEY) || "{}") || {};
  } catch {
    map = {};
  }
  const live = {};
  let dropped = false;
  for (const [id, until] of Object.entries(map)) {
    if (Number(until) > now) live[id] = Number(until);
    else dropped = true;
  }
  if (dropped) writeSnoozes(live, storage);
  return live;
}

function writeSnoozes(map, storage = store()) {
  try {
    storage?.setItem(SNOOZE_KEY, JSON.stringify(map));
  } catch {}
}

export function snooze(id, now = Date.now(), storage = store()) {
  const map = readSnoozes(now, storage);
  map[id] = now + SNOOZE_MS;
  writeSnoozes(map, storage);
}

export function unsnooze(id, now = Date.now(), storage = store()) {
  const map = readSnoozes(now, storage);
  delete map[id];
  writeSnoozes(map, storage);
}

export const waits = (item) => item.status === "action" || item.status === "waiting";

const int = (value) => Math.max(0, Math.trunc(Number(value)) || 0);

// Whether an open ask can wait: snoozed on this device, or given a deadline
// more than a day off.
export function canWait(item, { now = Date.now(), snoozes = {} } = {}) {
  if (snoozes[item.event_id] > now) return true;
  const due = Date.parse(item.expires_at || "");
  return Number.isFinite(due) && due - now > LATER_MS;
}

// The asks, split. Each keeps the hub's order, newest first.
export function needsYou(data, options = {}) {
  const items = (data.waiting_items || []).filter(waits);
  const now = [];
  const later = [];
  for (const item of items) (canWait(item, options) ? later : now).push(item);
  // An ask the response did not carry is counted as needing the reader: the
  // list is cut, not the queue, and nothing says it can wait.
  const unlisted = Math.max(0, int(data.waiting) - items.length);
  return { now, later, unlisted };
}

// The badge and the rail count: the asks that need the reader now.
export function needsYouCount(data, options = {}) {
  const split = needsYou(data, options);
  return split.now.length + split.unlisted;
}

// A feed event that still waits is an ask, and Now shows it in the first tier.
const isOpen = (event) => event.inbox_status === "action" || event.inbox_status === "waiting";

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

// What happened, split into what is new since the reader looked, grouped by
// project, and the quiet rest. New is above a project's read cursor, or
// finished work still unread in the inbox. A project whose unseen count is
// larger than the rows the response carried says how many more it holds.
export function sinceAndQuiet(data) {
  const recent = (data.recent || []).filter((event) => !isOpen(event));
  const unseen = data.unseen || [];
  const above = unseenIds(data.recent || [], unseen);
  const groups = new Map();
  const quiet = [];
  const group = (id, name) => {
    if (!groups.has(id)) groups.set(id, { id, name: name || id, rows: [], more: 0 });
    return groups.get(id);
  };
  for (const event of recent) {
    if (above.has(event.id) || event.inbox_status === "unread") {
      group(event.project_id, event.project_display_name).rows.push(event);
    } else {
      quiet.push(event);
    }
  }
  for (const entry of unseen) {
    const shown = (data.recent || []).filter((event) => above.has(event.id) && event.project_id === entry.project_id).length;
    const more = int(entry.events) - shown;
    if (more > 0) group(entry.project_id, entry.project_display_name).more = more;
  }
  const since = [...groups.values()];
  const count = since.reduce((sum, g) => sum + g.rows.length + g.more, 0);
  return { since, quiet, count };
}

// The oldest time the since tier reaches back to, or null when it is empty.
export function sinceOldest(since) {
  let oldest = null;
  for (const g of since) {
    for (const event of g.rows) {
      const ms = Date.parse(event.created_at);
      if (Number.isFinite(ms) && (oldest === null || ms < oldest)) oldest = ms;
    }
  }
  return oldest;
}

const WORDS = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];

// A count in a sentence: a word up to twelve, digits after.
export const word = (n) => (n < WORDS.length ? WORDS[n] : String(n));

const capital = (text) => text.charAt(0).toUpperCase() + text.slice(1);

// A summary cut at a word boundary so one ask cannot take over the sentence.
export function clip(text, max = 72) {
  const flat = String(text ?? "").replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  const cut = flat.slice(0, max);
  const space = cut.lastIndexOf(" ");
  return `${(space > max / 2 ? cut.slice(0, space) : cut).replace(/[\s,;:.]+$/, "")}…`;
}

const ENDED = /[.?!…]$/;

// One ask in the opening line: who, in semibold, and what they want, quoted.
function askClause(item) {
  const said = clip(item.summary);
  const quoted = `“${esc(said)}”`;
  const who = `<b>${esc(item.actor)}</b>`;
  if (item.kind === "approval") return { html: `${who} wants a decision on ${quoted}`, said };
  if (item.kind === "question") return { html: `${who} asks ${quoted}`, said };
  if (item.kind === "idea") return { html: `${who} left an idea, ${quoted}`, said };
  return { html: `${who} is waiting on ${quoted}`, said };
}

// The asks the opening line names: one per agent, in the hub's order, two at
// most, so two agents each get their say before one agent's second ask.
function named(items) {
  const seen = new Set();
  const picked = [];
  for (const item of items) {
    if (seen.has(item.actor)) continue;
    seen.add(item.actor);
    picked.push(item);
    if (picked.length === 2) break;
  }
  return picked;
}

// The opening line, as markup. Every value an agent wrote is escaped here, so
// the caller can put the result straight into the page.
//
//   nothing waits   "Nothing waits on you. Nothing is new since you looked."
//   one ask         "One agent needs you. build-bot asks “Ship it?”"
//   two agents      "Two agents need you. a wants a decision on “x”, and b asks “y?”"
//   more            "... and three more." after the two it names
export function summaryHTML({ now = [], later = 0, since = 0, agents = 0, unlisted = 0 } = {}) {
  const total = now.length + unlisted;
  const parts = [];
  const rest = [];
  if (!total) {
    parts.push("Nothing waits on you.");
  } else {
    const actors = new Set(now.map((item) => item.actor)).size || 1;
    const lead = `${capital(word(actors))} ${actors === 1 ? "agent needs" : "agents need"} you`;
    parts.push(actors === total ? `${lead}.` : `${lead}, with ${word(total)} asks.`);
    const clauses = named(now).map(askClause);
    const left = total - clauses.length;
    if (clauses.length) {
      let line = clauses.map((clause) => clause.html).join(clauses.length === 2 && !left ? ", and " : ", ");
      if (left) line += `, and ${word(left)} more`;
      const last = clauses[clauses.length - 1];
      parts.push(left || !ENDED.test(last.said) ? `${line}.` : line);
    }
  }
  if (later) rest.push(`${capital(word(later))} more can wait.`);
  if (since) rest.push(`${capital(word(since))} ${since === 1 ? "thing" : "things"} changed since you looked.`);
  else rest.push(total || later ? "Nothing else is new." : "Nothing is new since you looked.");
  if (!total && !later && agents) {
    rest.push(`${capital(word(agents))} ${agents === 1 ? "agent is" : "agents are"} working.`);
  }
  return `${parts.join(" ")} <span class="now-rest">${rest.join(" ")}</span>`;
}

// The closing line, plain text: how far back the stream reaches, and what is
// left to do.
// `scope` is the chip the reader has pressed: a line under Needs you speaks of
// what waits on them and not of the stream, and an ask may be an approval, so
// the count is of asks and not of answers.
export function endingLine({ needs = 0, oldest = null, now = Date.now(), scope = "" } = {}) {
  if (scope === "needs") return "That is every ask waiting on you.";
  const reach = oldest === null ? "That is everything." : `That is everything since ${placeTime(oldest, now)}.`;
  if (!needs) return reach;
  return `${reach} ${capital(word(needs))} ${needs === 1 ? "ask" : "asks"} to go and the desk is clear.`;
}

// What each kind did, for the sentence a row reads as.
// An artifact's, a session's and a score's summary already says what was done
// ("published Check note"), so those take no verb of their own.
const DID = {
  finished: "finished",
  signal: "noted",
  answer: "answered",
  question: "asked",
  approval: "asked for a decision",
  idea: "left an idea",
  anchor: "marked a phase",
};

// A row's sentence, as markup: the agent in semibold and what it did.
export function eventSentenceHTML(event) {
  const did = Object.prototype.hasOwnProperty.call(DID, event.kind) ? DID[event.kind] : "";
  const who = `<b>${esc(event.actor)}</b>`;
  return `${who}${did ? ` ${did}` : ""}: ${esc(event.summary)}`;
}

// A summary the feed has turned into what was done: "started rel-1.10",
// "published Notes", "asked: Roll the logs?".
const DONE = /^(?:[a-z]+ed|picked up)\b/;

// A feed row's sentence, as markup. An ask that is still open reads as it does
// on Now. Anything else is the agent and what it did: `phrase` is the summary
// as the feed words it, and one that already says what was done follows the
// name directly. A signal's words are the agent's own, so it is never read as
// a verb, and a summary that does not say what was done reads as Now reads it.
export function feedSentenceHTML(event, phrase = event.summary) {
  if (isOpen(event)) return askSentenceHTML(event);
  const said = String(phrase ?? "").trim().replace(/^([A-Z])(?=[a-z]+ed\b)/, (c) => c.toLowerCase());
  if (event.kind !== "signal" && DONE.test(said)) return `<b>${esc(event.actor)}</b> ${esc(said)}`;
  return eventSentenceHTML(event);
}

// An ask's sentence, as markup: who wants what.
export function askSentenceHTML(item) {
  const who = `<b>${esc(item.actor)}</b>`;
  const want =
    item.kind === "approval"
      ? "wants a decision:"
      : item.kind === "question"
        ? "asks:"
        : item.kind === "idea"
          ? "left an idea:"
          : "is waiting on you:";
  return `${who} ${want} ${esc(item.summary)}`;
}

// The asks an opened item moves through with previous and next: what needs the
// reader now, then what can wait, the order Now draws them in. The items come
// from more than one list, so each is taken once and put back in the hub's
// order, newest first by id.
export function askQueue(items, options = {}) {
  const byId = new Map();
  for (const item of items) if (waits(item) && !byId.has(item.event_id)) byId.set(item.event_id, item);
  const open = [...byId.values()].sort((a, b) => (a.event_id < b.event_id ? 1 : a.event_id > b.event_id ? -1 : 0));
  const { now, later } = needsYou({ waiting_items: open, waiting: open.length }, options);
  return { now, later, all: [...now, ...later] };
}

// Where one item sits in the queue: the header's words, the items either side,
// and the one that opens once this one is answered. An item outside the queue
// (answered, or not an ask) has no place, and its next is the head of the queue.
export function queuePlace(queue, id) {
  const all = queue.all;
  const at = all.findIndex((item) => item.event_id === id);
  if (at < 0) return { place: "", previous: null, next: all[0] || null, after: all[0] || null };
  const previous = at > 0 ? all[at - 1] : null;
  const next = at < all.length - 1 ? all[at + 1] : null;
  // The last ask answered hands over to the first one still waiting.
  const after = next || all.find((item) => item.event_id !== id) || null;
  const place =
    at < queue.now.length
      ? `${at + 1} of ${queue.now.length} need you`
      : `${at - queue.now.length + 1} of ${queue.later.length} can wait`;
  return { place, previous, next, after };
}

// Other asks worth reading beside this one: those from the same session
// first, then those from the same project, each in queue order.
export function relatedAsks(item, queue, limit = 4) {
  const others = queue.all.filter((other) => other.event_id !== item.event_id);
  const session = item.session_id ? others.filter((other) => other.session_id === item.session_id) : [];
  const project = others.filter((other) => other.project_id === item.project_id && !session.includes(other));
  return [
    ...session.map((other) => ({ item: other, why: "session" })),
    ...project.map((other) => ({ item: other, why: "project" })),
  ].slice(0, limit);
}

// The replies on an item's thread, oldest first, from a page of the project's
// feed read forward from the item.
export function threadReplies(id, events = []) {
  return events
    .filter((event) => event.thread_id === id && event.id !== id)
    .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

// A reply on a thread, as markup: who, and what they said. An answer reads its
// body, which is the decision or the words sent; anything else adds to the ask.
export function replySentenceHTML(event) {
  const who = `<b>${esc(event.actor)}</b>`;
  const body = typeof event.payload?.body === "string" && event.payload.body.trim() ? event.payload.body.trim() : "";
  if (event.kind === "answer") return `${who} answered: ${esc(body || event.summary)}`;
  return `${who} added: ${esc(body || event.summary)}`;
}
