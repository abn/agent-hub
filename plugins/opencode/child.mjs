// A child session's hub name, and the exclusive start that finds it free. The
// twin of src/client/hook/child.rs: both run the subagent-* contract fixtures.
//
// The hub is the authority on names. A new child is started exclusively under
// its parent, and a name that is already held is refused, so the next one is
// tried. A name that exists is never reused, since it may belong to another
// subagent, live or ended.

import { slug } from "./resolve.mjs";

const NAME_TAKEN = "session_name_taken";
const SHORT_ID_LEN = 8;
const TYPE_MAX = 24;
const SEGMENT_MAX = 64;

// The part of a child name that says what kind of subagent it is.
export function typeSlug(agentType) {
  const cut = Array.from(slug(agentType ?? ""))
    .slice(0, TYPE_MAX)
    .join("")
    .replace(/-+$/u, "");
  return cut || "agent";
}

// The first 8 letters and digits of the harness's id for the subagent.
export const shortId = (agentId) => String(agentId).replace(/[^A-Za-z0-9]/gu, "").slice(0, SHORT_ID_LEN);

// The names a child may try, in order: the plain one, then -2 to -9, then the
// type with the whole harness id.
export function candidates(agentType, agentId) {
  const kind = typeSlug(agentType);
  const short = shortId(agentId);
  const base = short ? `${kind}-${short}` : kind;
  const names = [base, ...Array.from({ length: 8 }, (_, i) => `${base}-${i + 2}`)];
  const whole = Array.from(slug(String(agentId)))
    .slice(0, Math.max(0, SEGMENT_MAX - kind.length - 1))
    .join("")
    .replace(/-+$/u, "");
  if (whole) names.push(`${kind}-${whole}`);
  return names;
}

const childOf = (name, started) => ({ name, sessionId: started?.session_id ?? null });

// `start(sessionName, exclusive)` is `session_start`; a refusal it throws
// carries the hub's `reason`. Returns the child, or throws what the hub refused
// with.
export async function startNew({ start, parent, agentType, agentId }) {
  for (const candidate of candidates(agentType, agentId)) {
    const name = `${parent}/${candidate}`;
    try {
      return childOf(name, await start(name, true));
    } catch (error) {
      if (error?.reason !== NAME_TAKEN) throw error;
    }
  }
  throw new Error("every child session name is taken");
}

// A child the plugin has started before starts again under its own name.
export async function resume({ start, name }) {
  return childOf(name, await start(name, false));
}
