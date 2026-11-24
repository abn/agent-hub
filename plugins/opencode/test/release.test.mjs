// The release checks for the OpenCode plugin: what a child session is called on
// the hub, which hub calls carry which session, and what a deleted session
// does. The plugin runs against a hub that keeps the session names it was
// given, so a name taken is refused the way the real hub refuses it, and
// against the slice of the host the plugin touches.

import { createServer } from "node:http";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { createPlugin } from "../plugin.mjs";

const scratchRoot = fileURLToPath(new URL("../../../target/tmp/", import.meta.url));

// A hub that speaks streamable HTTP MCP for the calls the plugin makes. It
// records every tool call and holds the session names it has started.
async function startHub() {
  const hub = { calls: [], started: new Map(), refuse: new Set(), ids: 0, mcp: new Set() };
  const send = (res, status, body, headers = {}) => {
    res.writeHead(status, { "content-type": "application/json", ...headers });
    res.end(JSON.stringify(body));
  };
  const result = (structuredContent) => ({ content: [], structuredContent });
  const refusal = (reason, message, details = {}) => ({
    isError: true,
    content: [{ type: "text", text: JSON.stringify({ error: { code: "conflict", message, retryable: false, details: { reason, ...details } } }) }],
    structuredContent: { error: { code: "conflict", message, retryable: false, details: { reason, ...details } } },
  });

  const tools = {
    session_start(args) {
      const name = args.session_name;
      if (args.exclusive && (hub.started.has(name) || hub.refuse.has(name))) {
        return refusal("session_name_taken", `session ${name} exists`, { session_id: hub.started.get(name) ?? "other" });
      }
      if (!hub.started.has(name)) hub.started.set(name, `01HUB${++hub.ids}`);
      return result({ session_id: hub.started.get(name), session_name: name, project_id: args.project_id, handoff: "", anchor: null });
    },
  };

  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://hub");
    if (req.method === "GET" && url.pathname === "/api/v1/projects") return send(res, 200, { projects: [{ id: "proj" }] });
    if (req.method === "GET" && url.pathname === "/api/v1/me/pending") {
      const timer = setTimeout(() => send(res, 200, { count: 0, items: [], cursor: "c" }), 40);
      res.on("close", () => clearTimeout(timer));
      return undefined;
    }
    if (url.pathname === "/mcp" && req.method === "DELETE") {
      res.writeHead(204).end();
      return undefined;
    }
    if (url.pathname === "/mcp" && req.method === "POST") {
      let raw = "";
      req.on("data", (chunk) => (raw += chunk));
      req.on("end", () => {
        const message = JSON.parse(raw);
        if (message.id === undefined) return void res.writeHead(202).end();
        const reply = (value, headers = {}) => send(res, 200, { jsonrpc: "2.0", id: message.id, result: value }, headers);
        if (message.method === "initialize") {
          const id = `mcp-${hub.mcp.size + 1}`;
          hub.mcp.add(id);
          return reply({ protocolVersion: "2025-06-18", capabilities: {}, serverInfo: { name: "hub" } }, { "mcp-session-id": id });
        }
        if (message.method === "tools/call") {
          const { name, arguments: args = {} } = message.params;
          hub.calls.push({ name, args });
          return reply((tools[name] ?? (() => result({})))(args));
        }
        return send(res, 200, { jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "no method" } });
      });
      return undefined;
    }
    return send(res, 404, {});
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  hub.url = `http://127.0.0.1:${server.address().port}`;
  hub.named = (tool) => hub.calls.filter((call) => call.name === tool);
  hub.stop = () =>
    new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    });
  return hub;
}

