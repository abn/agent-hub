// The session-start bundle: resolve, brief, bind, read the index, subscribe and
// render the <agent-hub> block.

import { ago, contextBlock, resourceUrl } from "./format.mjs";
import { gatherGit, readTopLevelFile, resolveContext } from "./resolve.mjs";

const splitLines = (text) =>
  String(text ?? "")
    .split(/\r?\n/u)
    .map((line) => line.trimEnd())
    .filter((line) => line.length > 0);

const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

const total = (section) => (section?.items?.length ?? 0) + (section?.more ?? 0);

export function briefLines(brief, hubUrl) {
  if (!brief) return [];
  const link = (item) => (hubUrl && item.id ? `${resourceUrl(hubUrl, item.id)} ` : "");
  const answers = total(brief.answers);
  const events = total(brief.events);
  const stale = total(brief.stale_pages);
  const parts = [];
  if (answers) parts.push(plural(answers, "answer"));
  if (events) parts.push(plural(events, "event"));
  if (stale) parts.push(plural(stale, "stale page"));
  const lines = [`Since your last look: ${parts.length ? parts.join(", ") : "nothing new"}.`];
  for (const item of (brief.answers?.items ?? []).slice(0, 3)) {
    lines.push(`- ${item.kind} ${item.outcome}: ${link(item)}${item.summary ?? item.id}`);
  }
  for (const item of (brief.events?.items ?? []).slice(0, 6)) {
    lines.push(`- [${item.kind}]${item.open ? " open" : ""} ${link(item)}${item.summary ?? item.id}`);
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

const NOT_A_MEMBER = "Not a member of this hub yet: call enrol.";
const AGENT = "opencode";

// Today, in UTC, as YYYY-MM-DD.
const today = (now) => new Date(now).toISOString().slice(0, 10);

// Returns everything the loop and the hooks need to follow this session, or the
// one line that says why the hub cannot give one. The block is the stable
// bundle. Items the hub reports as pending along the way, in any tool result,
// are pushed onto `trailers`, which the caller owns so they survive a start
// that fails part way. `notices` are the resolver's lines for the block.
export async function startSession({ hub, cwd, env = process.env, now = Date.now(), trailers = [], notices = [], signal }) {
  const call = (name, args) => hub.callTool(name, args, { trailers, signal });
  const git = gatherGit(cwd);
  const who = await call("whoami");
  if (who.state === "pending") return { notice: `Waiting for approval as ${who.member ?? ""}.` };
  if (who.state === "anonymous") return { notice: NOT_A_MEMBER };
  const member = who.member ?? who.actor ?? "local";
  const agent = who.agent ?? AGENT;
  const known = await quiet(() => hub.projects({ signal }), []);
  const resolved = resolveContext({
    agent,
    date: today(now),
    cwd,
    env,
    git,
    files: readTopLevelFile(git.top_level),
    known_projects: known,
    personal_project: who.personal_project ?? member,
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
    member,
    agent,
    context_lines: [...notices, ...(resolved.context_line ? resolved.context_line.split("\n") : [])],
    brief: briefLines(brief, hub.url),
    hub_url: hub.url,
    anchor: anchorView(started.anchor, now),
    handoff: splitLines(started.handoff),
    kb_index: splitLines(index?.content),
  });
  return { block: text, projectId, sessionName, hubSessionId, member, agent };
}

// What a subagent is told about its own hub session.
export function subagentText(projectId, sessionName) {
  const ref = JSON.stringify({ project_id: projectId, name: sessionName }).replace(/":/gu, '": ').replace(/,"/gu, ', "');
  return `You work in hub session ${sessionName} (project ${projectId}). Hub calls carry it; pass session: ${ref} if you call a hub tool directly.`;
}

export function keepInstruction({ projectId, sessionName, hubSessionId }) {
  return `Keep: hub project ${projectId}, session ${sessionName}${hubSessionId ? ` (id ${hubSessionId})` : ""}, the current phase and open hub items.`;
}
