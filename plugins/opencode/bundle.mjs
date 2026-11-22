// The session-start bundle: resolve, brief, bind, read the index, subscribe and
// render the <agent-hub> block.

import { ago, contextBlock } from "./format.mjs";
import { gatherGit, readTopLevelFile, resolveContext } from "./resolve.mjs";

const splitLines = (text) =>
  String(text ?? "")
    .split(/\r?\n/u)
    .map((line) => line.trimEnd())
    .filter((line) => line.length > 0);

const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

const total = (section) => (section?.items?.length ?? 0) + (section?.more ?? 0);

export function briefLines(brief) {
  if (!brief) return [];
  const answers = total(brief.answers);
  const events = total(brief.events);
  const stale = total(brief.stale_pages);
  const parts = [];
  if (answers) parts.push(plural(answers, "answer"));
  if (events) parts.push(plural(events, "event"));
  if (stale) parts.push(plural(stale, "stale page"));
  const lines = [`Since your last look: ${parts.length ? parts.join(", ") : "nothing new"}.`];
  for (const item of (brief.answers?.items ?? []).slice(0, 3)) {
    lines.push(`- ${item.kind} ${item.outcome}: ${item.summary ?? item.id}`);
  }
  for (const item of (brief.events?.items ?? []).slice(0, 6)) {
    lines.push(`- [${item.kind}]${item.open ? " open" : ""} ${item.summary ?? item.id}`);
  }
  return lines;
}

function anchorView(anchor, now) {
  if (!anchor) return null;
  const lines = [...splitLines(anchor.summary)];
  const next = splitLines(Array.isArray(anchor.next_steps) ? anchor.next_steps.join("\n") : anchor.next_steps);
  if (next.length) lines.push("Next:", ...next);
  return { phase: anchor.phase ?? "", age: ago(anchor.created_at, now), lines };
}

const quiet = async (work, fallback) => {
  try {
    return await work();
  } catch {
    return fallback;
  }
};

// Returns everything the loop and the hooks need to follow this session. The
// block is the stable bundle. Items the hub reports as pending along the way,
// in any tool result, are pushed onto `trailers`, which the caller owns so they
// survive a start that fails part way.
export async function startSession({ hub, cwd, harnessSessionId, env = process.env, now = Date.now(), trailers = [], signal }) {
  const call = (name, args) => hub.callTool(name, args, { trailers, signal });
  const git = gatherGit(cwd);
  const who = await quiet(() => call("whoami"), {});
  const known = await quiet(() => hub.projects({ signal }), []);
  const resolved = resolveContext({
    harness: "opencode",
    harness_session_id: harnessSessionId,
    cwd,
    env,
    git,
    files: readTopLevelFile(git.top_level),
    known_projects: known,
    personal_project: who.personal_project ?? who.actor ?? "opencode",
  });
  const { project_id: projectId, session_name: sessionName } = resolved;

  const brief = await quiet(() => call("session_brief", { project_id: projectId }), null);
  const started = await call("session_start", { project_id: projectId, session_name: sessionName });
  const hubSessionId = started.session_id ?? null;
  const ref = hubSessionId ? { session: { session_id: hubSessionId } } : {};

  const index = await quiet(() => call("brain_get", { path: "/fs/index.md", store: "project", project_id: projectId }), null);
  for (const kind of ["comment", "page"]) {
    await quiet(() => call("notify_subscribe", { kinds: kind, project_id: projectId, about_self: true, ...ref }), null);
  }
  if (hubSessionId) await quiet(() => hub.drain(hubSessionId, { deadline: "startup", trailers, signal }), null);

  const { text } = contextBlock({
    project_id: projectId,
    session_name: sessionName,
    session_id: hubSessionId,
    actor: who.actor ?? "opencode",
    context_lines: resolved.context_line ? resolved.context_line.split("\n") : [],
    brief: briefLines(brief),
    anchor: anchorView(started.anchor, now),
    handoff: splitLines(started.handoff),
    kb_index: splitLines(index?.content),
  });
  return { block: text, projectId, sessionName, hubSessionId, actor: who.actor ?? "opencode" };
}

export function subagentText(hubSessionId) {
  return hubSessionId
    ? `You share the parent's hub connection and session ${hubSessionId}. Do not call session_start. Write with \`session: {session_id: "${hubSessionId}"}\`.`
    : "You share the parent's hub connection and session. Do not call session_start.";
}

export function keepInstruction({ projectId, sessionName, hubSessionId }) {
  return `Keep: hub project ${projectId}, session ${sessionName}${hubSessionId ? ` (id ${hubSessionId})` : ""}, the current phase and open hub items.`;
}
