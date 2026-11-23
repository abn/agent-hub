import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import index from "../index.mjs";
import { createPlugin } from "../plugin.mjs";
import { startFakeHub } from "./fake-hub.mjs";

const scratchRoot = fileURLToPath(new URL("../../../target/tmp/", import.meta.url));

let fake;
let scratch;
let dispose;
afterEach(async () => {
  await dispose?.();
  dispose = undefined;
  await fake?.stop();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
  scratch = undefined;
});

const until = async (check, ms = 3000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (check()) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error("timed out waiting for the condition");
};

// The slice of the OpenCode v2 plugin context the plugin touches.
function fakeCtx(options = {}) {
  const hooks = {};
  const feed = [];
  const waiters = [];
  const ctx = {
    options,
    location: { directory: scratch },
    mcps: {},
    entries: [],
    prompts: [],
    promptFailures: 0,
    promptCalls: 0,
    promptHold: null,
    sessions: {},
    mcp: {
      async transform(callback) {
        callback({ get: (name) => ctx.mcps[name], set: (name, config) => (ctx.mcps[name] = config) });
      },
    },
    session: {
      async hook(name, callback) {
        hooks[name] = callback;
      },
      async get({ sessionID }) {
        return ctx.sessions[sessionID];
      },
      async prompt(input) {
        ctx.promptCalls += 1;
        if (ctx.promptHold) await ctx.promptHold;
        if (ctx.promptFailures > 0) {
          ctx.promptFailures -= 1;
          throw new Error("prompt refused");
        }
        ctx.prompts.push(input);
        return {};
      },
      instructions: {
        entry: {
          async put(input) {
            ctx.entries.push(input);
          },
        },
      },
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
    hooks,
    emit(event) {
      feed.push(event);
      waiters.splice(0).forEach((resolve) => resolve());
    },
  };
  return ctx;
}

async function boot({ ctxOptions, env = {}, sse = false, blockedState = false, plugin: pluginOptions = {}, prepare } = {}) {
  fake = await startFakeHub({ sse, pollMs: 40 });
  mkdirSync(scratchRoot, { recursive: true });
  scratch = mkdtempSync(join(scratchRoot, "opencode-plugin-"));
  const ctx = fakeCtx(ctxOptions ?? {});
  prepare?.(ctx);
  if (blockedState) {
    writeFileSync(join(scratch, "blocker"), "a file, not a directory");
    env = { ...env, XDG_STATE_HOME: join(scratch, "blocker") };
  }
  const plugin = createPlugin({
    env: {
      HUB_URL: fake.url,
      HUB_TOKEN: "tok",
      HUB_PROJECT: "proj",
      HUB_SESSION: "work",
      XDG_STATE_HOME: join(scratch, "state"),
      HOME: scratch,
      ...env,
    },
    ...pluginOptions,
    pause: (ms, signal) => new Promise((r) => {
      const t = setTimeout(r, Math.min(ms, 20));
      signal?.addEventListener("abort", () => (clearTimeout(t), r()), { once: true });
    }),
  });
  dispose = await plugin.setup(ctx);
  return ctx;
}

const created = (sessionID, parentID) => ({
  type: "session.created",
  data: { sessionID, parentID, location: { directory: scratch } },
});
const event = (type, sessionID) => ({ type, data: { sessionID } });
const item = (id, direct) => ({ id, kind: direct ? "answer" : "signal", title: `t ${id}`, project_id: "proj", direct });

describe("plugin shape", () => {
  it("default-exports an object with an id and a setup function", () => {
    expect(index.id).toBe("agent-hub");
    expect(typeof index.setup).toBe("function");
  });

  it("does nothing without a hub url", async () => {
    scratch = mkdtempSync(join((mkdirSync(scratchRoot, { recursive: true }), scratchRoot), "opencode-plugin-"));
    const ctx = fakeCtx();
    const cleanup = await createPlugin({ env: { HOME: scratch, AGENT_HUB_SYSTEM_CONFIG: join(scratch, "none.toml") } }).setup(ctx);
    expect(cleanup).toBeUndefined();
    expect(ctx.mcps).toEqual({});
    expect(ctx.hooks).toEqual({});
  });
});

describe("the plugin against a fake hub", () => {
  it("registers the remote MCP server with the token and agent name", async () => {
    const ctx = await boot({ ctxOptions: { agentName: "oc-work" } });
    expect(ctx.mcps["agent-hub"]).toEqual({
      type: "remote",
      url: `${fake.url}/mcp`,
      headers: { "X-Agent-Id": "oc-work", Authorization: "Bearer tok" },
    });
  });

  it("sends no Authorization when the url comes from options and the token from the environment", async () => {
    fake = await startFakeHub();
    mkdirSync(scratchRoot, { recursive: true });
    scratch = mkdtempSync(join(scratchRoot, "opencode-plugin-"));
    const ctx = fakeCtx({ url: fake.url });
    const env = { HUB_TOKEN: "env-token", HUB_PROJECT: "proj", HUB_SESSION: "work", XDG_STATE_HOME: scratch, HOME: scratch, HUB_CONFIG: join(scratch, "none.toml") };
    dispose = await createPlugin({ env }).setup(ctx);
    expect(ctx.mcps["agent-hub"].headers).toEqual({ "X-Agent-Id": "opencode" });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(fake.requests.length).toBeGreaterThan(0);
    for (const request of fake.requests) expect(request.headers.authorization).toBeUndefined();
  });

  it("sends no Authorization when the token file is outside the config directory", async () => {
    fake = await startFakeHub();
    mkdirSync(scratchRoot, { recursive: true });
    scratch = mkdtempSync(join(scratchRoot, "opencode-plugin-"));
    const stolen = join(scratch, "id_key");
    writeFileSync(stolen, "secret-key");
    const ctx = fakeCtx({ url: fake.url, tokenFile: stolen });
    const env = { HUB_PROJECT: "proj", HUB_SESSION: "work", XDG_STATE_HOME: scratch, HOME: scratch, HUB_CONFIG: join(scratch, "none.toml") };
    const warn = console.warn;
    console.warn = () => {};
    try {
      dispose = await createPlugin({ env }).setup(ctx);
    } finally {
      console.warn = warn;
    }
    expect(ctx.mcps["agent-hub"].headers).toEqual({ "X-Agent-Id": "opencode" });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    for (const request of fake.requests) expect(request.headers.authorization).toBeUndefined();
  });

  it("keeps an MCP entry the user already configured", async () => {
    fake = await startFakeHub();
    const ctx = fakeCtx();
    ctx.mcps["agent-hub"] = { type: "remote", url: "http://own/mcp" };
    mkdirSync(scratchRoot, { recursive: true });
    scratch = mkdtempSync(join(scratchRoot, "opencode-plugin-"));
    dispose = await createPlugin({ env: { HUB_URL: fake.url, XDG_STATE_HOME: scratch, HOME: scratch } }).setup(ctx);
    expect(ctx.mcps["agent-hub"].url).toBe("http://own/mcp");
  });

  it.each([false, true])("binds a root session and puts the block in its instructions (sse: %s)", async (sse) => {
    const ctx = await boot({ sse });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    const [entry] = ctx.entries;
    expect(entry.sessionID).toBe("ses_root");
    expect(entry.key).toBe("agent-hub");
    expect(entry.value).toContain("<agent-hub>");
    expect(entry.value).toContain("project proj | session work (s_01TEST) | you are tester/opencode");
    expect(entry.value).toContain('session_start(project_id: "proj", session_name: "work")');
    expect(entry.value).toContain("KB index:\n# Index");
    expect(fake.calls.map((c) => c.name)).toEqual(
      expect.arrayContaining(["whoami", "session_brief", "session_start", "brain_get", "notify_subscribe"]),
    );
    const subscribed = fake.calls.filter((c) => c.name === "notify_subscribe").map((c) => c.args.kinds);
    expect(subscribed).toEqual(["comment", "page"]);
    expect(fake.calls.find((c) => c.name === "notify_subscribe").args).toMatchObject({ about_self: true, session: { session_id: "s_01TEST" } });
    const state = JSON.parse(readFileSync(join(scratch, "state", "agent-hub", "hooks", "opencode", "ses_root.json"), "utf8"));
    expect(state).toMatchObject({ project_id: "proj", session_name: "work", session_id: "s_01TEST" });
  });

  it("delivers items found while starting through the context hook and keeps them out of the entry", async () => {
    const ctx = await boot();
    fake.trailerFor.session_start = [item("evt_8", false)];
    fake.pending.push(item("evt_9", true));
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(ctx.entries[0].value).not.toContain("Pending:");
    expect(ctx.entries[0].value).not.toContain("evt_8");
    expect(ctx.entries[0].value).not.toContain("evt_9");
    await new Promise((r) => setTimeout(r, 30));
    expect(ctx.prompts).toEqual([]);
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("- [signal] t evt_8 (proj, evt_8)");
    expect(hook.system[0].text).toContain("- [answer] t evt_9 (proj, evt_9)");
    ctx.emit(event("session.compaction.ended", "ses_root"));
    await until(() => ctx.entries.length === 2);
    expect(ctx.entries[1].value).toBe(ctx.entries[0].value);
  });

  it("gives a child session the subagent instruction and no bundle", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    const calls = fake.calls.length;
    ctx.emit(created("ses_child", "ses_root"));
    await until(() => ctx.entries.length === 2);
    expect(ctx.entries[1].sessionID).toBe("ses_child");
    expect(ctx.entries[1].value).toContain("Do not call session_start");
    expect(ctx.entries[1].value).toContain("s_01TEST");
    expect(fake.calls.length).toBe(calls);
  });

  it("appends the keep instruction on compaction and restores the entry after", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    const hook = { sessionID: "ses_root", system: [] };
    await ctx.hooks.compaction(hook);
    expect(hook.system).toHaveLength(1);
    expect(hook.system[0].text).toBe("Keep: hub project proj, session work (id s_01TEST), the current phase and open hub items.");
    ctx.emit(event("session.compaction.ended", "ses_root"));
    await until(() => ctx.entries.length === 2);
    expect(ctx.entries[1].value).toBe(ctx.entries[0].value);
  });

  describe("on a host without the instructions API", () => {
    const without = (ctx) => {
      delete ctx.session.instructions;
    };
    const call = (ctx, sessionID = "ses_root") => {
      const hook = { sessionID, system: [] };
      ctx.hooks.context(hook);
      return hook.system.map((part) => part.text);
    };
    const settled = async (ctx) => {
      await until(() => fake.count("notify_subscribe") === 2);
      await new Promise((r) => setTimeout(r, 30));
      return ctx;
    };

    it("injects the block through the context hook on the first call and not on the next", async () => {
      const ctx = await boot({ prepare: without });
      ctx.emit(created("ses_root"));
      await settled(ctx);
      const first = call(ctx);
      expect(first).toHaveLength(1);
      expect(first[0]).toContain("project proj | session work (s_01TEST) | you are tester/opencode");
      expect(first[0]).toContain("KB index:\n# Index");
      expect(call(ctx)).toEqual([]);
    });

    it("injects the block again after compaction", async () => {
      const ctx = await boot({ prepare: without });
      ctx.emit(created("ses_root"));
      await settled(ctx);
      const first = call(ctx);
      expect(call(ctx)).toEqual([]);
      ctx.emit(event("session.compaction.ended", "ses_root"));
      await new Promise((r) => setTimeout(r, 30));
      const again = call(ctx);
      expect(again).toEqual(first);
      expect(call(ctx)).toEqual([]);
    });

    it("still follows the session, so a mid-turn item reaches the context hook", async () => {
      const ctx = await boot({ prepare: without });
      ctx.emit(created("ses_root"));
      await settled(ctx);
      call(ctx);
      ctx.emit(event("session.execution.started", "ses_root"));
      const base = fake.count("notify_wait");
      fake.pending.push(item("evt_1", true));
      await until(() => fake.count("notify_wait") > base);
      await new Promise((r) => setTimeout(r, 30));
      expect(ctx.prompts).toEqual([]);
      const texts = call(ctx);
      expect(texts).toHaveLength(1);
      expect(texts[0]).toContain("- [answer] t evt_1 (proj, evt_1)");
    });

    it("gives a child session the subagent instruction through the context hook", async () => {
      const ctx = await boot({ prepare: without });
      ctx.emit(created("ses_root"));
      await settled(ctx);
      ctx.emit(created("ses_child", "ses_root"));
      await new Promise((r) => setTimeout(r, 50));
      const texts = call(ctx, "ses_child");
      expect(texts).toHaveLength(1);
      expect(texts[0]).toContain("Do not call session_start");
      expect(call(ctx, "ses_child")).toEqual([]);
    });

    it("carries the block on the very first context call of a session it adopts there", async () => {
      const ctx = await boot({ prepare: without });
      ctx.sessions.ses_new = { id: "ses_new", location: { directory: scratch } };
      const hook = { sessionID: "ses_new", system: [] };
      await ctx.hooks.context(hook);
      expect(hook.system).toHaveLength(1);
      expect(hook.system[0].text).toContain("project proj | session work (s_01TEST) | you are tester/opencode");
    });

    it("treats a session adopted on a context call as busy, so an item waits for the context hook", async () => {
      const ctx = await boot({ prepare: without });
      ctx.sessions.ses_new = { id: "ses_new", location: { directory: scratch } };
      await ctx.hooks.context({ sessionID: "ses_new", system: [] });
      const base = fake.count("notify_wait");
      fake.pending.push(item("evt_1", true));
      await until(() => fake.count("notify_wait") > base);
      await new Promise((r) => setTimeout(r, 30));
      expect(ctx.prompts).toEqual([]);
      const texts = call(ctx, "ses_new");
      expect(texts[0]).toContain("- [answer] t evt_1 (proj, evt_1)");
    });

    it("falls back to the context hook when the entry put is refused", async () => {
      const ctx = await boot({
        prepare(c) {
          c.session.instructions.entry.put = () => Promise.reject(new Error("refused"));
        },
      });
      ctx.emit(created("ses_root"));
      await settled(ctx);
      expect(call(ctx)[0]).toContain("<agent-hub>");
    });

    it("starts without the optional prompt and mcp APIs and still follows", async () => {
      const ctx = await boot({
        prepare(c) {
          without(c);
          delete c.session.prompt;
          delete c.mcp;
        },
      });
      ctx.emit(created("ses_root"));
      await settled(ctx);
      expect(call(ctx)[0]).toContain("<agent-hub>");
      fake.pending.push(item("evt_1", true));
      await until(() => fake.count("notify_wait") >= 1);
      await new Promise((r) => setTimeout(r, 30));
      expect(call(ctx)[0]).toContain("- [answer] t evt_1 (proj, evt_1)");
    });
  });

  it("uses one agent name for the MCP entry and for the plugin's own hub calls", async () => {
    const ctx = await boot({ ctxOptions: { agentName: "oc-work" } });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(ctx.mcps["agent-hub"].headers["X-Agent-Id"]).toBe("oc-work");
    expect(fake.requests.length).toBeGreaterThan(0);
    for (const request of fake.requests) expect(request.headers["x-agent-id"]).toBe("oc-work");
  });

  it("uses the default agent name for both when none is set", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(ctx.mcps["agent-hub"].headers["X-Agent-Id"]).toBe("opencode");
    for (const request of fake.requests) expect(request.headers["x-agent-id"]).toBe("opencode");
  });

  it("wakes an idle session with a prompt on a direct item", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.pending.push(item("evt_1", true));
    await until(() => ctx.prompts.length === 1);
    expect(ctx.prompts[0].sessionID).toBe("ses_root");
    expect(ctx.prompts[0].text).toContain("- [answer] t evt_1 (proj, evt_1)");
  });

  it("queues for the context hook while the session is busy, with no network call", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(event("session.execution.started", "ses_root"));
    await new Promise((r) => setTimeout(r, 30));
    const base = fake.count("notify_wait");
    fake.pending.push(item("evt_1", true));
    await until(() => fake.count("notify_wait") === base + 1);
    await new Promise((r) => setTimeout(r, 30));
    expect(ctx.prompts).toEqual([]);

    const before = fake.requests.length;
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("- [answer] t evt_1 (proj, evt_1)");
    expect(fake.requests.length).toBe(before);

    const again = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(again);
    expect(again.system).toEqual([]);
  });

  it("prompts with what is still queued when the session goes idle", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(event("session.execution.started", "ses_root"));
    await new Promise((r) => setTimeout(r, 30));
    const base = fake.count("notify_wait");
    fake.pending.push(item("evt_1", true));
    await until(() => fake.count("notify_wait") === base + 1);
    ctx.emit(event("session.execution.succeeded", "ses_root"));
    await until(() => ctx.prompts.length === 1);
  });

  it("delivers an ambient item at the next prompt and not before", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    const base = fake.count("notify_wait");
    fake.pending.push(item("evt_1", false));
    await new Promise((r) => setTimeout(r, 200));
    expect(fake.count("notify_wait")).toBe(base);
    expect(ctx.prompts).toEqual([]);

    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.count("notify_wait")).toBe(base + 1);
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("- [signal] t evt_1 (proj, evt_1)");
  });

  it("makes no hub call from the prompt hook when nothing is known to be pending", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    const before = fake.calls.length;
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.calls.length).toBe(before);
  });

  it("adopts a session that was open before the plugin loaded", async () => {
    const ctx = await boot();
    ctx.sessions.ses_old = { id: "ses_old", location: { directory: scratch } };
    ctx.hooks.context({ sessionID: "ses_old", system: [] });
    await until(() => ctx.entries.length === 1);
    expect(ctx.entries[0].sessionID).toBe("ses_old");
  });

  it("ends the hub session when a root session is deleted", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_end") === 1);
    expect(fake.calls.find((c) => c.name === "session_end").args).toEqual({ session_id: "s_01TEST" });
  });

  it("stops its loop and closes its MCP session on dispose", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => fake.peeks.length > 0);
    await dispose();
    dispose = undefined;
    await until(() => fake.mcpSessions.size === 0);
    const seen = fake.peeks.length;
    await new Promise((r) => setTimeout(r, 150));
    expect(fake.peeks.length).toBe(seen);
  });

  it("returns from the prompt hook within the hook deadline when the hub stalls", async () => {
    const ctx = await boot({ plugin: { timeouts: { hook: 150, startup: 2000, peekSlack: 150 } } });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.pending.push(item("evt_1", false));
    await until(() => fake.peeks.some((p) => p.after === "cur:evt_1"));
    fake.stallTools.add("notify_wait");
    const started = Date.now();
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(Date.now() - started).toBeLessThan(1500);
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system).toEqual([]);
  });

  it("keeps the lines for the context hook when the wake prompt is refused", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.promptFailures = 1;
    fake.pending.push(item("evt_1", true));
    await until(() => fake.count("notify_wait") >= 1 && ctx.promptFailures === 0);
    expect(ctx.prompts).toEqual([]);
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("- [answer] t evt_1 (proj, evt_1)");
  });

  it("does not count a busy session's drain as a wake", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(event("session.execution.started", "ses_root"));
    await new Promise((r) => setTimeout(r, 30));
    const base = fake.count("notify_wait");
    fake.pending.push(item("evt_1", true));
    await until(() => fake.count("notify_wait") === base + 1);
    fake.pending.push(item("evt_2", true));
    await until(() => fake.count("notify_wait") === base + 2);
    const state = JSON.parse(readFileSync(join(scratch, "state", "agent-hub", "hooks", "opencode", "ses_root.json"), "utf8"));
    expect(state.last_wake_at).toBeUndefined();
    expect(ctx.prompts).toEqual([]);
  });

  it("records the wake when the prompt goes through", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.pending.push(item("evt_1", true));
    await until(() => ctx.prompts.length === 1);
    const file = join(scratch, "state", "agent-hub", "hooks", "opencode", "ses_root.json");
    await until(() => existsSync(file) && JSON.parse(readFileSync(file, "utf8")).last_wake_at > 0);
  });

  it("holds the cooldown in memory when the state directory is unwritable", async () => {
    const ctx = await boot({ blockedState: true });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.pending.push(item("evt_1", true));
    await until(() => ctx.prompts.length === 1);
    const base = fake.count("notify_wait");
    fake.pending.push(item("evt_2", true));
    await until(() => fake.peeks.some((p) => p.after === "cur:evt_2"));
    await new Promise((r) => setTimeout(r, 100));
    expect(fake.count("notify_wait")).toBe(base);
    expect(ctx.prompts).toHaveLength(1);
  });

  it("drops a session deleted during its start and still ends the hub session", async () => {
    const ctx = await boot();
    fake.delayTools.session_start = 200;
    ctx.emit(created("ses_root"));
    await until(() => fake.count("session_start") === 1);
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_end") === 1);
    expect(fake.calls.find((c) => c.name === "session_end").args).toEqual({ session_id: "s_01TEST" });
    await new Promise((r) => setTimeout(r, 100));
    expect(ctx.entries).toEqual([]);
    expect(fake.peeks.filter((p) => p.session === "s_01TEST")).toEqual([]);
  });

  it("follows the most recently active remaining root when the focused one is deleted", async () => {
    const ctx = await boot();
    fake.sessionIds.push("s_A", "s_B");
    ctx.emit(created("ses_a"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_b"));
    await until(() => ctx.entries.length === 2);
    ctx.emit(event("session.execution.started", "ses_a"));
    await until(() => fake.peeks.some((p) => p.session === "s_A"));
    const mark = fake.peeks.length;
    ctx.emit(event("session.deleted", "ses_a"));
    await until(() => fake.count("session_end") === 1);
    await until(() => fake.peeks.slice(mark).some((p) => p.session === "s_B"));
    expect(fake.calls.find((c) => c.name === "session_end").args).toEqual({ session_id: "s_A" });
  });

  it("retries a failed start on later activity, at most once a minute", async () => {
    let clock = 1_000_000;
    const ctx = await boot({ plugin: { now: () => clock } });
    fake.failTools.session_start = 1;
    ctx.emit(created("ses_root"));
    await until(() => fake.count("session_start") === 1);
    await new Promise((r) => setTimeout(r, 100));
    ctx.hooks.context({ sessionID: "ses_root", system: [] });
    await new Promise((r) => setTimeout(r, 50));
    expect(fake.count("session_start")).toBe(1);
    expect(ctx.entries).toEqual([]);

    clock += 61_000;
    ctx.hooks.context({ sessionID: "ses_root", system: [] });
    await until(() => ctx.entries.length === 1);
    expect(fake.count("session_start")).toBe(2);
    await until(() => fake.peeks.some((p) => p.session === "s_01TEST"));
  });

  it("peeks once from the prompt hook for a session the loop does not watch", async () => {
    const ctx = await boot();
    fake.sessionIds.push("s_A", "s_B");
    ctx.emit(created("ses_a"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_b"));
    await until(() => ctx.entries.length === 2);
    ctx.emit(event("session.execution.started", "ses_b"));
    await until(() => fake.peeks.some((p) => p.session === "s_B"));
    fake.pending.push(item("evt_1", false));
    const marks = fake.peeks.length;
    await ctx.hooks.prompt({ sessionID: "ses_a" });
    expect(fake.peeks.slice(marks).some((p) => p.session === "s_A" && p.wait === "0")).toBe(true);
    const hook = { sessionID: "ses_a", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("evt_1");
  });

  it("keeps one wake prompt in flight per session and queues a second wake for the context hook", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    let release;
    ctx.promptHold = new Promise((resolve) => (release = resolve));
    fake.pending.push(item("evt_1", true));
    await until(() => ctx.promptCalls === 1);
    fake.pending.push(item("evt_2", true));
    await until(() => fake.count("notify_wait") >= 2);
    await new Promise((r) => setTimeout(r, 60));
    expect(ctx.promptCalls).toBe(1);
    release();
    await until(() => ctx.prompts.length === 1);
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("evt_2");
    expect(hook.system[0].text).not.toContain("evt_1");
  });

  it("rechecks the cooldown before an idle flush and leaves the lines for the context hook", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(event("session.execution.started", "ses_root"));
    await new Promise((r) => setTimeout(r, 30));
    const base = fake.count("notify_wait");
    fake.pending.push(item("evt_1", true));
    await until(() => fake.count("notify_wait") > base);
    await new Promise((r) => setTimeout(r, 30));
    const file = join(scratch, "state", "agent-hub", "hooks", "opencode", "ses_root.json");
    const state = JSON.parse(readFileSync(file, "utf8"));
    writeFileSync(file, JSON.stringify({ ...state, last_wake_at: Math.floor(Date.now() / 1000) }));
    ctx.emit(event("session.idle", "ses_root"));
    await new Promise((r) => setTimeout(r, 60));
    expect(ctx.promptCalls).toBe(0);
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("evt_1");
  });

  it("keeps items reported while starting when the start fails, and delivers them once", async () => {
    const ctx = await boot();
    fake.trailerFor.whoami = [item("evt_t", false)];
    fake.failTools.session_start = 1;
    ctx.emit(created("ses_root"));
    await until(() => fake.count("session_start") === 1);
    await new Promise((r) => setTimeout(r, 60));
    const hook = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(hook);
    expect(hook.system[0].text).toContain("evt_t");
    const again = { sessionID: "ses_root", system: [] };
    ctx.hooks.context(again);
    expect(again.system).toEqual([]);
  });

  it("settles a session_end before it closes the MCP session on dispose", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.delayTools.session_end = 150;
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_end") === 1);
    await dispose();
    dispose = undefined;
    const last = fake.requests.at(-1);
    expect(last.method).toBe("DELETE");
    expect(fake.requests.filter((r) => r.method === "DELETE")).toHaveLength(1);
    expect(fake.mcpSessions.size).toBe(0);
  });

  it("settles a start in progress before it closes, with no call after the DELETE", async () => {
    const ctx = await boot();
    fake.delayTools.session_start = 150;
    ctx.emit(created("ses_root"));
    await until(() => fake.count("session_start") === 1);
    await dispose();
    dispose = undefined;
    const lastDelete = fake.requests.map((r) => r.method).lastIndexOf("DELETE");
    expect(lastDelete === -1 || lastDelete === fake.requests.length - 1).toBe(true);
  });
});
