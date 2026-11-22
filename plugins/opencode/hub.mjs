// The hub client: REST peek and JSON-RPC over streamable HTTP MCP, with fetch only.

const PROTOCOL_VERSION = "2025-06-18";
const CLIENT_INFO = { name: "agent-hub-opencode", version: "0.1.0" };

export class HubError extends Error {
  constructor(message, status) {
    super(message);
    this.name = "HubError";
    this.status = status;
  }
}

function sseMessages(stream) {
  return {
    async *[Symbol.asyncIterator]() {
      const reader = stream.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (value) buffer += decoder.decode(value, { stream: !done });
          if (done) buffer += "\n\n";
          const events = buffer.split(/\r?\n\r?\n/u);
          buffer = events.pop();
          for (const event of events) {
            const data = event
              .split(/\r?\n/u)
              .filter((line) => line.startsWith("data:"))
              .map((line) => line.slice(5).replace(/^ /u, ""))
              .join("\n");
            if (data) {
              try {
                yield JSON.parse(data);
              } catch {
                // a non-JSON event is not a JSON-RPC message
              }
            }
          }
          if (done) return;
        }
      } finally {
        await reader.cancel().catch(() => {});
      }
    },
  };
}

function toolPayload(result) {
  if (result.structuredContent !== undefined && result.structuredContent !== null) return result.structuredContent;
  const text = (result.content ?? []).find((part) => part.type === "text")?.text;
  if (text === undefined) return {};
  try {
    return JSON.parse(text);
  } catch {
    return { text };
  }
}

export const DEFAULT_TIMEOUTS = { hook: 5000, startup: 10_000, peekSlack: 10_000 };

const normalizeAll = (items) => (items ?? []).map(normalizeItem);

