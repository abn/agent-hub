// The credential resolver and the config.toml credential store, the JS twin of
// the Rust resolver. `resolve` is pure over data; the store functions take the
// config.toml lock-dir lock, re-read, modify and rename into place.

import {
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  rmdirSync,
  statSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import { randomBytes } from "node:crypto";
import { hostname } from "node:os";
import { dirname } from "node:path";

const present = (value) => (typeof value === "string" && value.trim() !== "" ? value.trim() : null);

// Scheme and host lowercased, the default port and trailing slashes dropped.
export function normalizeUrl(raw) {
  const text = present(raw);
  if (!text) return null;
  try {
    const url = new URL(text);
    const path = url.pathname.replace(/\/+$/u, "");
    return `${url.protocol}//${url.host}${path}${url.search}`;
  } catch {
    return text.replace(/\/+$/u, "");
  }
}

const same = (a, b) => normalizeUrl(a) === normalizeUrl(b);

// The distinct urls of the credentials, as first written.
function distinctUrls(credentials) {
  const seen = new Map();
  for (const c of credentials) {
    const key = normalizeUrl(c.url);
    if (key && !seen.has(key)) seen.set(key, c.url);
  }
  return [...seen.values()];
}

// The url, member and token of this client, and why. See the credential
// fixtures for the cases; `state` is what an entry point acts on.
export function resolve(input) {
  const env = input.env ?? {};
  const project = input.project_file ?? null;
  const config = input.config ?? {};
  const client = config.client ?? {};
  const credentials = config.credentials ?? [];
  const pairings = config.trust?.pairings ?? [];
  const notices = [];

  const pinned = present(input.pinned_url);
  const envUrl = present(env.HUB_URL);
  const projectUrl = present(project?.url);
  const clientUrl = present(client.url);
  const urls = distinctUrls(credentials);

  let url = null;
  let urlSource = "none";
  if (pinned) [url, urlSource] = [pinned, "pinned"];
  else if (envUrl) [url, urlSource] = [envUrl, "env"];
  else if (projectUrl) [url, urlSource] = [projectUrl, "project"];
  else if (clientUrl) [url, urlSource] = [clientUrl, "client"];
  else if (urls.length === 1) [url, urlSource] = [urls[0], "only"];

  if (pinned && projectUrl && !same(projectUrl, pinned)) {
    notices.push(`.agent-hub.toml names ${projectUrl}; the plugin is set to ${pinned}`);
  }

  const out = (fields) => ({
    url: url ? normalizeUrl(url) : null,
    member: null,
    token: null,
    source: { url: urlSource, member: "none", token: "none" },
    state: "ok",
    notices,
    ...fields,
  });

  if (!url) return out({ state: "no_hub" });

  const forUrl = credentials.filter((c) => same(c.url, url));
  const named = forUrl.filter((c) => present(c.member));
  const namedMembers = [...new Set(named.map((c) => present(c.member)))];

  let member = null;
  let memberSource = "none";
  let several = false;
  if (present(env.HUB_MEMBER)) [member, memberSource] = [present(env.HUB_MEMBER), "env"];
  else if (present(project?.member) && (!projectUrl || same(projectUrl, url))) {
    [member, memberSource] = [present(project.member), "project"];
  } else if (present(client.member) && (!clientUrl || same(clientUrl, url))) {
    [member, memberSource] = [present(client.member), "client"];
  } else if (namedMembers.length === 1) [member, memberSource] = [namedMembers[0], "only"];
  else if (namedMembers.length > 1) several = true;

  let token = null;
  let tokenSource = "none";
  const envToken = present(env.HUB_TOKEN);
  if (envToken && same(envUrl, url) && (!present(env.HUB_MEMBER) || env.HUB_MEMBER.trim() === member)) {
    [token, tokenSource] = [envToken, "env"];
  } else if (member) {
    const hit = forUrl.find((c) => present(c.member) === member && present(c.token));
    if (hit) [token, tokenSource] = [hit.token, "credential"];
  } else if (!several) {
    const legacy = forUrl.find((c) => !present(c.member) && present(c.token));
    if (legacy) [token, tokenSource] = [legacy.token, "legacy"];
  }

  const source = { url: urlSource, member: memberSource, token: tokenSource };

  const supplied = urlSource === "project" || memberSource === "project";
  if (supplied) {
    const trusted = pairings.some(
      (p) => p.path === input.project_root && same(p.url, url) && (present(p.member) ?? null) === member,
    );
    if (!trusted) {
      return out({ member, token: null, source: { ...source, token: "none" }, state: "needs_trust" });
    }
  }

  if (several && !member) return out({ member: null, token: null, state: "several_members", source });
  return out({ member, token, source, state: token ? "ok" : "tokenless" });
}

// config.toml: the client table, the [[credential]] entries and [trust]. A
// small scanner over the text, not a TOML library: it knows strings (basic,
// literal, multiline), comments, arrays and inline tables, and it records the
// span of every value so the store can replace one in place.

const ESCAPES = { b: "\b", t: "\t", n: "\n", f: "\f", r: "\r", '"': '"', "\\": "\\" };

function unescape(body, multiline) {
  let out = "";
  for (let i = 0; i < body.length; i += 1) {
    const c = body[i];
    if (c !== "\\" || i + 1 >= body.length) {
      out += c;
      continue;
    }
    const next = body[(i += 1)];
    if (next === "u" || next === "U") {
      const len = next === "u" ? 4 : 8;
      const hex = body.slice(i + 1, i + 1 + len);
      if (hex.length === len && /^[0-9a-fA-F]+$/u.test(hex)) {
        try {
          out += String.fromCodePoint(parseInt(hex, 16));
          i += len;
          continue;
        } catch {
          // not a code point: keep the letter
        }
      }
      out += next;
      continue;
    }
    if (multiline && /[ \t\r\n]/u.test(next)) {
      let j = i;
      while (j < body.length && /[ \t]/u.test(body[j])) j += 1;
      if (body[j] === "\r" || body[j] === "\n") {
        while (j < body.length && /\s/u.test(body[j])) j += 1;
        i = j - 1;
        continue;
      }
    }
    out += ESCAPES[next] ?? next;
  }
  return out;
}

const lineEndAt = (text, from) => {
  const nl = text.indexOf("\n", from);
  if (nl === -1) return text.length;
  return nl > 0 && text[nl - 1] === "\r" && nl - 1 >= from ? nl - 1 : nl;
};

// Skips blanks, and with `lines` also line breaks and comments.
function skipFiller(text, from, lines) {
  let i = from;
  while (i < text.length) {
    const c = text[i];
    if (c === " " || c === "\t") i += 1;
    else if (lines && (c === "\n" || c === "\r")) i += 1;
    else if (lines && c === "#") i = lineEndAt(text, i);
    else break;
  }
  return i;
}

// Reads one value at `from`. Strings come back unescaped, arrays as arrays,
// inline tables as objects and anything else as its trimmed text. `end` is the
// offset just past the value, never past a trailing comment.
function parseValue(text, from, nested = false) {
  const n = text.length;
  const at = (s) => text.startsWith(s, from);
  if (at('"""')) {
    let pos = from + 3;
    while (pos < n && !text.startsWith('"""', pos)) pos += text[pos] === "\\" ? 2 : 1;
    if (pos >= n) return { value: unescape(text.slice(from + 3).replace(/^\r?\n/u, ""), true), end: n };
    let extra = 0;
    while (extra < 2 && text[pos + 3 + extra] === '"') extra += 1;
    const body = text.slice(from + 3, pos + extra).replace(/^\r?\n/u, "");
    return { value: unescape(body, true), end: pos + 3 + extra };
  }
  if (at("'''")) {
    const close = text.indexOf("'''", from + 3);
    if (close === -1) return { value: text.slice(from + 3).replace(/^\r?\n/u, ""), end: n };
    let extra = 0;
    while (extra < 2 && text[close + 3 + extra] === "'") extra += 1;
    const body = text.slice(from + 3, close + extra).replace(/^\r?\n/u, "");
    return { value: body, end: close + 3 + extra };
  }
  if (at('"')) {
    let pos = from + 1;
    while (pos < n && text[pos] !== '"' && text[pos] !== "\n") pos += text[pos] === "\\" ? 2 : 1;
    const stop = Math.min(pos, n);
    const closed = text[stop] === '"';
    const body = text.slice(from + 1, stop).replace(/\r$/u, "");
    return { value: unescape(body, false), end: closed ? stop + 1 : lineEndAt(text, from) };
  }
  if (at("'")) {
    const eol = lineEndAt(text, from);
    const close = text.indexOf("'", from + 1);
    if (close === -1 || close > eol) return { value: text.slice(from + 1, eol), end: eol };
    return { value: text.slice(from + 1, close), end: close + 1 };
  }
  if (at("[")) {
    const items = [];
    let i = from + 1;
    while (i < n) {
      i = skipFiller(text, i, true);
      if (i >= n) break;
      if (text[i] === "]") {
        i += 1;
        break;
      }
      if (text[i] === ",") {
        i += 1;
        continue;
      }
      const r = parseValue(text, i, true);
      items.push(r.value);
      i = Math.max(r.end, i + 1);
    }
    return { value: items, end: i };
  }
  if (at("{")) {
    const table = {};
    let i = from + 1;
    while (i < n) {
      i = skipFiller(text, i, true);
      if (i >= n) break;
      if (text[i] === "}") {
        i += 1;
        break;
      }
      if (text[i] === ",") {
        i += 1;
        continue;
      }
      let key;
      if (text[i] === '"' || text[i] === "'") {
        const k = parseValue(text, i, true);
        [key, i] = [k.value, k.end];
      } else {
        const start = i;
        while (i < n && !",=}\n".includes(text[i])) i += 1;
        key = text.slice(start, i).trim();
      }
      i = skipFiller(text, i, false);
      if (text[i] !== "=") {
        i += 1;
        continue;
      }
      i = skipFiller(text, i + 1, true);
      const r = parseValue(text, i, true);
      table[key] = r.value;
      i = Math.max(r.end, i + 1);
    }
    return { value: table, end: i };
  }
  let end = from;
  const stops = nested ? ",]}#\n\r \t" : "#\n\r";
  while (end < n && !stops.includes(text[end])) end += 1;
  const raw = text.slice(from, end);
  const trimmed = raw.trimEnd();
  return { value: trimmed.trim(), end: from + trimmed.length };
}

// Every table header and key/value of the text, in order, with the offsets the
// store edits by. A commented-out line is never an item.
export function scanConfig(text) {
  const items = [];
  const n = text.length;
  let i = 0;
  while (i < n) {
    const start = i;
    let j = i;
    while (text[j] === " " || text[j] === "\t") j += 1;
    const next = () => {
      const nl = text.indexOf("\n", j);
      i = nl === -1 ? n : nl + 1;
    };
    if (j >= n) break;
    const c = text[j];
    if (c === "\n" || c === "\r" || c === "#") {
      next();
      continue;
    }
    const eol = lineEndAt(text, j);
    if (c === "[") {
      const line = text.slice(j, eol);
      const array = /^\[\[\s*([^\]]*?)\s*\]\]/u.exec(line);
      const header = array ?? /^\[\s*([^\]]*?)\s*\]/u.exec(line);
      if (header) items.push({ kind: "table", array: Boolean(array), name: header[1], start, lineEnd: eol });
      next();
      continue;
    }
    let key;
    let at = j;
    if (c === '"' || c === "'") {
      const k = parseValue(text, j, true);
      [key, at] = [k.value, k.end];
    } else {
      while (at < eol && text[at] !== "=") at += 1;
      key = text.slice(j, at).trim();
    }
    at = skipFiller(text, at, false);
    if (text[at] !== "=" || at >= eol) {
      next();
      continue;
    }
    const valueStart = skipFiller(text, at + 1, false);
    if (valueStart >= eol || text[valueStart] === "#") {
      next();
      continue;
    }
    const r = parseValue(text, valueStart);
    items.push({ kind: "kv", key, value: r.value, valueStart, valueEnd: r.end, lineEnd: lineEndAt(text, r.end) });
    const nl = text.indexOf("\n", r.end);
    i = nl === -1 ? n : nl + 1;
  }
  return items;
}

