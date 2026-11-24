// The Who menu: one control in the control row of the feed, the artifacts and
// search, that narrows what is shown to what one member, one agent or one
// session wrote. It sends the hub's own filters (`member`, `agent`, `session`,
// `kind`), so the hub does the narrowing and the page holds no second copy of
// the rows. One choice at a time: each is a different question, and the answer
// to two at once is rarely what the reader meant.

import { api } from "./api.mjs";
import { authorOf, foldSessions, leafName, sessionTitle } from "./author.mjs";
import { esc } from "./dom.mjs";
import { render, restoreFocusAfterRender } from "./router.mjs";
import { installGroupMenu } from "./shell-layout.mjs";

// What a choice sends. `kind` is the author kind the hub knows: the human, or a
// row from before members.
export const WHO_KEYS = ["member", "agent", "session", "kind"];

const picks = new Map();

// The scope of the Artifacts list: the hub-wide one, or one project's segment.
// Each project's segment is its own screen, so a pick made in one project
// narrows that project only, never another's and never the global list.
export const artifactsScope = (projectId = "") => (projectId ? `artifacts:${projectId}` : "artifacts");

// The choice a scope holds, as `{ key, value, label }`, or null. A scope is the
// screen the menu sits on, so the feed, each project's artifacts, the global
// artifacts list and search each remember their own and none leaks into another.
export function whoPick(scope) {
  return picks.get(scope) || null;
}

// `label` is what the choice was called when it was made, so the trigger reads
// the same on a screen whose directory does not hold the name.
export function setWho(scope, key, value, label = "") {
  if (!key || !value || !WHO_KEYS.includes(key)) picks.delete(scope);
  else picks.set(scope, { key, value, label });
}

export const whoActive = (scope) => picks.has(scope);

// The choice as query parameters, ready to follow `?` or `&`: "member=laptop".
// Empty when nothing is chosen.
export function whoParams(scope) {
  const pick = picks.get(scope);
  return pick ? `${pick.key}=${encodeURIComponent(pick.value)}` : "";
}

// The same, led by `&` for a URL that already has a query.
export const whoQuery = (scope) => (picks.has(scope) ? `&${whoParams(scope)}` : "");

// The people and sessions the menu offers, read once and kept for a short
// while: opening the menu must not cost a round trip, and a stale name here is
// only a name that is no longer worth choosing.
const FRESH_MS = 20000;
let directory = null;
let directoryAt = 0;
let directoryFor = "";

const SESSIONS_SHOWN = 30;

function agentsOf(members, sessions) {
  const seen = new Set();
  for (const member of members) if (member.enrol && member.enrol.agent) seen.add(member.enrol.agent);
  for (const session of sessions) for (const p of session.participants || []) if (p.agent) seen.add(p.agent);
  return [...seen].sort();
}

// Whether the hub still holds rows from before members. It does not say so, so
// this reads the sessions it sent: a migrated hub's sessions were started under
// a label and not a participant. A hub made after members has none.
const hasLegacy = (sessions) =>
  sessions.some((s) => authorOf({ author: s.started_by, actor: s.started_label }).author.kind === "legacy");

// `project` narrows the sessions to one project's. With none, the live ones
// across the hub: the menu is a shortcut to what is happening, not an archive.
export async function loadWho(project = "", now = Date.now()) {
  if (directory && directoryFor === project && now - directoryAt < FRESH_MS) return directory;
  const sessionsPath = project ? `/api/v1/sessions?project=${encodeURIComponent(project)}` : "/api/v1/sessions?live=1";
  const [members, sessions] = await Promise.all([
    api("/api/v1/members").then((a) => (a.members || []).filter((m) => m.state !== "pending")).catch(() => []),
    api(sessionsPath).then((a) => a.sessions || []).catch(() => []),
  ]);
  directory = { members, sessions: sessions.slice(0, SESSIONS_SHOWN), agents: agentsOf(members, sessions), legacy: hasLegacy(sessions) };
  directoryAt = now;
  directoryFor = project;
  return directory;
}