// The slice of the OpenCode host the plugin touches: the MCP transform, the
// tool hook, the session hooks, and an event feed the test pushes into.
function fakeHost(scratch) {
  const feed = [];
  const waiters = [];
  const host = {
    options: {},
    location: { directory: scratch },
    mcps: {},
    sessions: {},
    hooks: {},
    toolHooks: {},
    mcp: {
      async transform(callback) {
        callback({
          get: (name) => host.mcps[name],
          set: (name, config) => (host.mcps[name] = config),
          list: () => Object.keys(host.mcps),
          update: (name, patch) => (host.mcps[name] = { ...host.mcps[name], ...patch }),
          remove: (name) => delete host.mcps[name],
        });
      },
    },
    tool: {
      async hook(name, callback) {
        host.toolHooks[name] = callback;
      },
    },
    session: {
      async hook(name, callback) {
        host.hooks[name] = callback;
      },
      async get({ sessionID }) {
        return host.sessions[sessionID];
      },
      async prompt() {
        return {};
      },
      instructions: { entry: { async put() {} } },
    },
    event: {
      subscribe({ signal }) {
        return {
          async *[Symbol.asyncIterator]() {
            for (;;) {
              while (feed.length) yield feed.shift();
              if (signal.aborted) return;
              await new Promise((resolve) => {
                waiters.push(resolve);
                signal.addEventListener("abort", resolve, { once: true });
              });
            }
          },
        };
      },
    },
    emit(event) {
      feed.push(event);
      waiters.splice(0).forEach((resolve) => resolve());
    },
  };
  return host;
}