const stringOf = (value) => (typeof value === "string" ? value : null);

export function parseConfig(text) {
  const config = { client: {}, credentials: [], trust: { pairings: [] } };
  let table = "";
  let current = null;
  for (const item of scanConfig(text)) {
    if (item.kind === "table") {
      table = item.name;
      current = null;
      if (item.array && table === "credential") {
        current = { url: null, member: null, token: null };
        config.credentials.push(current);
      }
      continue;
    }
    const { key, value } = item;
    if (table === "trust" && key === "pairings" && Array.isArray(value)) {
      config.trust.pairings = value
        .filter((p) => p && typeof p === "object" && !Array.isArray(p))
        .filter((p) => stringOf(p.path) && stringOf(p.url))
        .map((p) => ({ path: p.path, url: p.url, member: stringOf(p.member) ?? null }));
    } else if (table === "client" && ["url", "member", "token"].includes(key)) {
      config.client[key] = stringOf(value) ?? "";
    } else if (table === "client" && key === "timeout") {
      config.client.timeout = Number(value);
    } else if (current && ["url", "member", "token"].includes(key)) {
      current[key] = stringOf(value) ?? "";
    }
  }
  // A legacy [client] token reads as one credential with the member unknown.
  if (present(config.client.token) && present(config.client.url)) {
    config.credentials.unshift({ url: config.client.url, member: null, token: config.client.token });
  }
  delete config.client.token;
  return config;
}

