// Who wrote a row, as a reader sees it. Every write-bearing row carries a typed
// `author` (participant, human, local or legacy) and an `origin` that is null
// for a caller's own write and "hub" for the hub's. This module is the one
// place that turns the pair into words, so a feed row, an ask, a comment and a
// version line cannot disagree.
//
// A byline is a subject and receipts. The subject is what the sentence is
// about, set in 600: the session when the write named one, else the agent.
// Receipts are `agent · member` in mono. The wire's `actor` string is an
// opaque key and is never read here except to give a row from before the author
// record its old label.

import { api } from "./api.mjs";
import { esc } from "./dom.mjs";

// What a row written before members says beside its subject, in the receipts'
// place, so the reason it has no agent or member is read rather than hovered.
export const LEGACY_RECEIPT = "before members";

// How many active members the hub has. One member needs no naming beside every
// row, so receipts drop the member while the answer is one. Unknown is not one.
let members = null;

export function setMemberCount(n) {
  members = Number.isFinite(n) && n >= 0 ? Math.trunc(n) : null;
}

export const soloHub = () => members === 1;

// The hub's member count, read once a minute and whenever a screen that changes
// it asks. A hub that will not say leaves the count unknown, and receipts then
// name the member, which is the safe side of the rule.
let primedAt = 0;
export async function primeMembers({ force = false, now = Date.now() } = {}) {
  if (!force && primedAt && now - primedAt < 60000) return;
  primedAt = now;
  try {
    const answer = await api("/api/v1/members");
    const rows = Array.isArray(answer && answer.members) ? answer.members : [];
    setMemberCount(rows.filter((row) => row.state !== "pending").length);
  } catch {
    primedAt = 0;
  }
}

// The author and origin a row carries. A row from a hub that has not sent an
// author record (or a comment whose `author` is still the old string) reads as
// legacy with the label it had.
export function authorOf(row) {
  const author = row && row.author;
  if (author && typeof author === "object" && typeof author.kind === "string") {
    return { author, origin: row.origin ?? null };
  }
  const label = typeof author === "string" ? author : typeof row?.actor === "string" ? row.actor : "";
  return { author: { kind: "legacy", label }, origin: row?.origin ?? null };
}

const text = (value) => (typeof value === "string" ? value.trim() : "");

// The words for one author. `kind` says which rule produced them, so a screen
// can draw the human or the hub differently from a participant.
export function byline(author, origin = null, { solo = soloHub() } = {}) {
  const a = author && typeof author === "object" ? author : { kind: "legacy", label: text(author) };
  if (a.kind === "human") {
    return origin === "hub"
      ? { kind: "hub", subject: "the hub", receipts: "" }
      : { kind: "human", subject: "you", receipts: "" };
  }
  if (a.kind === "legacy") {
    const label = text(a.label);
    // Before members the human wrote as "human" and the hub as "hub"; those two
    // were never agents, so they keep the words a reader already knows.
    if (label === "human" || label === "you") return { kind: "human", subject: "you", receipts: LEGACY_RECEIPT };
    if (label === "hub") return { kind: "hub", subject: "the hub", receipts: LEGACY_RECEIPT };
    // With no label at all the subject already says it.
    return { kind: "legacy", subject: label || LEGACY_RECEIPT, receipts: label ? LEGACY_RECEIPT : "" };
  }
  const agent = text(a.agent);
  const session = text(a.session && a.session.name);
  const local = a.kind === "local";
  const member = local ? "local" : text(a.member);
  // The member is named beside the agent unless the hub has only one, and the
  // embedded store's `local` is always named, since it is not a member at all.
  const shownMember = local || !solo ? member : "";
  if (session) {
    return {
      kind: a.kind,
      subject: session,
      receipts: [agent, shownMember].filter(Boolean).join(" · "),
    };
  }
  const subject = agent || member || "unknown";
  return { kind: a.kind, subject, receipts: subject === shownMember ? "" : shownMember };
}

export const bylineOf = (row, options) => {
  const { author, origin } = authorOf(row);
  return byline(author, origin, options);
};

// Whether a row was written by the human, who is "you" in a sentence, and
// whether it was written by someone who is not an agent: the human or the hub.
// Neither is a session, so neither is counted as one.
export const isYou = (row, options) => bylineOf(row, options).kind === "human";
export const isOutsider = (row, options) => {
  const kind = bylineOf(row, options).kind;
  return kind === "human" || kind === "hub";
};

// A verb that agrees with the row's subject: "you ask" but "claude-code asks",
// "you are" but "build-bot is".
export const verb = (row, third, second) => (isYou(row) ? second : third);

// The subject of a sentence, in semibold and escaped. Agent text wraps: a long
// session name breaks at its dashes, slashes, dots and underscores first, and
// inside a word only when nothing else fits. It is never cut with an ellipsis.
export function subjectHTML(row, options) {
  const b = bylineOf(row, options);
  const parts = b.subject.split(/(?<=[-/._])/).map(esc);
  return `<b class="by-s">${parts.join("<wbr>")}</b>`;
}

// The receipts, as a mono run to sit after a sentence or in a margin. Empty
// when there are none, so a caller can append it unconditionally.
export function receiptsHTML(row, options) {
  const { receipts } = bylineOf(row, options);
  return receipts ? `<span class="by-r mono">${esc(receipts)}</span>` : "";
}

// The subject and receipts as one plain string, for an aria-label or a title.
export function bylineText(row, options) {
  const b = bylineOf(row, options);
  return b.receipts ? `${b.subject} (${b.receipts})` : b.subject;
}

// Whether a row names the session that wrote it.
export const hasSession = (row) => {
  const { author } = authorOf(row);
  return Boolean(author.session && author.session.id);
};

