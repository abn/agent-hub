// Project and session resolution of the hook contract. resolveContext is pure:
// git facts, files and the environment arrive as data. gatherGit and
// readTopLevelFile collect them from the machine.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { clip, flatten } from "./format.mjs";

const ID = /^[A-Za-z0-9_-]{1,64}$/u;
const NAME_MAX = 64;
const PERSONAL_LINE = "no project matched; write `.agent-hub` to pin one";

const chars = (text) => Array.from(text);

// The first 8 characters of the harness id that a session name may hold. An id
// with fewer than 8 such characters is named by the start of its SHA-256.
export function shortSessionId(harnessSessionId) {
  const valid = String(harnessSessionId).replace(/[^A-Za-z0-9_-]/gu, "");
  if (valid.length >= 8) return valid.slice(0, 8);
  return createHash("sha256").update(String(harnessSessionId), "utf8").digest("hex").slice(0, 8);
}

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

function rejection(source, value) {
  return `ignored ${source} "${clip(flatten(value), NAME_MAX)}": not [A-Za-z0-9_-]{1,64}`;
}

export function resolveContext(input) {
  const { harness, harness_session_id, env, git, files, known_projects, personal_project } = input;
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
  if (!project && top) {
    const text = files?.[`${top}/.agent-hub`];
    if (typeof text === "string") {
      const first = text.split(/\r?\n/u, 1)[0].trim();
      if (first) {
        const id = explicit(".agent-hub", first);
        if (id) project = { project_id: id, project_source: "file" };
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
  if (!session) {
    session = {
      session_name: `${harness}-${shortSessionId(harness_session_id)}`,
      session_source: "harness_session",
    };
  }
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
  if (!top) return { is_repo: false, top_level: null, origin_url: null, branch: null, detached: false };
  const branch = git(cwd, ["symbolic-ref", "--short", "-q", "HEAD"]);
  return {
    is_repo: true,
    top_level: top,
    origin_url: git(cwd, ["remote", "get-url", "origin"]) || null,
    branch: branch || null,
    detached: !branch,
  };
}

export function readTopLevelFile(top) {
  if (!top) return {};
  try {
    return { [`${top}/.agent-hub`]: readFileSync(`${top}/.agent-hub`, "utf8") };
  } catch {
    return {};
  }
}