export function readConfigFile(path, readFile = (p) => readFileSync(p, "utf8")) {
  try {
    return parseConfig(readFile(path));
  } catch {
    return parseConfig("");
  }
}

// The lock-dir protocol: mkdir <target>.lock, an owner line, and stale breaking
// serialised through <target>.lock.break so a stale decision can never remove a
// lock another process has taken since.

const BREAK_STALE_MS = 10_000;

const sleepSync = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

function readOwner(dir) {
  try {
    const line = readFileSync(`${dir}/owner`, "utf8").trim();
    const [pid, host, epoch] = line.split(" ");
    if (/^\d+$/u.test(pid) && host && /^\d+$/u.test(epoch)) return { line, pid: Number(pid), host, epoch: Number(epoch) };
  } catch {
    // missing or unreadable
  }
  return null;
}

// "gone", "held" (fresh, or alive here) or "stale".
function lockState(dir, { host, now, staleMs, isAlive }) {
  const owner = readOwner(dir);
  let age;
  if (owner) age = now() - owner.epoch;
  else {
    try {
      age = now() - statSync(dir).mtimeMs;
    } catch {
      return "gone";
    }
  }
  if (age <= staleMs) return "held";
  if (owner && owner.host === host && isAlive(owner.pid)) return "held";
  return "stale";
}