// What makes two rows "the same asker": the session when there is one, else the
// participant. Used where a sentence names one ask per asker.
export function askerKey(row) {
  const { author } = authorOf(row);
  if (author.kind === "human") return "human";
  if (author.kind === "legacy") return `label:${text(author.label)}`;
  if (author.session && author.session.id) return `session:${author.session.id}`;
  return `${author.kind}:${text(author.member)}/${text(author.agent)}`;
}

// The participants of a session as receipts: agents grouped under their member,
// "claude-code, opencode · laptop". Participants who have left are not listed
// unless the caller asks for everyone who ever took part.
export function participantsText(participants, { solo = soloHub(), includeLeft = false } = {}) {
  const byMember = new Map();
  for (const p of participants || []) {
    if (!p || (p.left_at && !includeLeft)) continue;
    const member = text(p.member);
    const agent = text(p.agent);
    if (!byMember.has(member)) byMember.set(member, []);
    if (agent && !byMember.get(member).includes(agent)) byMember.get(member).push(agent);
  }
  const parts = [];
  for (const [member, agents] of byMember) {
    const named = agents.join(", ");
    const who = solo || !member ? "" : member;
    parts.push([named, who].filter(Boolean).join(" · "));
  }
  return parts.filter(Boolean).join(", ");
}

// A session's name as its last segment: a child of `feat-x` called
// `feat-x/review-3fa9` reads as `review-3fa9` under its parent.
export const leafName = (name) => {
  const s = text(name);
  const at = s.lastIndexOf("/");
  return at < 0 ? s : s.slice(at + 1);
};

const subagents = (n) => `+${n} ${n === 1 ? "subagent" : "subagents"}`;

// Sessions in a list, children folded under their parent. A child whose parent
// is not in the list stands alone under its full name, because its leaf would
// say nothing. Returns `[{ session, children, folded }]` in the list's order,
// where `folded` is the "+3 subagents" mark ("" when there are none). A list
// the hub filtered to live sessions says `live`: its parents' `children_count`
// also counts ended children, so only the children listed are folded.
export function foldSessions(sessions, { live = false } = {}) {
  const rows = Array.isArray(sessions) ? sessions : [];
  const byId = new Map(rows.map((s) => [s.id, s]));
  const children = new Map();
  const top = [];
  for (const s of rows) {
    const parent = s.parent_id && byId.has(s.parent_id) ? s.parent_id : null;
    if (parent) {
      if (!children.has(parent)) children.set(parent, []);
      children.get(parent).push(s);
    } else {
      top.push(s);
    }
  }
  return top.map((session) => {
    const kids = children.get(session.id) || [];
    // The hub counts children it did not list (a list split by status).
    const n = live ? kids.length : kids.length || Math.max(0, Math.trunc(Number(session.children_count)) || 0);
    return { session, children: kids, folded: n ? subagents(n) : "" };
  });
}

// What a list of sessions holds, counted as the hub counts: a session with a
// parent is a subagent, whether or not its parent is in the list.
export function liveCounts(sessions) {
  const rows = Array.isArray(sessions) ? sessions : [];
  const subagents = rows.filter((s) => s && s.parent_id).length;
  return { sessions: rows.length - subagents, subagents };
}

// What a project's list holds, counted the way its rows show it: the sessions
// that head a row, and the subagents folded under them.
function rowCounts(sessions) {
  const rows = foldSessions(sessions);
  const subagents = rows.reduce(
    (sum, row) => sum + (row.children.length || Math.max(0, Math.trunc(Number(row.session.children_count)) || 0)),
    0,
  );
  return { sessions: rows.length, subagents };
}

const sessionWord = (n) => `${n} ${n === 1 ? "session" : "sessions"}`;
const subagentWord = (n) => `${n} ${n === 1 ? "subagent" : "subagents"}`;

// "2 sessions, 1 subagent": the counts over a list, as its rows show them.
export function sessionCountText(sessions) {
  const { sessions: n, subagents } = rowCounts(sessions);
  return subagents ? `${sessionWord(n)}, ${subagentWord(subagents)}` : sessionWord(n);
}

// "2 live sessions, 1 subagent": the hub's two counts said one way everywhere.
// The subagents are left out when there are none.
export function liveWords(sessions, subagents = 0) {
  const n = Math.max(0, Math.trunc(Number(sessions)) || 0);
  const m = Math.max(0, Math.trunc(Number(subagents)) || 0);
  const head = `${n} live ${n === 1 ? "session" : "sessions"}`;
  return m ? `${head}, ${subagentWord(m)}` : head;
}

// The head over the hub's list of live sessions, split the way the hub splits
// its own counts.
export function liveCountText(sessions) {
  const { sessions: n, subagents } = liveCounts(sessions);
  return n ? liveWords(n, subagents) : subagents ? liveWords(0, subagents) : "No live sessions";
}

// A session is live when the hub says so: it is active and a participant who
// has not left was seen inside the hub's activity window. An active session
// that is not live is idle: it is open, and nobody is there.
export const isLive = (session) => Boolean(session && session.status === "active" && session.live === true);

// The word a session's state is read by: live, idle (active, nobody seen
// lately), or the status itself once it is no longer active.
export const stateWord = (session) =>
  session && session.status === "active" ? (isLive(session) ? "live" : "idle") : (session && session.status) || "";

// A session with no name, read by its agent and the tail of its id. The head of
// a ULID is its timestamp, shared by every session made that minute.
export function sessionTitle(session) {
  const name = text(session && session.session_name);
  if (name) return name;
  const id = text(session && session.id);
  const agent = text(session && session.participants && session.participants[0] && session.participants[0].agent);
  const tail = id ? `…${id.slice(-6)}` : "";
  return [agent, tail].filter(Boolean).join(" ") || "session";
}
