// Per-session state under $XDG_STATE_HOME/agent-hub/hooks/opencode, shared in
// shape with the other harnesses. Never the system temporary directory.

import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export function stateDir(env = process.env, home = env.HOME || homedir()) {
  const xdg = env.XDG_STATE_HOME && env.XDG_STATE_HOME.trim() ? env.XDG_STATE_HOME : join(home, ".local", "state");
  return join(xdg, "agent-hub", "hooks", "opencode");
}

const file = (dir, id) => join(dir, `${String(id).replace(/[^A-Za-z0-9_.-]/gu, "_")}.json`);

export function readState(dir, id) {
  try {
    return JSON.parse(readFileSync(file(dir, id), "utf8"));
  } catch {
    return {};
  }
}

export function writeState(dir, id, patch) {
  const next = { ...readState(dir, id), ...patch };
  try {
    mkdirSync(dir, { recursive: true });
    const target = file(dir, id);
    writeFileSync(`${target}.tmp`, JSON.stringify(next));
    renameSync(`${target}.tmp`, target);
  } catch {
    // state is a cache of the cooldown; the plugin works without it
  }
  return next;
}