const PERSON = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="3.5"></circle><path d="M5 20c0-3.6 3.1-6 7-6s7 2.4 7 6"></path></svg>`;

const CHEVRON = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M 6 9l6 6 6-6"></path></svg>`;

// The words a choice is called, for the trigger. The name it was chosen under
// is kept with the pick; the directory only fills in a pick made before it.
function pickLabel(pick, dir) {
  if (!pick) return "";
  if (pick.label) return pick.label;
  if (pick.key === "member") {
    const member = dir.members.find((m) => m.id === pick.value);
    return member ? member.display_name || member.id : pick.value;
  }
  if (pick.key === "session") {
    const session = dir.sessions.find((s) => s.id === pick.value);
    return session ? sessionTitle(session) : pick.value;
  }
  if (pick.key === "kind") return pick.value === "human" ? "You" : pick.value === "legacy" ? "Before members" : pick.value;
  return pick.value;
}

const ended = (session) => Boolean(session.status) && session.status !== "active";

// The sessions the menu offers: each parent with its subagents set in under it
// on their last segment, and the ended ones marked and after the live ones.
function sessionItems(sessions, item) {
  // Array sort is stable, so the hub's order holds within live and within ended.
  const rows = foldSessions([...sessions].sort((a, b) => Number(ended(a)) - Number(ended(b))));
  const out = [];
  for (const { session, children } of rows) {
    const name = sessionTitle(session);
    out.push(item("session", session.id, ended(session) ? `${name} (ended)` : name, { called: name }));
    for (const kid of children) {
      const full = sessionTitle(kid);
      const label = ended(kid) ? `${full} (ended)` : full;
      out.push(item("session", kid.id, `${leafName(full) || full}${ended(kid) ? " (ended)" : ""}`, { child: true, name: label, called: full }));
    }
  }
  return out;
}

// The menu. `dir` is what `loadWho` answered, or an empty directory: the menu
// is still drawn, with Anyone and the two kinds that need no list.
export function whoMenu(scope, dir = { members: [], sessions: [], agents: [] }) {
  const pick = picks.get(scope) || null;
  // `called` is what the choice is named in the trigger once made, which is not
  // always what the row says: a subagent's row says its last segment.
  const item = (key, value, label, { child = false, name = "", called = label } = {}) => {
    const on = pick ? pick.key === key && pick.value === value : !key;
    const own = child ? ` class="who-child"${name ? ` aria-label="${esc(name)}" title="${esc(name)}"` : ""}` : "";
    return `<button type="button"${own} role="menuitemradio" aria-checked="${on}" data-action="who" data-scope="${esc(scope)}" data-key="${esc(key)}" data-value="${esc(value)}" data-label="${esc(key ? called : "")}">${esc(label)}</button>`;
  };
  const group = (label, items) =>
    items.length ? `<div role="group" aria-label="${esc(label)}"><span class="shell-menu-head" aria-hidden="true">${esc(label)}</span>${items.join("")}</div>` : "";
  // One member needs no menu of members: the hub would answer the same rows.
  const keepMember = pick && pick.key === "member";
  const members = dir.members.length > 1 || keepMember ? dir.members.map((m) => item("member", m.id, m.display_name || m.id)) : [];
  const agents = dir.agents.map((a) => item("agent", a, a));
  const sessions = sessionItems(dir.sessions, item);
  // "Before members" is offered only where the hub still holds such rows.
  const keepLegacy = dir.legacy || (pick && pick.key === "kind" && pick.value === "legacy");
  const kinds = [item("kind", "human", "You"), ...(keepLegacy ? [item("kind", "legacy", "Before members")] : [])];
  const label = pickLabel(pick, dir);
  // Nothing chosen: the person glyph and the menu chevron, named Who, as Group's
  // trigger is a word and a chevron. A choice is named in the trigger.
  const face = label
    ? `<span class="who-text">Who: <span class="who-pick">${esc(label)}</span></span>${CHEVRON}`
    : `${PERSON}${CHEVRON}`;
  return `<div class="shell-group who-menu" data-who="${esc(scope)}">
    <button type="button" class="hub-group-toggle who-toggle${label ? " is-picked" : ""}" data-group-toggle aria-haspopup="menu" aria-expanded="false"${label ? ` title="Who: ${esc(label)}"` : ` aria-label="Who" title="Who"`}>${face}</button>
    <div class="shell-group-menu who-menu-list" role="menu" aria-label="Filter by who wrote it" data-group-menu hidden>
      ${item("", "", "Anyone")}
      ${group("Members", members)}
      ${group("Agents", agents)}
      ${group("Sessions", sessions)}
      ${group("Other", kinds)}
    </div>
  </div>`;
}

