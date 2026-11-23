import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import { CLIENT_INFO, createHub, HubError } from "../hub.mjs";
import { startFakeHub } from "./fake-hub.mjs";

let fake;
afterEach(async () => {
  await fake?.stop();
});

describe.each([
  ["JSON replies", false],
  ["SSE replies", true],
])("MCP over streamable HTTP with %s", (_label, sse) => {
  it("initializes once, carries the session id and calls a tool", async () => {
    fake = await startFakeHub({ sse });
    const hub = createHub({ url: fake.url, token: "tok", agentName: "opencode" });
    const who = await hub.callTool("whoami");
    await hub.callTool("session_brief", {});
    expect(who.actor).toBe("tester/opencode");
    expect(fake.initializations).toBe(1);
    const post = fake.requests.filter((r) => r.method === "POST").at(-1);
    expect(post.headers["mcp-session-id"]).toBe("mcp-1");
    expect(post.headers.authorization).toBe("Bearer tok");
    expect(post.headers["x-agent-id"]).toBe("opencode");
    expect(post.headers.accept).toContain("text/event-stream");
  });

  it("starts a new MCP session when the hub forgot the old one", async () => {
    fake = await startFakeHub({ sse });
    const hub = createHub({ url: fake.url, token: "tok", agentName: "opencode" });
    await hub.callTool("whoami");
    fake.mcpSessions.clear();
    const who = await hub.callTool("whoami");
    expect(who.actor).toBe("tester/opencode");
    expect(fake.initializations).toBe(2);
  });
});

describe("hub client", () => {
  it("raises a tool error", async () => {
    fake = await startFakeHub();
    const hub = createHub({ url: fake.url, token: "tok" });
    await expect(hub.callTool("nope")).rejects.toBeInstanceOf(HubError);
  });

  it("peeks with the session, the wait and the cursor and normalizes items", async () => {
    fake = await startFakeHub();
    fake.pending.push({ id: "evt_2", kind: "comment", title: "t", project_id: "proj", direct: true });
    const hub = createHub({ url: fake.url, token: "tok" });
    const peek = await hub.peek({ session: "s_1", wait: 60, after: "cur:evt_1" });
    expect(fake.peeks[0]).toEqual({ session: "s_1", wait: "60", after: "cur:evt_1" });
    expect(peek.cursor).toBe("cur:evt_2");
    expect(peek.items).toEqual([{ id: "evt_2", kind: "comment", title: "t", project: "proj", direct: true }]);
  });

  it("drains through notify_wait with a zero wait scoped to the session", async () => {
    fake = await startFakeHub();
    fake.pending.push({ id: "evt_1", kind: "answer", title: "a", project_id: "proj", direct: true });
    const hub = createHub({ url: fake.url, token: "tok" });
    const items = await hub.drain("s_1");
    expect(items.map((i) => i.id)).toEqual(["evt_1"]);
    expect(fake.calls.find((c) => c.name === "notify_wait").args).toEqual({ wait_seconds: 0, session: { session_id: "s_1" } });
    expect(await hub.drain("s_1")).toEqual([]);
  });

  it("ends its MCP session on close", async () => {
    fake = await startFakeHub();
    const hub = createHub({ url: fake.url, token: "tok" });
    await hub.callTool("whoami");
    await hub.close();
    expect(fake.mcpSessions.size).toBe(0);
  });
});

describe("deadlines", () => {
  const timeouts = { hook: 150, startup: 300, peekSlack: 150 };
  const elapsed = async (work) => {
    const started = Date.now();
    await work().then(
      () => {
        throw new Error("expected a rejection");
      },
      () => {},
    );
    return Date.now() - started;
  };

  it("gives up on a tool call the hub never answers", async () => {
    fake = await startFakeHub();
    fake.stallTools.add("notify_wait");
    const hub = createHub({ url: fake.url, token: "t", timeouts });
    expect(await elapsed(() => hub.drain("s_1"))).toBeLessThan(1500);
  });

  it("gives up on a reply whose body never arrives", async () => {
    fake = await startFakeHub();
    fake.stallBodyTools.add("notify_wait");
    const hub = createHub({ url: fake.url, token: "t", timeouts });
    expect(await elapsed(() => hub.drain("s_1"))).toBeLessThan(1500);
  });

  it("gives up on a peek that outlasts its wait plus the slack", async () => {
    fake = await startFakeHub({ pollMs: 10_000 });
    const hub = createHub({ url: fake.url, token: "t", timeouts });
    expect(await elapsed(() => hub.peek({ session: "s_1", wait: 0 }))).toBeLessThan(1500);
  });

  it("still stops at once when the caller aborts", async () => {
    fake = await startFakeHub({ pollMs: 10_000 });
    const hub = createHub({ url: fake.url, token: "t", timeouts: { ...timeouts, peekSlack: 60_000 } });
    const ctrl = new AbortController();
    setTimeout(() => ctrl.abort(), 50);
    expect(await elapsed(() => hub.peek({ session: "s_1", wait: 0, signal: ctrl.signal }))).toBeLessThan(1500);
  });
});

