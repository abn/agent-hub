// Which hub, member and token the plugin uses. The choice is the credential
// resolver's (credential.mjs, the twin of src/client/credential.rs), over the
// HUB_* environment, the project's .agent-hub.toml and the config.toml files.
// This module only gathers what the resolver reads.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { parseConfig, resolve, scanConfig } from "./credential.mjs";

const present = (value) => (typeof value === "string" && value.trim() !== "" ? value.trim() : null);

export function configPaths(env, home = env.HOME || homedir()) {
  if (present(env.HUB_CONFIG)) return [present(env.HUB_CONFIG)];
  const xdg = present(env.XDG_CONFIG_HOME);
  const user = [xdg ? join(xdg, "agent-hub", "config.toml") : join(home, ".config", "agent-hub", "config.toml")];
  user.push(join(home, ".agent-hub", "config.toml"));
  const system = present(env.AGENT_HUB_SYSTEM_CONFIG) ?? "/etc/agent-hub/config.toml";
  return [...user, system];
}

function tryRead(path, readFile) {
  try {
    return readFile(path);
  } catch {
    return null;
  }
}

// The file credentials are written to: HUB_CONFIG, else the user file already
// in use, else the first place a user file would go.
export function credentialPath(env, { home, readFile = (p) => readFileSync(p, "utf8") } = {}) {
  const paths = configPaths(env, home);
  if (present(env.HUB_CONFIG)) return paths[0];
  const user = paths.slice(0, -1);
  return user.find((path) => tryRead(path, readFile) !== null) ?? user[0];
}

// The parts of the config files the resolver reads: the user's file (the first
// that exists) and then the system's, merged as the Rust client merges them.
export function readConfigView(env, { home, readFile = (p) => readFileSync(p, "utf8") } = {}) {
  const paths = configPaths(env, home);
  const chosen = present(env.HUB_CONFIG) ? paths : [...paths.slice(0, -1).filter((p) => tryRead(p, readFile) !== null).slice(0, 1), paths.at(-1)];
  const view = { client: {}, credentials: [], trust: { pairings: [] } };
  for (const path of chosen) {
    const text = tryRead(path, readFile);
    if (text === null) continue;
    let parsed;
    try {
      parsed = parseConfig(text);
    } catch {
      continue;
    }
    view.client.url ??= present(parsed.client.url) ? parsed.client.url : undefined;
    view.client.member ??= present(parsed.client.member) ? parsed.client.member : undefined;
    view.credentials.push(...parsed.credentials);
    view.trust.pairings.push(...parsed.trust.pairings);
  }
  return view;
}

const FILE_MAX = 4096;

// `.agent-hub.toml` at the git top level: its project, url and member. A token
// in it is refused, since the file is shared.
export function readProjectFile(topLevel, readFile = (p) => readFileSync(p, "utf8")) {
  if (!topLevel) return { file: null, notices: [] };
  const notices = [];
  const legacy = tryRead(`${topLevel}/.agent-hub`, readFile);
  const first = legacy?.slice(0, FILE_MAX).split("\n")[0].trim();
  if (first) {
    notices.push(`found .agent-hub; this release reads .agent-hub.toml: project = "${first}"`);
  }
  const text = tryRead(`${topLevel}/.agent-hub.toml`, readFile);
  if (text === null) return { file: null, notices };
  const file = {};
  for (const item of scanConfig(text.slice(0, FILE_MAX))) {
    if (item.kind === "table") break;
    if (item.key === "token") {
      return { file: null, notices: [...notices, ".agent-hub.toml must not hold a token; it is shared"] };
    }
    if (["project", "url", "member"].includes(item.key) && typeof item.value === "string") file[item.key] = item.value;
  }
  return { file, notices };
}

// The url, member and token for this plugin and why. `options.url` names a hub
// only when nothing else does, so a project's OpenCode config cannot redirect
// a user who has set one up. `topLevel` is the git top level of the project.
export function resolveConfig({
  options = {},
  env = process.env,
  readFile = (p) => readFileSync(p, "utf8"),
  warn = (line) => console.warn(line),
  home,
  topLevel = null,
} = {}) {
  const config = readConfigView(env, { home, readFile });
  const { file, notices } = readProjectFile(topLevel, readFile);
  const input = {
    env: {
      HUB_URL: present(env.HUB_URL),
      HUB_TOKEN: present(env.HUB_TOKEN),
      HUB_MEMBER: present(env.HUB_MEMBER),
      HUB_CONFIG: present(env.HUB_CONFIG),
    },
    pinned_url: null,
    project_file: file,
    project_root: topLevel,
    config,
  };
  let resolved = resolve(input);
  const optionUrl = present(options.url);
  if (optionUrl && resolved.state === "no_hub") {
    resolved = resolve({ ...input, pinned_url: optionUrl });
  } else if (optionUrl && resolved.url && resolved.url !== resolve({ ...input, pinned_url: optionUrl }).url) {
    warn(`agent-hub: ignoring the plugin option url ${optionUrl}; the environment, .agent-hub.toml or config.toml sets the hub`);
  }
  return {
    url: resolved.url ? resolved.url.replace(/\/+$/u, "") : null,
    member: resolved.member,
    token: resolved.token,
    state: resolved.state,
    source: resolved.source,
    notices: [...notices, ...(resolved.notices ?? [])],
    configPath: credentialPath(env, { home, readFile }),
  };
}

// The line the session block carries for a resolver state that rules out
// connecting, or null when the plugin may connect.
export function refusal(config) {
  if (config.state === "several_members") {
    return `Several members for ${config.url}; set member in .agent-hub.toml or HUB_MEMBER.`;
  }
  if (config.state === "needs_trust") {
    return `This project asks to use member ${config.member ?? "(none)"} on ${config.url}. Allow it with agent-hub trust or under [trust] in config.toml.`;
  }
  return null;
}