// What happens when a choice changes. The feed and the artifact list repaint
// through the router; search registers its own, since it paints itself.
const handlers = new Map();

export function onWhoChange(scope, handler) {
  handlers.set(scope, handler);
}

// A screen that paints itself (search) keeps its menu through a choice, so the
// menu is drawn again in place with the new pick checked and named.
function redraw(scope) {
  const node = document.querySelector(`.who-menu[data-who="${CSS.escape(scope)}"]`);
  if (!node || !directory) return;
  const holder = document.createElement("template");
  holder.innerHTML = whoMenu(scope, directory);
  const parent = node.parentElement;
  node.replaceWith(holder.content);
  if (parent) installGroupMenu(parent);
}

function choose(button) {
  const { scope, key, value, label } = button.dataset;
  const same = whoPick(scope);
  if (same ? same.key === key && same.value === value : !key) return;
  setWho(scope, key, value, label);
  // The choice moves the reader's place by nothing: focus goes back to the
  // trigger once the screen has repainted.
  const refocus = () => /** @type {HTMLElement} */ (document.querySelector(`.who-menu[data-who="${CSS.escape(scope)}"] [data-group-toggle]`))?.focus({ preventScroll: true });
  const handler = handlers.get(scope);
  if (handler) {
    redraw(scope);
    handler();
    refocus();
    return;
  }
  restoreFocusAfterRender(refocus);
  render();
}

const items = (menu) => /** @type {HTMLElement[]} */ ([...menu.querySelectorAll("button[role='menuitemradio']")]);

// The keys: ArrowDown on the trigger opens the menu, the arrows and Home and End
// move through the choices, Escape closes it with focus on the trigger, and Tab
// leaves it closed behind the reader.
function onKey(event) {
  const target = /** @type {HTMLElement} */ (event.target);
  const wrap = target.closest?.(".who-menu");
  if (!wrap) return;
  const toggle = /** @type {HTMLElement} */ (wrap.querySelector("[data-group-toggle]"));
  const menu = /** @type {HTMLElement} */ (wrap.querySelector("[data-group-menu]"));
  if (target === toggle && menu.hidden && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
    event.preventDefault();
    toggle.click();
    return;
  }
  if (!menu.contains(target) || menu.hidden) return;
  const all = items(menu);
  const at = all.indexOf(target);
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    all[(at + (event.key === "ArrowDown" ? 1 : -1) + all.length) % all.length].focus();
  } else if (event.key === "Home" || event.key === "End") {
    event.preventDefault();
    all[event.key === "Home" ? 0 : all.length - 1].focus();
  } else if (event.key === "Escape") {
    event.preventDefault();
    menu.hidden = true;
    toggle.setAttribute("aria-expanded", "false");
    toggle.focus();
  } else if (event.key === "Tab") {
    menu.hidden = true;
    toggle.setAttribute("aria-expanded", "false");
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("keydown", onKey);
  document.addEventListener("click", (event) => {
    const button = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.("button[data-action='who']"));
    if (button) choose(button);
  });
}
