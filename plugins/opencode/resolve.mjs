// Project and session resolution of the hook contract. resolveContext is pure:
// git facts, files and the environment arrive as data. gatherGit and
// readTopLevelFile collect them from the machine.

import { execFileSync } from "node:child_process";
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs";
import { clip, flatten } from "./format.mjs";

const ID = /^[A-Za-z0-9_-]{1,64}$/u;
const NAME_MAX = 64;
const PERSONAL_LINE = "no project matched; write `.agent-hub.toml` to pin one";
const PROJECT_FILE = ".agent-hub.toml";
const LEGACY_FILE = ".agent-hub";

const chars = (text) => Array.from(text);

export function slug(text) {
  const lowered = String(text).replace(/[A-Z]/g, (c) => c.toLowerCase());
  return lowered.replace(/[^a-z0-9_]+/gu, "-").replace(/^-+|-+$/g, "");
}

// An inferred name: slugged, cut to 64, with a trailing "-" from the cut removed.
function inferred(text) {
  const slugged = slug(text);
  if (!slugged) return null;
  return chars(slugged).slice(0, NAME_MAX).join("").replace(/-+$/g, "") || null;
}

export function repoName(originUrl) {
  if (!originUrl) return null;
  let url = originUrl.trim().replace(/\/+$/g, "");
  url = url.replace(/\.git$/u, "").replace(/\/+$/g, "");
  const last = url.split(/[/:]/u).pop();
  return last || null;
}

const set = (value) => (value === undefined || value === null || value === "" ? null : value);

// The `project` key of a .agent-hub.toml: a quoted string before the first
// table. Anything else reads as no pin.
export function tomlProject(text) {
  for (const raw of String(text).split(/\r?\n/u)) {
    const line = raw.trim();
    if (line.startsWith("[")) return null;
    if (!line.startsWith("project")) continue;
    const rest = line.slice("project".length).trimStart();
    if (!rest.startsWith("=")) continue;
    const value = rest.slice(1).trimStart();
    const quote = value[0];
    if (quote !== '"' && quote !== "'") return null;
    let out = "";
    let escaped = false;
    for (const c of value.slice(1)) {
      if (quote === '"' && escaped) {
        out += c;
        escaped = false;
      } else if (quote === '"' && c === "\\") escaped = true;
      else if (c === quote) return out;
      else out += c;
    }
    return null;
  }
  return null;
}

// The agent as a session name holds it, as the hub's own suggestion makes it.
const datedAgent = (agent) => String(agent).replace(/[^A-Za-z0-9_-]/gu, "-");

function rejection(source, value) {
  return `ignored ${source} "${clip(flatten(value), NAME_MAX)}": not [A-Za-z0-9_-]{1,64}`;
}

export function resolveContext(input) {
  const { agent, date, env, git, files, known_projects, personal_project } = input;
  const known = new Set(known_projects ?? []);
  const projectRejections = [];
  const sessionRejections = [];
  let project = null;

  const explicit = (source, value) => {
    if (ID.test(value)) return value;
    projectRejections.push(rejection(source, value));
    return null;
  };

  const fromEnv = set(env?.HUB_PROJECT);
  if (fromEnv !== null) {
    const id = explicit("HUB_PROJECT", fromEnv);
    if (id) project = { project_id: id, project_source: "env" };
  }

  const top = git?.is_repo ? git.top_level : null;
  if (top) {
    const text = files?.[`${top}/${PROJECT_FILE}`];
    if (!project && typeof text === "string") {
      const pinned = (tomlProject(text) ?? "").trim();
      if (pinned) {
        const id = explicit(PROJECT_FILE, pinned);
        if (id) project = { project_id: id, project_source: "file" };
      }
    }
    const legacy = files?.[`${top}/${LEGACY_FILE}`];
    if (typeof legacy === "string") {
      const first = legacy.split("\n", 1)[0].replace(/\r$/u, "").trim();
      if (first) {
        projectRejections.push(
          `found ${LEGACY_FILE}; this release reads ${PROJECT_FILE}: project = "${clip(flatten(first), NAME_MAX)}"`,
        );
      }
    }
  }

  if (!project && top) {
    const origin = inferred(repoName(git.origin_url) ?? "");
    if (origin && known.has(origin)) project = { project_id: origin, project_source: "origin" };
  }
  if (!project && top) {
    const base = inferred(top.split("/").filter(Boolean).pop() ?? "");
    if (base && known.has(base)) project = { project_id: base, project_source: "dir" };
  }

  const lines = [...projectRejections];
  if (!project) {
    project = { project_id: personal_project, project_source: "personal" };
    lines.push(PERSONAL_LINE);
  }

  let session = null;
  const fromSession = set(env?.HUB_SESSION);
  if (fromSession !== null) {
    if (ID.test(fromSession)) session = { session_name: fromSession, session_source: "env" };
    else sessionRejections.push(rejection("HUB_SESSION", fromSession));
  }
  if (!session && git?.is_repo && !git.detached && git.branch && !["main", "master"].includes(git.branch)) {
    const name = inferred(git.branch);
    if (name) session = { session_name: name, session_source: "branch" };
  }
  if (!session && git?.linked_worktree && top) {
    const name = inferred(top.split("/").filter(Boolean).pop() ?? "");
    if (name) session = { session_name: name, session_source: "worktree" };
  }
  if (!session) session = { session_name: `${datedAgent(agent)}-${date}`, session_source: "date" };
  lines.push(...sessionRejections);

  return { ...project, ...session, context_line: lines.length ? lines.join("\n") : null };
}

function git(cwd, args) {
  try {
    return execFileSync("git", ["-C", cwd, ...args], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 5000,
    }).trim();
  } catch {
    return null;
  }
}

export function gatherGit(cwd) {
  const top = git(cwd, ["rev-parse", "--show-toplevel"]);
  if (!top) return { is_repo: false, top_level: null, origin_url: null, branch: null, detached: false, linked_worktree: false };
  const branch = git(cwd, ["symbolic-ref", "--short", "-q", "HEAD"]);
  const dir = git(cwd, ["rev-parse", "--path-format=absolute", "--git-dir"]);
  const common = git(cwd, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
  return {
    is_repo: true,
    top_level: top,
    origin_url: git(cwd, ["remote", "get-url", "origin"]) || null,
    branch: branch || null,
    detached: !branch,
    linked_worktree: Boolean(dir && common && dir !== common),
  };
}

const PIN_FILE_MAX = 4096;

// .agent-hub.toml and a legacy .agent-hub at the top level, keyed by path.
export function readTopLevelFile(top) {
  const files = {};
  if (!top) return files;
  for (const name of [PROJECT_FILE, LEGACY_FILE]) {
    try {
      // Non-blocking, so a FIFO in its place is not waited on.
      const fd = openSync(`${top}/${name}`, constants.O_RDONLY | constants.O_NONBLOCK);
      try {
        if (!fstatSync(fd).isFile()) continue;
        const buffer = Buffer.alloc(PIN_FILE_MAX);
        const n = readSync(fd, buffer, 0, PIN_FILE_MAX, 0);
        files[`${top}/${name}`] = buffer.subarray(0, n).toString("utf8");
      } finally {
        closeSync(fd);
      }
    } catch {
      // absent or unreadable: no pin from it
    }
  }
  return files;
}