describe("startup deadline and DELETE deadline", () => {
  it("uses the startup deadline for an ordinary call", async () => {
    fake = await startFakeHub();
    fake.stallTools.add("whoami");
    const hub = createHub({ url: fake.url, token: "t", timeouts: { hook: 5000, startup: 150 } });
    const started = Date.now();
    await expect(hub.callTool("whoami")).rejects.toThrow();
    expect(Date.now() - started).toBeLessThan(1500);
  });

  it("does not wait on a DELETE the hub never answers", async () => {
    const stub = async (url, init) => {
      const body = init.body ? JSON.parse(init.body) : {};
      if (init.method === "DELETE") {
        return new Promise((_resolve, reject) => init.signal?.addEventListener("abort", () => reject(init.signal.reason)));
      }
      if (body.method === "initialize") {
        const result = { protocolVersion: "2025-06-18", capabilities: {} };
        return Response.json({ jsonrpc: "2.0", id: body.id, result }, { headers: { "mcp-session-id": "m1" } });
      }
      if (body.id === undefined) return new Response(null, { status: 202 });
      return Response.json({ jsonrpc: "2.0", id: body.id, result: { content: [], structuredContent: {} } });
    };
    const hub = createHub({ url: "http://hub", fetch: stub, timeouts: { hook: 100 } });
    await hub.callTool("whoami");
    const started = Date.now();
    await hub.close();
    expect(Date.now() - started).toBeLessThan(1500);
  });
});

describe("replies that are not a clean success", () => {
  const hubFor = () => createHub({ url: fake.url, token: "t" });

  it.each([
    ["a JSON reply with another id", (_m, body) => ({ ...body, id: body.id + 1000 })],
    ["a JSON reply with neither result nor error", (_m, body) => ({ jsonrpc: "2.0", id: body.id })],
  ])("rejects %s", async (_label, mangle) => {
    fake = await startFakeHub();
    const hub = hubFor();
    await hub.callTool("whoami");
    fake.mangle = (message, body) => (message.method === "tools/call" ? mangle(message, body) : body);
    await expect(hub.callTool("whoami")).rejects.toBeInstanceOf(HubError);
  });

  it("rejects an SSE stream whose reply has another id", async () => {
    fake = await startFakeHub({ sse: true });
    const hub = hubFor();
    await hub.callTool("whoami");
    fake.mangle = (message, body) => (message.method === "tools/call" ? { ...body, id: body.id + 1000 } : body);
    await expect(hub.callTool("whoami")).rejects.toBeInstanceOf(HubError);
  });

  it("rejects an initialize that is an error or carries no protocol version", async () => {
    fake = await startFakeHub();
    fake.mangle = (message, body) => (message.method === "initialize" ? { jsonrpc: "2.0", id: body.id, error: { code: -1, message: "no" } } : body);
    await expect(hubFor().callTool("whoami")).rejects.toBeInstanceOf(HubError);
    fake.mangle = (message, body) => (message.method === "initialize" ? { jsonrpc: "2.0", id: body.id, result: {} } : body);
    await expect(hubFor().callTool("whoami")).rejects.toBeInstanceOf(HubError);
  });

  it("sends the protocol version the server chose on every request after initialize", async () => {
    fake = await startFakeHub();
    fake.protocolVersion = "2025-03-26";
    const hub = hubFor();
    await hub.callTool("whoami");
    const posts = fake.requests.filter((r) => r.method === "POST");
    expect(posts).toHaveLength(3);
    expect(posts[0].headers["mcp-protocol-version"]).toBeUndefined();
    expect(posts[1].headers["mcp-protocol-version"]).toBe("2025-03-26");
    expect(posts[2].headers["mcp-protocol-version"]).toBe("2025-03-26");
  });
});

describe("the client identity", () => {
  it("reports the version in package.json", () => {
    const { version } = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    expect(CLIENT_INFO).toEqual({ name: "agent-hub-opencode", version });
  });
});

describe("a late 404", () => {
  it("does not reset an MCP session newer than the one that failed", async () => {
    let initializations = 0;
    let release;
    const gate = new Promise((resolve) => (release = resolve));
    const stub = async (_url, init) => {
      const body = init.body ? JSON.parse(init.body) : {};
      const sid = init.headers["Mcp-Session-Id"];
      if (body.method === "initialize") {
        initializations += 1;
        const result = { protocolVersion: "2025-06-18", capabilities: {} };
        return Response.json({ jsonrpc: "2.0", id: body.id, result }, { headers: { "mcp-session-id": `m${initializations}` } });
      }
      if (body.id === undefined) return new Response(null, { status: 202 });
      const name = body.params.name;
      if (sid === "m1" && name === "slow") {
        await gate;
        return new Response("{}", { status: 404 });
      }
      if (sid === "m1" && name === "fast") return new Response("{}", { status: 404 });
      return Response.json({ jsonrpc: "2.0", id: body.id, result: { content: [], structuredContent: { sid } } });
    };
    const hub = createHub({ url: "http://hub", fetch: stub });
    await hub.callTool("warm");
    const slow = hub.callTool("slow");
    await new Promise((r) => setTimeout(r, 20));
    expect((await hub.callTool("fast")).sid).toBe("m2");
    release();
    expect((await slow).sid).toBe("m2");
    expect(initializations).toBe(2);
  });
});

describe("a shared initialization", () => {
  it("lets a waiting caller abort on its own signal while another still completes", async () => {
    fake = await startFakeHub();
    fake.initDelay = 400;
    const hub = createHub({ url: fake.url, token: "t", agentName: "a" });
    const ctrl = new AbortController();
    const first = hub.callTool("whoami", {}, { signal: ctrl.signal });
    const second = hub.callTool("whoami", {});
    setTimeout(() => ctrl.abort(), 50);
    const started = Date.now();
    await expect(first).rejects.toBeDefined();
    expect(Date.now() - started).toBeLessThan(300);
    await expect(second).resolves.toBeDefined();
    expect(fake.initializations).toBe(1);
    await hub.close();
  });
});