export function createHub({ url, token, agentName, fetch: fetchImpl = globalThis.fetch, timeouts = {} }) {
  const limits = { ...DEFAULT_TIMEOUTS, ...timeouts };
  const base = url.replace(/\/+$/u, "");
  const headers = () => {
    const h = {};
    if (token) h.Authorization = `Bearer ${token}`;
    if (agentName) h["X-Agent-Id"] = agentName;
    return h;
  };

  // One deadline for the whole request, body included, joined with the caller's signal.
  const limited = (deadline, signal) => {
    const ms = typeof deadline === "number" ? deadline : limits[deadline];
    const timer = AbortSignal.timeout(ms);
    return signal ? AbortSignal.any([signal, timer]) : timer;
  };

  async function rest(path, { signal, deadline = "startup" } = {}) {
    const res = await fetchImpl(`${base}${path}`, {
      headers: { ...headers(), Accept: "application/json" },
      signal: limited(deadline, signal),
    });
    if (!res.ok) throw new HubError(`GET ${path.split("?")[0]} answered ${res.status}`, res.status);
    return res.json();
  }

  // Waits for something newer than `after`, the cursor of the previous reply, to be
  // pending for the session. Advances nothing.
  async function peek({ session, wait = 60, after, signal, deadline }) {
    const query = new URLSearchParams();
    if (session) query.set("session", session);
    query.set("wait", String(wait));
    if (after) query.set("after", after);
    const body = await rest(`/api/v1/me/pending?${query}`, {
      signal,
      deadline: deadline ?? wait * 1000 + limits.peekSlack,
    });
    return { count: body.count ?? (body.items ?? []).length, items: normalizeAll(body.items), cursor: body.cursor ?? null };
  }

  async function projects({ signal, deadline } = {}) {
    const body = await rest("/api/v1/projects", { signal, deadline });
    return (body.projects ?? []).map((project) => project.id).filter(Boolean);
  }

  let sessionId = null;
  let protocolVersion = null;
  let ready = null;
  let closed = false;
  let nextId = 1;

  function post(message, signal, sid = sessionId) {
    const h = {
      ...headers(),
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
    };
    if (sid) h["Mcp-Session-Id"] = sid;
    if (protocolVersion) h["MCP-Protocol-Version"] = protocolVersion;
    return fetchImpl(`${base}/mcp`, { method: "POST", headers: h, body: JSON.stringify(message), signal });
  }

  const isReply = (message, id) =>
    message && typeof message === "object" && message.id === id && ("result" in message || "error" in message);

  async function readReply(res, id) {
    const type = res.headers.get("content-type") ?? "";
    if (type.includes("text/event-stream")) {
      for await (const message of sseMessages(res.body)) {
        if (isReply(message, id)) return message;
      }
      throw new HubError("the MCP stream ended without a reply", res.status);
    }
    let body;
    try {
      body = await res.json();
    } catch {
      throw new HubError("the MCP reply is not JSON", res.status);
    }
    const reply = (Array.isArray(body) ? body : [body]).find((message) => isReply(message, id));
    if (!reply) throw new HubError("the MCP reply does not answer the request", res.status);
    return reply;
  }

  async function initialize(signal) {
    sessionId = null;
    protocolVersion = null;
    const id = nextId++;
    const res = await post(
      {
        jsonrpc: "2.0",
        id,
        method: "initialize",
        params: { protocolVersion: PROTOCOL_VERSION, capabilities: {}, clientInfo: CLIENT_INFO },
      },
      signal,
    );
    if (!res.ok) throw new HubError(`MCP initialize answered ${res.status}`, res.status);
    const reply = await readReply(res, id);
    if (reply.error) throw new HubError(`MCP initialize failed: ${reply.error.message}`, res.status);
    const negotiated = reply.result?.protocolVersion;
    if (typeof negotiated !== "string" || !negotiated) {
      throw new HubError("MCP initialize did not return a protocol version", res.status);
    }
    if (closed) throw new HubError("the hub client is closed");
    sessionId = res.headers.get("mcp-session-id");
    protocolVersion = negotiated;
    const ack = await post({ jsonrpc: "2.0", method: "notifications/initialized" }, signal);
    await ack.body?.cancel().catch(() => {});
    if (!ack.ok) throw new HubError(`MCP initialized notification answered ${ack.status}`, ack.status);
  }

  // Initialization is shared and runs on its own deadline, so one caller aborting
  // neither fails the others nor leaves them waiting on it.
  function ensure(signal) {
    if (closed) throw new HubError("the hub client is closed");
    ready ??= initialize(limited("startup")).catch((error) => {
      ready = null;
      throw error;
    });
    const shared = ready;
    if (!signal) return shared;
    return new Promise((resolve, reject) => {
      if (signal.aborted) return reject(signal.reason);
      const abort = () => reject(signal.reason);
      signal.addEventListener("abort", abort, { once: true });
      shared.then(resolve, reject).finally(() => signal.removeEventListener("abort", abort));
    });
  }

  async function rpc(method, params, { signal, retried = false } = {}) {
    await ensure(signal);
    if (closed) throw new HubError("the hub client is closed");
    const used = sessionId;
    const id = nextId++;
    const res = await post({ jsonrpc: "2.0", id, method, params }, signal, used);
    if (res.status === 404 && used && !retried) {
      await res.body?.cancel().catch(() => {});
      // A late 404 for an old session must not drop a newer one.
      if (sessionId === used) {
        sessionId = null;
        ready = null;
      }
      return rpc(method, params, { signal, retried: true });
    }
    if (!res.ok) throw new HubError(`MCP ${method} answered ${res.status}`, res.status);
    const reply = await readReply(res, id);
    if (reply.error) throw new HubError(`MCP ${method} failed: ${reply.error.message}`, res.status);
    return reply.result ?? {};
  }

  // `trailers`, when given, collects the items a result reports as pending.
  async function callTool(name, args = {}, { signal, deadline = "startup", trailers } = {}) {
    const result = await rpc("tools/call", { name, arguments: args }, { signal: limited(deadline, signal) });
    const payload = toolPayload(result);
    if (trailers) trailers.push(...normalizeAll(payload.notifications?.pending));
    if (result.isError) throw new HubError(`${name} failed: ${payload.text ?? payload.message ?? JSON.stringify(payload)}`);
    return payload;
  }

  // Drains what is pending for the session, once, and returns it as items.
  async function drain(hubSessionId, { signal, deadline = "hook", trailers } = {}) {
    const args = { wait_seconds: 0 };
    if (hubSessionId) args.session = { session_id: hubSessionId };
    const body = await callTool("notify_wait", args, { signal, deadline, trailers });
    return normalizeAll(body.notifications?.pending);
  }

  async function close() {
    closed = true;
    if (!sessionId) return;
    const id = sessionId;
    sessionId = null;
    protocolVersion = null;
    ready = null;
    try {
      const res = await fetchImpl(`${base}/mcp`, {
        method: "DELETE",
        headers: { ...headers(), "Mcp-Session-Id": id },
        signal: limited("hook"),
      });
      await res.body?.cancel().catch(() => {});
    } catch {
      // the hub drops an idle session by itself
    }
  }

  return { rest, peek, projects, callTool, drain, close, timeouts: limits };
}

export function normalizeItem(item) {
  return {
    id: item.id,
    kind: item.kind,
    title: item.title ?? item.summary ?? "",
    project: item.project ?? item.project_id ?? "",
    direct: item.direct === true,
  };
}
