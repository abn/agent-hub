// Pure text rules of the hook contract: notice lines, the wake decision and the
// <agent-hub> block. Lengths count Unicode scalar values.

export const WAKE_COOLDOWN_SECONDS = 300;
export const BLOCK_CAP = 2500;
const TITLE_MAX = 120;
const LINE_MAX = 240;

const scalars = (text) => Array.from(text);

export function clip(text, max) {
  const chars = scalars(text);
  if (chars.length <= max) return text;
  return chars.slice(0, max - 3).join("") + "...";
}

export function flatten(text) {
  return String(text ?? "")
    .replace(/[ \t\n\v\f\r]+/g, " ")
    .replace(/^ | $/g, "");
}

// A hub-sourced field is one line, and cannot open or close the block it sits in.
export function sanitize(text) {
  return flatten(text).replace(/<(\/?)agent-hub/gi, "<$1agent_hub");
}

export function noticeLine(item) {
  const title = clip(sanitize(item.title), TITLE_MAX);
  return `- [${sanitize(item.kind)}] ${title} (${sanitize(item.project)}, ${sanitize(item.id)})`;
}

// One line about the connection, such as a member waiting for approval.
export function noticeBlock(line) {
  return `<agent-hub>\n${sanitize(line)}\n</agent-hub>`;
}

export function wakeMode(value) {
  return value === "all" || value === "never" ? value : "direct";
}

// Whether an idle agent is woken by this batch, and what a wake would print.
export function notices({ hub_wake, now, last_wake_at, items }) {
  const lines = items.map(noticeLine);
  const mode = wakeMode(hub_wake);
  const eligible =
    mode === "all" ? items.length > 0 : mode === "direct" ? items.some((item) => item.direct) : false;
  const cooled = last_wake_at == null || now - last_wake_at >= WAKE_COOLDOWN_SECONDS;
  const wake = eligible && cooled;
  const ordered = [...items.filter((i) => i.direct), ...items.filter((i) => !i.direct)];
  return { lines, wake, watch_lines: wake ? ordered.map(noticeLine) : [] };
}

const cut = (lines, max) => lines.slice(0, max).map((line) => clip(sanitize(line), LINE_MAX));

export function contextBlock(input) {
  const project_id = sanitize(input.project_id);
  const session_name = sanitize(input.session_name);
  const session_id = input.session_id ? sanitize(input.session_id) : null;
  const member = sanitize(input.member);
  const agent = sanitize(input.agent);
  const how = input.plugin_adds_session === false ? "on every hub call" : "on hub calls (the plugin adds it)";
  const anchor = input.anchor
    ? { ...input.anchor, phase: sanitize(input.anchor.phase), age: sanitize(input.anchor.age) }
    : null;
  const contextLines = (input.context_lines ?? []).map(sanitize);
  const head = [
    `project ${project_id} | session ${session_name}${session_id ? ` (${session_id})` : ""} | you are ${member} \u00b7 ${agent}`,
    ...contextLines,
    `Pass session: {"project_id": "${project_id}", "name": "${session_name}"} ${how}.`,
    "Do not call session_brief; this block is the brief.",
  ];
  const groups = {
    brief: cut(input.brief ?? [], 12),
    anchor: anchor ? cut(anchor.lines ?? [], 10) : [],
    handoff: cut(input.handoff ?? [], Infinity),
    kb: cut(input.kb_index ?? [], 15),
    pending: cut(input.pending ?? [], Infinity),
  };

  const render = (truncated) => {
    const body = [...head, ...groups.brief];
    if (anchor) body.push(`Last anchor (phase "${anchor.phase}", ${anchor.age}):`, ...groups.anchor);
    if (groups.handoff.length) body.push("Handoff:", ...groups.handoff);
    if (groups.kb.length) body.push("KB index:", ...groups.kb);
    if (groups.pending.length) body.push("Pending:", ...groups.pending);
    if (truncated) body.push("[truncated]");
    return `<agent-hub>\n${body.join("\n")}\n</agent-hub>`;
  };

  let text = render(false);
  if (scalars(text).length <= BLOCK_CAP) return { text, truncated: false };

  const dropOrder = ["kb", "handoff", "anchor", "brief", "pending"];
  for (;;) {
    const name = dropOrder.find((key) => groups[key].length > 0);
    if (!name) break;
    groups[name].pop();
    text = render(true);
    if (scalars(text).length <= BLOCK_CAP) break;
  }
  return { text, truncated: true };
}

export function ago(fromIso, now = Date.now()) {
  const then = Date.parse(fromIso);
  if (Number.isNaN(then)) return "earlier";
  const secs = Math.max(0, Math.floor((now - then) / 1000));
  if (secs < 90) return "just now";
  const mins = Math.round(secs / 60);
  if (mins < 90) return `${mins}m ago`;
  const hours = Math.round(secs / 3600);
  if (hours < 36) return `${hours}h ago`;
  return `${Math.round(secs / 86400)}d ago`;
}
