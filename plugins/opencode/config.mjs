// Connection settings for the plugin: HUB_* environment, then the client table
// of config.toml, then plugin options. The url and the token come from the
// same layer, the first one that has a url.

import { readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { join, sep } from "node:path";

export const DEFAULT_AGENT_NAME = "opencode";

const present = (value) => (typeof value === "string" && value.trim() !== "" ? value.trim() : null);

function unquote(raw) {
  const value = raw.trim();
  if (value.startsWith('"')) {
    let out = "";
    for (let i = 1; i < value.length; i += 1) {
      const c = value[i];
      if (c === "\\" && i + 1 < value.length) {
        const next = value[(i += 1)];
        out += { n: "\n", t: "\t", r: "\r", '"': '"', "\\": "\\" }[next] ?? next;
      } else if (c === '"') {
        return out;
      } else {
        out += c;
      }
    }
    return out;
  }
  if (value.startsWith("'")) {
    const end = value.indexOf("'", 1);
    return end === -1 ? value.slice(1) : value.slice(1, end);
  }
  return value.replace(/\s+#.*$/u, "");
}

// Reads the `url` and `token` keys of the [client] table, nothing else.
export function parseClientToml(text) {
  const out = {};
  let table = "";
  for (const raw of text.split(/\r?\n/u)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const header = /^\[([^\]]+)\]/u.exec(line);
    if (header) {
      table = header[1].trim();
      continue;
    }
    if (table !== "client") continue;
    const eq = line.indexOf("=");
    if (eq === -1) continue;
    const key = line.slice(0, eq).trim();
    if (key === "url" || key === "token") out[key] = unquote(line.slice(eq + 1));
  }
  return out;
}

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

// The directory a token file must live in: a project config can name any path,
// so only the user's own agent-hub config directory is trusted.
export function tokenDir(env, home = env.HOME || homedir()) {
  const xdg = present(env.XDG_CONFIG_HOME);
  return join(xdg ?? join(home, ".config"), "agent-hub");
}

function trustedTokenFile(path, { env, home, realpath, warn }) {
  try {
    const dir = realpath(tokenDir(env, home));
    const real = realpath(path);
    if (real.startsWith(dir + sep)) return real;
  } catch {
    // an unreadable or missing path is refused below
  }
  warn(`agent-hub: ignoring tokenFile outside ${tokenDir(env, home)}; no token is sent`);
  return null;
}

export function resolveConfig({
  options = {},
  env = process.env,
  readFile = (p) => readFileSync(p, "utf8"),
  realpath = realpathSync,
  warn = (line) => console.warn(line),
  home,
} = {}) {
  // The first file with a url wins, with its own token and nothing from the others.
  let fromFile = {};
  for (const path of configPaths(env, home)) {
    const text = tryRead(path, readFile);
    if (text === null) continue;
    const parsed = parseClientToml(text);
    if (present(parsed.url)) {
      fromFile = parsed;
      break;
    }
  }

  // A token goes only to the url from its own layer. The user-level layers, the
  // environment and config.toml, come first: a project-scoped plugin option
  // applies only when neither has a url, so a project config cannot redirect a
  // user who has configured a hub, nor collect that user's token.
  const optionsUrl = present(options.url);
  const layers = [
    { url: env.HUB_URL, token: () => env.HUB_TOKEN },
    { url: fromFile.url, token: () => fromFile.token },
    {
      url: options.url,
      token: () => {
        const requested = present(options.tokenFile);
        const trusted = requested ? trustedTokenFile(requested, { env, home, realpath, warn }) : null;
        return trusted ? tryRead(trusted, readFile) : null;
      },
    },
  ];
  const layer = layers.find((candidate) => present(candidate.url));
  const url = layer ? present(layer.url) : null;
  if (optionsUrl && layer !== layers[2] && url.replace(/\/+$/u, "") !== optionsUrl.replace(/\/+$/u, "")) {
    warn(`agent-hub: ignoring the plugin option url ${optionsUrl}; ${layer === layers[0] ? "HUB_URL" : "config.toml"} sets the hub`);
  }
  const token = layer ? present(layer.token()) : null;
  return {
    url: url ? url.replace(/\/+$/u, "") : null,
    token,
    agentName: present(options.agentName) ?? DEFAULT_AGENT_NAME,
  };
}
