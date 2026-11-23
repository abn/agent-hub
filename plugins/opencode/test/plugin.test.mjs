import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
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
  const toolHooks = {};
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
    transforms: 0,
    mcp: {
      async transform(callback) {
        ctx.transforms += 1;
        callback({ get: (name) => ctx.mcps[name], set: (name, config) => (ctx.mcps[name] = config) });
      },
    },
    tool: {
      async hook(name, callback) {
        toolHooks[name] = callback;
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
    toolHooks,
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
  if (typeof env === "function") env = env();
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

const created = (sessionID, parentID, agent) => ({
  type: "session.created",
  data: { sessionID, parentID, agent, location: { directory: scratch } },
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
  it("registers the remote MCP server with the token and the agent name, whatever an option says", async () => {
    const ctx = await boot({ ctxOptions: { agentName: "oc-work" } });
    expect(ctx.mcps["agent-hub"]).toEqual({
      type: "remote",
      url: `${fake.url}/mcp`,
      headers: { "X-Agent-Id": "opencode", Authorization: "Bearer tok" },
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
    expect(entry.value).toContain("project proj | session work (s_01TEST) | you are tester \u00b7 opencode");
    expect(entry.value).toContain('Pass session: {"project_id": "proj", "name": "work"} on hub calls (the plugin adds it).');
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

  it("gives a child session a hub session of its own under the parent's, and tells it so", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_df68c1b96", "ses_root", "general"));
    await until(() => ctx.entries.length === 2);
    expect(ctx.entries[1].sessionID).toBe("ses_df68c1b96");
    expect(ctx.entries[1].value).toBe(
      'You work in hub session work/general-sesdf68c (project proj). Hub calls carry it; pass session: {"project_id": "proj", "name": "work/general-sesdf68c"} if you call a hub tool directly.',
    );
    expect(fake.calls.filter((c) => c.name === "session_start").at(-1).args).toEqual({
      project_id: "proj",
      session_name: "work/general-sesdf68c",
      exclusive: true,
    });
    expect(fake.calls.filter((c) => c.name === "session_brief")).toHaveLength(1);
  });

  it("starts a child it mapped before under its own name, not a fresh one, even after a restart", async () => {
    const ctx = await boot({
      prepare: () => {
        const dir = join(scratch, "state", "agent-hub", "hooks", "opencode");
        mkdirSync(dir, { recursive: true });
        writeFileSync(join(dir, "ses_df68c1b96.json"), JSON.stringify({ child: { name: "work/general-earlier" } }));
      },
    });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_df68c1b96", "ses_root", "general"));
    await until(() => ctx.entries.length === 2);
    expect(fake.calls.filter((c) => c.name === "session_start").at(-1).args).toEqual({
      project_id: "proj",
      session_name: "work/general-earlier",
      exclusive: false,
    });
    expect(ctx.entries[1].value).toContain("work/general-earlier");
  });

  it("steps to the next name when the hub says the child's name is taken", async () => {
    const ctx = await boot();
    fake.sessionNames.add("work/general-sesdf68c");
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_df68c1b96", "ses_root", "general"));
    await until(() => ctx.entries.length === 2);
    expect(ctx.entries[1].value).toContain("work/general-sesdf68c-2");
    const starts = fake.calls.filter((c) => c.name === "session_start").slice(-2);
    expect(starts.map((c) => [c.args.session_name, c.args.exclusive])).toEqual([
      ["work/general-sesdf68c", true],
      ["work/general-sesdf68c-2", true],
    ]);
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
      expect(first[0]).toContain("project proj | session work (s_01TEST) | you are tester \u00b7 opencode");
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
      ctx.emit(created("ses_child", "ses_root", "general"));
      await new Promise((r) => setTimeout(r, 50));
      const texts = call(ctx, "ses_child");
      expect(texts).toHaveLength(1);
      expect(texts[0]).toContain("You work in hub session work/general-sesch");
      expect(call(ctx, "ses_child")).toEqual([]);
    });

    it("carries the block on the very first context call of a session it adopts there", async () => {
      const ctx = await boot({ prepare: without });
      ctx.sessions.ses_new = { id: "ses_new", location: { directory: scratch } };
      const hook = { sessionID: "ses_new", system: [] };
      await ctx.hooks.context(hook);
      expect(hook.system).toHaveLength(1);
      expect(hook.system[0].text).toContain("project proj | session work (s_01TEST) | you are tester \u00b7 opencode");
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

  it("names itself opencode on the MCP entry and on every call of its own", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(ctx.mcps["agent-hub"].headers["X-Agent-Id"]).toBe("opencode");
    expect(fake.requests.length).toBeGreaterThan(0);
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

  it("leaves the hub session, and never ends it, when a root session is deleted", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_leave") === 1);
    expect(fake.calls.find((c) => c.name === "session_leave").args).toEqual({ session: { project_id: "proj", name: "work" } });
    await new Promise((r) => setTimeout(r, 50));
    expect(fake.count("session_end")).toBe(0);
  });

  it("leaves a child's own session when the child is deleted, and the parent's stays", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_df68c1b96", "ses_root", "general"));
    await until(() => ctx.entries.length === 2);
    ctx.emit(event("session.deleted", "ses_df68c1b96"));
    await until(() => fake.count("session_leave") === 1);
    expect(fake.left).toEqual([{ project_id: "proj", name: "work/general-sesdf68c" }]);
  });

  it("keeps a leave the hub did not take and retries it on a later event, clearing the child's mapping only then", async () => {
    let clock = 1_000_000;
    const ctx = await boot({ plugin: { now: () => clock } });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_df68c1b96", "ses_root", "general"));
    await until(() => ctx.entries.length === 2);
    const file = join(scratch, "state", "agent-hub", "hooks", "opencode");
    expect(JSON.parse(readFileSync(join(file, "ses_df68c1b96.json"), "utf8")).child.name).toBe("work/general-sesdf68c");

    fake.failTools.session_leave = 1;
    ctx.emit(event("session.deleted", "ses_df68c1b96"));
    await until(() => fake.count("session_leave") === 1);
    await new Promise((r) => setTimeout(r, 60));
    expect(fake.left).toEqual([]);
    const kept = JSON.parse(readFileSync(join(file, "leaves.json"), "utf8"));
    expect(kept.pending).toEqual([
      { project_id: "proj", name: "work/general-sesdf68c", key: "ses_df68c1b96", attempts: 1, tried_at: clock },
    ]);
    expect(JSON.stringify(kept)).not.toContain("tok");
    expect(JSON.parse(readFileSync(join(file, "ses_df68c1b96.json"), "utf8")).child.name).toBe("work/general-sesdf68c");

    // Too soon: another event does not call again.
    ctx.emit(event("session.execution.started", "ses_root"));
    await new Promise((r) => setTimeout(r, 60));
    expect(fake.count("session_leave")).toBe(1);

    clock += 6000;
    ctx.emit(event("session.execution.started", "ses_root"));
    await until(() => fake.left.length === 1);
    expect(fake.left).toEqual([{ project_id: "proj", name: "work/general-sesdf68c" }]);
    await until(() => JSON.parse(readFileSync(join(file, "leaves.json"), "utf8")).pending.length === 0);
    expect(JSON.parse(readFileSync(join(file, "ses_df68c1b96.json"), "utf8")).child).toBeNull();
  });

  it("gives up on a leave after a bounded number of attempts", async () => {
    let clock = 1_000_000;
    const ctx = await boot({ plugin: { now: () => clock } });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.failTools.session_leave = 100;
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_leave") === 1);
    for (let n = 2; n <= 8; n += 1) {
      clock += 6000;
      await new Promise((r) => setTimeout(r, 40));
      ctx.emit(event("session.execution.started", "ses_other"));
      await new Promise((r) => setTimeout(r, 40));
    }
    expect(fake.count("session_leave")).toBe(5);
  });

  it("leaves nothing for a child that never got a session of its own", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.failTools.session_start = 1;
    ctx.emit(created("ses_child", "ses_root", "general"));
    await until(() => fake.count("session_start") === 2);
    ctx.emit(event("session.deleted", "ses_child"));
    await new Promise((r) => setTimeout(r, 80));
    expect(fake.count("session_leave")).toBe(0);
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

  it("drops a session deleted during its start and still leaves the hub session", async () => {
    const ctx = await boot();
    fake.delayTools.session_start = 200;
    ctx.emit(created("ses_root"));
    await until(() => fake.count("session_start") === 1);
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_leave") === 1);
    expect(fake.calls.find((c) => c.name === "session_leave").args).toEqual({ session: { project_id: "proj", name: "work" } });
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
    await until(() => fake.count("session_leave") === 1);
    await until(() => fake.peeks.slice(mark).some((p) => p.session === "s_B"));
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

  it("settles a session_leave before it closes the MCP session on dispose", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.delayTools.session_leave = 150;
    ctx.emit(event("session.deleted", "ses_root"));
    await until(() => fake.count("session_leave") === 1);
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

describe("hub calls the model makes", () => {
  // What the host fires for an MCP tool: the call on `<server>_<tool>`, and a
  // codemode `execute` wrapper whose input is code.
  const call = (ctx, sessionID, tool, input) => {
    const event = { tool, sessionID, agent: "build", messageID: "msg_1", id: "call_1", input };
    return Promise.resolve(ctx.toolHooks["execute.before"](event)).then(() => event);
  };

  it("adds the session to a call that names none", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    const event = await call(ctx, "ses_root", "agent-hub_feed_read", { project_id: "proj" });
    expect(event.input).toEqual({ project_id: "proj", session: { project_id: "proj", name: "work" } });
  });

  it("leaves the codemode wrapper, a call that names a session and a session_start alone", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect((await call(ctx, "ses_root", "execute", { code: "return await tools.agent_hub.whoami({})" })).input).toEqual({
      code: "return await tools.agent_hub.whoami({})",
    });
    const named = { project_id: "proj", session: { name: "other", project_id: "proj" } };
    expect((await call(ctx, "ses_root", "agent-hub_feed_read", named)).input).toEqual(named);
    const start = { project_id: "proj", session_name: "work" };
    expect((await call(ctx, "ses_root", "agent-hub_session_start", start)).input).toEqual(start);
    expect((await call(ctx, "ses_root", "other_server_tool", { a: 1 })).input).toEqual({ a: 1 });
  });

  it("gives a subagent's call its own child session, and an unstarted session none", async () => {
    const ctx = await boot();
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    ctx.emit(created("ses_df68c1b96", "ses_root", "general"));
    await until(() => ctx.entries.length === 2);
    expect((await call(ctx, "ses_df68c1b96", "agent-hub_whoami", {})).input.session).toEqual({
      project_id: "proj",
      name: "work/general-sesdf68c",
    });
    expect((await call(ctx, "ses_root", "agent-hub_whoami", {})).input.session).toEqual({ project_id: "proj", name: "work" });
    expect((await call(ctx, "ses_unknown", "agent-hub_whoami", {})).input).toEqual({});
  });

  it("puts no session on a subagent's call whose own start failed, never the parent's, and gives it its child once a start succeeds", async () => {
    let clock = 1_000_000;
    const ctx = await boot({ plugin: { now: () => clock } });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    fake.failTools.session_start = 1;
    ctx.emit(created("ses_child", "ses_root", "general"));
    await until(() => fake.count("session_start") === 2);
    expect((await call(ctx, "ses_child", "agent-hub_whoami", {})).input).toEqual({});
    expect((await call(ctx, "ses_root", "agent-hub_whoami", {})).input.session).toEqual({ project_id: "proj", name: "work" });

    clock += 61_000;
    expect((await call(ctx, "ses_child", "agent-hub_whoami", {})).input.session).toEqual({
      project_id: "proj",
      name: "work/general-seschild",
    });
  });

  it("starts the hub's own calls from a plugin with the hook API missing without failing", async () => {
    const ctx = await boot({ prepare: (c) => delete c.tool });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(ctx.toolHooks).toEqual({});
  });
});

// Storing a credential syncs the file and its directory, which a busy disk makes slow.
describe("enrolment and the states before a member exists", { timeout: 30_000 }, () => {
  const sessionWith = async (ctx) => {
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
  };
  const dirs = () => () => ({ HUB_CONFIG: join(scratch, "config.toml"), HUB_TOKEN: undefined });
  const pendingBody = (over = {}) => ({
    status: "pending",
    member: "laptop",
    token: "pending-token",
    status_url: `${fake.url}/api/v1/enrol/status`,
    ...over,
  });
  const enrol = (ctx, body) =>
    ctx.toolHooks["execute.after"]({ tool: "agent-hub_enrol", sessionID: "ses_root", input: {}, output: { structuredContent: body } });

  it("says a member waits for approval instead of starting a session", async () => {
    const ctx = await boot({ env: { HUB_TOKEN: "pending-token" } });
    fake.whoami = { state: "pending", member: "laptop", hub_url: fake.url };
    await sessionWith(ctx);
    expect(ctx.entries[0].value).toBe("<agent-hub>\nWaiting for approval as laptop.\n</agent-hub>");
    expect(fake.count("session_start")).toBe(0);
  });

  it("says a caller the hub does not know is not a member yet", async () => {
    const ctx = await boot();
    fake.whoami = { state: "anonymous", hub_url: fake.url, enrol: true };
    await sessionWith(ctx);
    expect(ctx.entries[0].value).toBe("<agent-hub>\nNot a member of this hub yet: call enrol.\n</agent-hub>");
  });

  it("makes no hub call when several members hold credentials and nothing picks one", async () => {
    mkdirSync(scratchRoot, { recursive: true });
    fake = await startFakeHub();
    scratch = mkdtempSync(join(scratchRoot, "opencode-plugin-"));
    const config = join(scratch, "config.toml");
    const entry = (member) => `[[credential]]\nurl = "${fake.url}"\nmember = "${member}"\ntoken = "t-${member}"\n`;
    writeFileSync(config, `${entry("a")}\n${entry("b")}`);
    const ctx = fakeCtx();
    dispose = await createPlugin({ env: { HUB_CONFIG: config, HUB_URL: fake.url, XDG_STATE_HOME: scratch, HOME: scratch } }).setup(ctx);
    expect(ctx.mcps["agent-hub"].headers).toEqual({ "X-Agent-Id": "opencode" });
    ctx.emit(created("ses_root"));
    await until(() => ctx.entries.length === 1);
    expect(ctx.entries[0].value).toBe(`<agent-hub>\nSeveral members for ${fake.url}; set member in .agent-hub.toml or HUB_MEMBER.\n</agent-hub>`);
    expect(fake.calls).toEqual([]);
  });

  it("stores the pending token the moment enrol returns it, and keeps the status url", async () => {
    const ctx = await boot({ env: dirs() });
    await enrol(ctx, pendingBody());
    const text = readFileSync(join(scratch, "config.toml"), "utf8");
    expect(text).toContain(`url = "${fake.url}"`);
    expect(text).toContain('member = "laptop"');
    expect(text).toContain('token = "pending-token"');
    expect(statSync(join(scratch, "config.toml")).mode & 0o777).toBe(0o600);
    const state = JSON.parse(readFileSync(join(scratch, "state", "agent-hub", "hooks", "opencode", "enrol.json"), "utf8"));
    expect(state.pending).toEqual({ url: fake.url, member: "laptop", status_url: `${fake.url}/api/v1/enrol/status` });
    expect(JSON.stringify(state)).not.toContain("pending-token");
  });

  it("reads the enrol result from a text part as well as a structured one", async () => {
    const ctx = await boot({ env: dirs() });
    await ctx.toolHooks["execute.after"]({
      tool: "agent-hub_enrol",
      sessionID: "ses_root",
      input: {},
      output: { content: [{ type: "text", text: JSON.stringify(pendingBody()) }] },
    });
    expect(readFileSync(join(scratch, "config.toml"), "utf8")).toContain('token = "pending-token"');
  });

  it("ignores a result that is not a pending enrolment, and a call to another tool", async () => {
    const ctx = await boot({ env: dirs() });
    await enrol(ctx, { status: "approved" });
    await ctx.toolHooks["execute.after"]({ tool: "agent-hub_whoami", sessionID: "ses_root", input: {}, output: pendingBody() });
    expect(existsSync(join(scratch, "config.toml"))).toBe(false);
  });

  it("follows the status on a prompt, renames the stored member to the approved name and reconnects with the token", async () => {
    const ctx = await boot({ env: dirs(), plugin: { now: () => 5_000_000 } });
    await sessionWith(ctx);
    await enrol(ctx, pendingBody());
    expect(ctx.mcps["agent-hub"].headers).toEqual({ "X-Agent-Id": "opencode" });
    expect(ctx.transforms).toBe(1);
    fake.enrolStatus = { status: "pending", member: "laptop" };
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.statusRequests).toHaveLength(1);
    expect(fake.statusRequests[0].search).toBe("?wait=0");
    expect(fake.statusRequests[0].headers.authorization).toBe("Bearer pending-token");
    expect(ctx.transforms).toBe(1);
    fake.enrolStatus = { status: "approved", member: "laptop-2", share: null };
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    // within the gap: not asked again
    expect(fake.statusRequests).toHaveLength(1);
  });

  it("asks again once the gap has passed, and registers the MCP entry with the token on approval", async () => {
    let clock = 5_000_000;
    const ctx = await boot({ env: dirs(), plugin: { now: () => clock } });
    await sessionWith(ctx);
    await enrol(ctx, pendingBody());
    fake.enrolStatus = { status: "approved", member: "laptop-2", share: null };
    clock += 31_000;
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.statusRequests).toHaveLength(1);
    const text = readFileSync(join(scratch, "config.toml"), "utf8");
    expect(text).toContain('member = "laptop-2"');
    expect(text).not.toContain('member = "laptop"\n');
    expect(ctx.transforms).toBe(2);
    expect(ctx.mcps["agent-hub"].headers).toEqual({ "X-Agent-Id": "opencode", Authorization: "Bearer pending-token" });
    clock += 31_000;
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.statusRequests).toHaveLength(1);
  });

  it("stops following a refused enrolment", async () => {
    let clock = 5_000_000;
    const ctx = await boot({ env: dirs(), plugin: { now: () => clock } });
    await sessionWith(ctx);
    await enrol(ctx, pendingBody());
    fake.enrolStatus = null;
    clock += 31_000;
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    clock += 31_000;
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.statusRequests).toHaveLength(1);
    expect(ctx.transforms).toBe(1);
  });

  it("never sends the pending token to a status url on another host", async () => {
    let clock = 5_000_000;
    const ctx = await boot({ env: dirs(), plugin: { now: () => clock } });
    await sessionWith(ctx);
    await enrol(ctx, pendingBody({ status_url: "http://elsewhere.invalid/api/v1/enrol/status" }));
    clock += 31_000;
    await ctx.hooks.prompt({ sessionID: "ses_root" });
    expect(fake.statusRequests).toEqual([]);
  });
});