// Creates a file exclusively with mode 0600 and writes the text before the
// file is renamed or read by anyone. A leftover at the path is removed first.
function createPrivate(path, text) {
  rmSync(path, { force: true });
  const fd = openSync(path, "wx", 0o600);
  try {
    writeSync(fd, text);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

// Breaks a stale lock. Only the holder of <dir>.break may rename or remove
// <dir>, and it decides staleness again once it holds it. True when the lock is
// gone and the caller may try to take it.
function tryBreak(dir, ctx) {
  const state = lockState(dir, ctx);
  if (state === "gone") return true;
  if (state === "held") return false;
  const gate = `${dir}.break`;
  const take = () => {
    try {
      mkdirSync(gate);
      return true;
    } catch (error) {
      if (error.code === "EEXIST") return false;
      throw error;
    }
  };
  if (!take()) {
    let age = 0;
    try {
      age = ctx.now() - statSync(gate).mtimeMs;
    } catch {
      // released meanwhile: try once more below
    }
    if (age <= BREAK_STALE_MS) return false;
    rmSync(gate, { recursive: true, force: true });
    if (!take()) return false;
  }
  try {
    const again = lockState(dir, ctx);
    if (again === "gone") return true;
    if (again === "held") return false;
    const aside = `${dir}.stale-${ctx.pid}-${randomBytes(4).toString("hex")}`;
    try {
      renameSync(dir, aside);
    } catch {
      return false;
    }
    rmSync(aside, { recursive: true, force: true });
    return true;
  } finally {
    try {
      rmdirSync(gate);
    } catch {
      // already gone
    }
  }
}

export function withLock(target, fn, options = {}) {
  const {
    budgetMs = 5000,
    staleMs = 30_000,
    pid = process.pid,
    host = hostname(),
    now = () => Date.now(),
    sleep = sleepSync,
    isAlive = alive,
    label = "config.toml",
  } = options;
  const dir = `${target}.lock`;
  const ownerLine = `${pid} ${host} ${now()}`;
  const deadline = now() + budgetMs;
  mkdirSync(dirname(target), { recursive: true, mode: 0o700 });

  const acquire = () => {
    try {
      mkdirSync(dir);
    } catch (error) {
      if (error.code === "EEXIST") return false;
      throw error;
    }
    const tmp = `${dir}/owner.tmp`;
    createPrivate(tmp, `${ownerLine}\n`);
    renameSync(tmp, `${dir}/owner`);
    return true;
  };

  let held = acquire();
  while (!held) {
    if (tryBreak(dir, { pid, host, now, staleMs, isAlive }) && acquire()) {
      held = true;
      break;
    }
    if (now() >= deadline) {
      const owner = readOwner(dir);
      throw new Error(`${label} is locked by pid ${owner?.pid ?? "unknown"} on ${owner?.host ?? "unknown"}`);
    }
    sleep(50);
    held = acquire();
  }
  try {
    return fn();
  } finally {
    const owner = readOwner(dir);
    if (owner?.line === ownerLine) {
      try {
        unlinkSync(`${dir}/owner`);
        rmdirSync(dir);
      } catch {
        // already released
      }
    }
  }
}

function writeAtomic(path, text, pid) {
  const tmp = `${path}.tmp.${pid}`;
  try {
    createPrivate(tmp, text);
    renameSync(tmp, path);
  } catch (error) {
    rmSync(tmp, { force: true });
    throw error;
  }
}

const quote = (value) =>
  `"${String(value)
    .replace(/\\/gu, "\\\\")
    .replace(/"/gu, '\\"')
    .replace(/\n/gu, "\\n")
    .replace(/\r/gu, "\\r")
    .replace(/\t/gu, "\\t")}"`;

// The [[credential]] blocks of the scanned text: header, fields by key, and the
// last item, which a new field is inserted after.
function credentialBlocks(items) {
  const blocks = [];
  let block = null;
  for (const item of items) {
    if (item.kind === "table") {
      block = item.array && item.name === "credential" ? { header: item, fields: new Map(), last: item } : null;
      if (block) blocks.push(block);
    } else if (block) {
      if (!block.fields.has(item.key)) block.fields.set(item.key, item);
      block.last = item;
    }
  }
  return blocks;
}

const fieldOf = (block, key) => {
  const value = block.fields.get(key)?.value;
  return typeof value === "string" ? value : null;
};

// The text change that sets a key in a block: the value's whole span when the
// key exists (its inline comment stays), else a new line after the last item.
function setField(block, key, value, eol) {
  const kv = block.fields.get(key);
  if (kv) return { start: kv.valueStart, end: kv.valueEnd, text: quote(value) };
  const at = block.last.lineEnd;
  return { start: at, end: at, text: `${eol}${key} = ${quote(value)}` };
}

const applyEdits = (text, edits) =>
  [...edits].sort((a, b) => b.start - a.start).reduce((out, e) => out.slice(0, e.start) + e.text + out.slice(e.end), text);

function appendBlock(text, eol, lines) {
  const finalNewline = text === "" || text.endsWith("\n");
  let out = text;
  if (out !== "" && !out.endsWith("\n")) out += eol;
  if (out !== "" && !/(?:^|\n)[ \t]*\r?\n$/u.test(out)) out += eol;
  out += lines.join(eol) + eol;
  return finalNewline ? out : out.slice(0, -eol.length);
}

function edit(path, mutate, options) {
  const pid = options?.pid ?? process.pid;
  withLock(
    path,
    () => {
      let text = "";
      try {
        text = readFileSync(path, "utf8");
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      const eol = text.includes("\r\n") ? "\r\n" : "\n";
      const next = mutate(text, credentialBlocks(scanConfig(text)), eol);
      if (next !== text) writeAtomic(path, next, pid);
    },
    options,
  );
}

// Stores the token for (url, member): replaces that entry, else appends one. A
// legacy entry for the url with no member is left alone.
export function writeCredential(path, url, member, token, options) {
  edit(
    path,
    (text, blocks, eol) => {
      const hit = blocks.find((b) => same(fieldOf(b, "url"), url) && present(fieldOf(b, "member")) === present(member));
      if (hit) return applyEdits(text, [setField(hit, "token", token, eol)]);
      return appendBlock(text, eol, ["[[credential]]", `url = ${quote(url)}`, `member = ${quote(member)}`, `token = ${quote(token)}`]);
    },
    options,
  );
}

// Renames a member of the credential for url in place; the token stays.
export function rekey(path, url, from, to, options) {
  edit(
    path,
    (text, blocks, eol) => {
      const edits = blocks
        .filter((b) => same(fieldOf(b, "url"), url) && present(fieldOf(b, "member")) === present(from))
        .map((b) => setField(b, "member", to, eol));
      return applyEdits(text, edits);
    },
    options,
  );
}