const until = async (check, ms = 4000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (check()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out waiting for the condition");
};

let hub;
let scratch;
const disposers = [];
afterEach(async () => {
  for (const dispose of disposers.splice(0)) await dispose();
  await hub?.stop();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
  hub = scratch = undefined;
});

async function boot() {
  if (!hub) hub = await startHub();
  if (!scratch) {
    mkdirSync(scratchRoot, { recursive: true });
    scratch = mkdtempSync(join(scratchRoot, "opencode-release-"));
  }
  const host = fakeHost(scratch);
  const plugin = createPlugin({
    env: {
      HUB_URL: hub.url,
      HUB_TOKEN: "tok",
      HUB_PROJECT: "proj",
      HUB_SESSION: "work",
      HUB_CONFIG: join(scratch, "no-config.toml"),
      XDG_STATE_HOME: join(scratch, "state"),
      HOME: scratch,
    },
    pause: (ms, signal) =>
      new Promise((resolve) => {
        const timer = setTimeout(resolve, Math.min(ms, 20));
        signal?.addEventListener("abort", () => (clearTimeout(timer), resolve()), { once: true });
      }),
  });
  const dispose = await plugin.setup(host);
  disposers.push(dispose);
  return { host, dispose };
}

const created = (sessionID, parentID, agent) => ({
  type: "session.created",
  data: { sessionID, parentID, agent, location: { directory: scratch } },
});

// Open a root session and wait until the hub has seen its start.
async function openRoot(host, id = "ses_root0000root") {
  host.sessions[id] = { sessionID: id, location: { directory: scratch } };
  host.emit(created(id, undefined, "build"));
  await until(() => hub.named("session_start").some((call) => call.args.session_name === "work"));
  return id;
}

// Open a child session of `parentID` and return the arguments of the start it made.
async function openChild(host, parentID, id, agent = "general") {
  host.sessions[id] = { sessionID: id, parentID, agent, location: { directory: scratch } };
  const before = hub.named("session_start").length;
  host.emit(created(id, parentID, agent));
  await until(() => hub.named("session_start").length > before);
  return hub.named("session_start").at(-1).args;
}

// The tool name the host gives a hub tool: the server's key, an underscore, the tool.
const toolName = (host, tool) => `${Object.keys(host.mcps)[0]}_${tool}`;

const alnum = (id) => id.replace(/[^a-z0-9]/gi, "").toLowerCase();

describe("a child session on the hub", () => {
  it("starts as <parent>/<type>-<id8>, exclusively, under a live parent", async () => {
    const { host } = await boot();
    await openRoot(host);
    const childId = "ses_df68c1b96ABCD";
    const args = await openChild(host, "ses_root0000root", childId, "general");

    expect(args.session_name).toMatch(/^work\/general-[a-z0-9]{8}$/);
    const id8 = args.session_name.split("-").at(-1);
    expect([alnum(childId), alnum(childId).replace(/^ses/, "")].some((id) => id.startsWith(id8))).toBe(true);
    expect(args.exclusive, "a child is started exclusively so a collision is the hub's to see").toBe(true);
    expect(args.project_id).toBe("proj");
    // The root keeps its own name: it was started once, not exclusively renamed.
    expect(hub.named("session_start").filter((call) => call.args.session_name === "work")).toHaveLength(1);
  });

  it("steps to -2 and on when the hub says the name is taken, and never reuses a taken one", async () => {
    const { host } = await boot();
    await openRoot(host);
    const childId = "ses_taken0000aaaa";
    const first = `work/general-${alnum(childId).slice(0, 8)}`;
    hub.refuse.add(first);
    hub.refuse.add(`${first}-2`);

    await openChild(host, "ses_root0000root", childId);
    await until(() => hub.named("session_start").some((call) => call.args.session_name === `${first}-3`));
    const children = hub.named("session_start").filter((call) => call.args.session_name.startsWith("work/"));
    expect(children.map((call) => call.args.session_name)).toEqual([first, `${first}-2`, `${first}-3`]);
    expect(children.every((call) => call.args.exclusive)).toBe(true);
  });

  it("carries its own session on hub calls and leaves the root's alone", async () => {
    const { host } = await boot();
    await openRoot(host);
    const child = await openChild(host, "ses_root0000root", "ses_carry0000bbbb");
    const hook = host.toolHooks["execute.before"];
    expect(hook, "the plugin registers the tool hook").toBeTypeOf("function");

    const asChild = { tool: toolName(host, "brain_put"), sessionID: "ses_carry0000bbbb", input: { path: "/kv/a", content: "x" } };
    await hook(asChild);
    expect(asChild.input.session).toEqual({ project_id: "proj", name: child.session_name });

    const asRoot = { tool: toolName(host, "brain_put"), sessionID: "ses_root0000root", input: { path: "/kv/a", content: "x" } };
    await hook(asRoot);
    expect(asRoot.input.session).toEqual({ project_id: "proj", name: "work" });

    const explicit = { tool: toolName(host, "brain_put"), sessionID: "ses_carry0000bbbb", input: { path: "/kv/a", session: { project_id: "proj", name: "other" } } };
    await hook(explicit);
    expect(explicit.input.session, "an explicit session is never replaced").toEqual({ project_id: "proj", name: "other" });

    const starting = { tool: toolName(host, "session_start"), sessionID: "ses_carry0000bbbb", input: { project_id: "proj", session_name: "x" } };
    await hook(starting);
    expect(starting.input.session, "session_start names its own session").toBeUndefined();

    const foreign = { tool: "other_brain_put", sessionID: "ses_carry0000bbbb", input: { path: "/kv/a" } };
    await hook(foreign);
    expect(foreign.input.session, "another server's tools are not touched").toBeUndefined();
  });

  it("resumes its own name, not a new one, when the host reopens a child it already mapped", async () => {
    const first = await boot();
    await openRoot(first.host);
    const childId = "ses_resume0000cccc";
    const opened = await openChild(first.host, "ses_root0000root", childId);
    await first.dispose();
    disposers.pop();

    const second = await boot();
    await openRoot(second.host, "ses_root0000root");
    const before = hub.named("session_start").length;
    second.host.sessions[childId] = { sessionID: childId, parentID: "ses_root0000root", agent: "general", location: { directory: scratch } };
    second.host.emit(created(childId, "ses_root0000root", "general"));
    const childStarts = () => hub.named("session_start").slice(before).filter((call) => call.args.session_name.startsWith("work/"));
    await until(() => childStarts().length > 0);
    const resumed = childStarts()[0];
    expect(resumed.args.session_name, "the child keeps the name it had").toBe(opened.session_name);
    expect(resumed.args.exclusive === true, "a mapped child resumes rather than claiming").toBe(false);
  });

  it("leaves its session when it is deleted, and ends nothing", async () => {
    const { host } = await boot();
    await openRoot(host);
    const child = await openChild(host, "ses_root0000root", "ses_leave0000dddd");

    host.emit({ type: "session.deleted", data: { sessionID: "ses_leave0000dddd" } });
    await until(() => hub.named("session_leave").length > 0);
    expect(hub.named("session_leave")[0].args.session).toEqual({ project_id: "proj", name: child.session_name });

    host.emit({ type: "session.deleted", data: { sessionID: "ses_root0000root" } });
    await until(() => hub.named("session_leave").length > 1);
    expect(hub.named("session_leave")[1].args.session).toEqual({ project_id: "proj", name: "work" });
    expect(hub.named("session_end"), "a deleted session is left, never ended").toHaveLength(0);
  });
});
