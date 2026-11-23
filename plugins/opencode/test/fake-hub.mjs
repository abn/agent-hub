// A hub on a loopback port that speaks just enough REST and streamable HTTP MCP
// for the plugin: the peek long poll, JSON-RPC over JSON or SSE, and the tools
// the session bundle and the loop call.

import { createServer } from "node:http";

// A refusal the way the hub sends one: a JSON-RPC error whose data holds the
// hub's own error object.
class RpcFail extends Error {
  constructor(code, reason, message, extra = {}) {
    super(message);
    this.error = { code, message, retryable: false, details: { reason, ...extra } };
  }
}

export async function startFakeHub({ sse = false, pollMs = 60 } = {}) {
  const hub = {
    sse,
    pending: [],
    mcpSessions: new Set(),
    calls: [],
    peeks: [],
    requests: [],
    initializations: 0,
    failPeeks: 0,
    initDelay: 0,
    protocolVersion: "2025-06-18",
    failTools: {},
    trailerFor: {},
    delayTools: {},
    sessionIds: [],
    stallTools: new Set(),
    stallBodyTools: new Set(),
    mangle: (_message, body) => body,
    projects: ["proj"],
    brief: { answers: { items: [], more: 0 }, events: { items: [], more: 0 }, stale_pages: { items: [], more: 0 } },
    sessionStart: { session_id: "s_01TEST", handoff: "", anchor: null },
    sessionNames: new Set(),
    left: [],
    whoami: {
      state: "active",
      member: "tester",
      agent: "opencode",
      kind: "participant",
      default_project: "tester-space",
      suggested_session: "opencode-2026-11-23",
      personal_project: "tester-space",
      admin: false,
      actor: "tester",
    },
    enrolStatus: { status: "pending", member: "tester" },
    statusRequests: [],
  };

  const drainTrailer = () => {
    const items = hub.pending.splice(0);
    return items.length ? { notifications: { pending: items } } : {};
  };

  const tools = {
    whoami: () => ({ ...hub.whoami }),
    session_brief: () => hub.brief,
    session_start: (args) => {
      if (args.exclusive && hub.sessionNames.has(args.session_name)) {
        throw new RpcFail("conflict", "session_name_taken", `session '${args.session_name}' already exists`, { session_id: "s_OLD" });
      }
      hub.sessionNames.add(args.session_name);
      return { ...hub.sessionStart, ...(hub.sessionIds.length ? { session_id: hub.sessionIds.shift() } : {}) };
    },
    session_leave: (args) => {
      hub.left.push(args.session);
      return { ok: true };
    },
    brain_get: () => ({ content: "# Index\n- page.md" }),
    notify_subscribe: () => ({ subscription_id: "sub_1" }),
    notify_wait: drainTrailer,
    session_end: () => ({ ok: true }),
  };

  const send = (res, status, body, headers = {}) => {
    res.writeHead(status, { "content-type": "application/json", ...headers });
    res.end(JSON.stringify(body));
  };

  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://hub");
    hub.requests.push({ method: req.method, path: url.pathname, headers: req.headers });
    if (req.method === "GET" && url.pathname === "/api/v1/projects") {
      return send(res, 200, { projects: hub.projects.map((id) => ({ id })) });
    }
    if (req.method === "GET" && url.pathname === "/api/v1/enrol/status") {
      hub.statusRequests.push({ search: url.search, headers: req.headers });
      if (hub.enrolStatus === null) return send(res, 401, { error: { code: "unauthenticated" } });
      return send(res, 200, hub.enrolStatus);
    }
    if (req.method === "GET" && url.pathname === "/api/v1/me/pending") {
      const after = url.searchParams.get("after");
      hub.peeks.push({ session: url.searchParams.get("session"), wait: url.searchParams.get("wait"), after });
      if (hub.failPeeks > 0) {
        hub.failPeeks -= 1;
        return send(res, 500, {});
      }
      // The cursor is opaque and unrelated to item ids: it lists what a response showed.
      const seen = new Set(after?.startsWith("cur:") ? after.slice(4).split("+").filter(Boolean) : []);
      const fresh = () => hub.pending.filter((item) => !seen.has(item.id));
      const cursor = () => `cur:${hub.pending.map((item) => item.id).join("+")}`;
      const answer = (items) => send(res, 200, { count: items.length, items, cursor: cursor() });
      if (fresh().length) return answer(fresh());
      // A held peek that reaches its deadline returns everything pending.
      const timer = setTimeout(() => answer(hub.pending.slice()), pollMs);
      const poll = setInterval(() => {
        if (!fresh().length) return;
        clearTimeout(timer);
        clearInterval(poll);
        answer(fresh());
      }, 10);
      res.on("close", () => {
        clearTimeout(timer);
        clearInterval(poll);
      });
      return undefined;
    }
    if (url.pathname === "/mcp" && req.method === "DELETE") {
      hub.mcpSessions.delete(req.headers["mcp-session-id"]);
      res.writeHead(204).end();
      return undefined;
    }
    if (url.pathname === "/mcp" && req.method === "POST") {
      let raw = "";
      req.on("data", (chunk) => (raw += chunk));
      req.on("end", () => {
        const message = JSON.parse(raw);
        const sessionId = req.headers["mcp-session-id"];
        if (message.method !== "initialize" && !hub.mcpSessions.has(sessionId)) {
          return send(res, 404, { error: "unknown session" });
        }
        if (message.id === undefined) return void res.writeHead(202).end();
        const reply = (result, headers = {}) => {
          const body = hub.mangle(message, { jsonrpc: "2.0", id: message.id, result });
          if (!hub.sse) return send(res, 200, body, headers);
          res.writeHead(200, { "content-type": "text/event-stream", ...headers });
          res.write(`data: ${JSON.stringify({ jsonrpc: "2.0", method: "notifications/message", params: {} })}\n\n`);
          res.end(`id: 1\ndata: ${JSON.stringify(body)}\n\n`);
          return undefined;
        };
        if (message.method === "initialize") {
          hub.initializations += 1;
          const id = `mcp-${hub.initializations}`;
          hub.mcpSessions.add(id);
          const answer = () => reply({ protocolVersion: hub.protocolVersion, capabilities: {}, serverInfo: { name: "fake" } }, { "mcp-session-id": id });
          if (hub.initDelay) setTimeout(answer, hub.initDelay);
          else answer();
          return undefined;
        }
        if (message.method === "tools/call") {
          const { name, arguments: args } = message.params;
          hub.calls.push({ name, args });
          if (hub.stallTools.has(name)) return undefined;
          if (hub.stallBodyTools.has(name)) {
            res.writeHead(200, { "content-type": "application/json" });
            res.flushHeaders();
            return undefined;
          }
          if (hub.failTools[name] > 0) {
            hub.failTools[name] -= 1;
            return reply({ isError: true, content: [{ type: "text", text: `${name} failed` }] });
          }
          const handler = tools[name];
          if (!handler) return reply({ isError: true, content: [{ type: "text", text: `no tool ${name}` }] });
          const run = () => {
            let structuredContent;
            try {
              structuredContent = handler(args);
            } catch (failure) {
              if (!(failure instanceof RpcFail)) throw failure;
              return send(res, 200, {
                jsonrpc: "2.0",
                id: message.id,
                error: { code: -32602, message: failure.message, data: { error: failure.error } },
              });
            }
            if (hub.trailerFor[name]) structuredContent.notifications = { pending: hub.trailerFor[name] };
            reply({ content: [], structuredContent });
          };
          if (hub.delayTools[name]) setTimeout(run, hub.delayTools[name]);
          else run();
          return undefined;
        }
        return send(res, 200, { jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "no method" } });
      });
      return undefined;
    }
    return send(res, 404, {});
  });

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  hub.url = `http://127.0.0.1:${server.address().port}`;
  hub.count = (name) => hub.calls.filter((call) => call.name === name).length;
  hub.stop = () =>
    new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    });
  return hub;
}
