// The OpenCode v2 plugin: a default export of { id, setup }. The factory takes
// its collaborators so a test can run the plugin against a fake hub.

import { keepInstruction, startSession, subagentText } from "./bundle.mjs";
import { resolveConfig } from "./config.mjs";
import { noticeLine, WAKE_COOLDOWN_SECONDS } from "./format.mjs";
import { createHub } from "./hub.mjs";
import { createLoop, sleep } from "./loop.mjs";
import { readState, stateDir, writeState } from "./state.mjs";

const MCP_NAME = "agent-hub";
const ENTRY_KEY = "agent-hub";

const EVENT_TYPES = [
  "session.created",
  "session.deleted",
  "session.execution.started",
  "session.execution.succeeded",
  "session.execution.failed",
  "session.execution.interrupted",
  "session.idle",
  "session.compaction.ended",
];

const pendingText = (lines) => `<agent-hub>\nPending:\n${lines.join("\n")}\n</agent-hub>`;

const RETRY_START_MS = 60_000;
const ADOPT_WAIT_MS = 3000;

export function createPlugin({
  env = process.env,
  fetch: fetchImpl = globalThis.fetch,
  pause = sleep,
  wait = 60,
  now = () => Date.now(),
  timeouts,
} = {}) {
  return {
    id: "agent-hub",
    /** @param {import("@opencode/plugin").Plugin.Context} ctx */
    async setup(ctx) {
      const config = resolveConfig({ options: ctx.options ?? {}, env });
      if (!config.url) return undefined;

      const dir = stateDir(env);
      const hub = createHub({ ...config, fetch: fetchImpl, timeouts });
      const controller = new AbortController();
      const sessions = new Map();
      const wakes = new Map();

      // Every host API beyond the context hook is optional: a host that lacks one
      // loses that feature and the plugin still follows the session.
      if (typeof ctx.mcp?.transform === "function") {
        await ctx.mcp.transform((editor) => {
          if (editor.get(MCP_NAME)) return;
          const headers = { "X-Agent-Id": config.agentName };
          if (config.token) headers.Authorization = `Bearer ${config.token}`;
          editor.set(MCP_NAME, { type: "remote", url: `${config.url}/mcp`, headers });
        });
      }

      // The stable text goes into the session's instructions when the host has
      // them. Without them, or when the put is refused, it waits on the session
      // and the context hook injects it on the next call.
      async function putEntry(session, value) {
        const put = ctx.session?.instructions?.entry?.put;
        if (typeof put === "function") {
          try {
            await put.call(ctx.session.instructions.entry, { sessionID: session.key, key: ENTRY_KEY, value });
            session.stable = null;
            return;
          } catch {
            // falls back to the context hook below
          }
        }
        session.stable = value;
      }

      // The cooldown lives in the state file and in memory, so an unwritable state
      // directory still holds it for this process.
      const lastWake = (key) => {
        const stored = readState(dir, key).last_wake_at;
        const latest = Math.max(typeof stored === "number" ? stored : 0, wakes.get(key) ?? 0);
        return latest || null;
      };
      const chargeWake = (key) => {
        const at = Math.floor(now() / 1000);
        wakes.set(key, at);
        writeState(dir, key, { last_wake_at: at });
      };

      // The lines leave the queue for the prompt and return to its front if the
      // prompt fails, so the context hook still delivers them.
      // One wake prompt is in flight per session, and a second wake inside the
      // cooldown waits in the queue for the context hook.
      async function flush(session) {
        if (!session.queue.length || session.busy || session.waking || typeof ctx.session?.prompt !== "function") return;
        const last = lastWake(session.key);
        if (last !== null && Math.floor(now() / 1000) - last < WAKE_COOLDOWN_SECONDS) return;
        const lines = session.queue.splice(0);
        session.waking = true;
        try {
          await ctx.session.prompt({ sessionID: session.key, text: pendingText(lines) });
          chargeWake(session.key);
        } catch {
          session.queue.unshift(...lines);
        } finally {
          session.waking = false;
        }
      }

      async function deliver(key, items) {
        const session = sessions.get(key);
        if (!session) return;
        session.queue.push(...items.map(noticeLine));
        void flush(session);
      }

      const loop = createLoop({
        hub,
        wake: () => env.HUB_WAKE ?? null,
        clock: () => now() / 1000,
        readState: (key) => ({ last_wake_at: lastWake(key) }),
        deliver,
        signal: controller.signal,
        wait,
        pause,
      });

      const alive = (session) => !session.dead && !controller.signal.aborted && sessions.get(session.key) === session;

      async function begin(session) {
        if (session.parentID) {
          const parent = sessions.get(session.parentID);
          await parent?.running?.catch(() => {});
          if (alive(session)) await putEntry(session, subagentText(parent?.info?.hubSessionId));
          return;
        }
        let info;
        try {
          info = await startSession({
            hub,
            cwd: session.directory,
            harnessSessionId: session.key,
            env,
            trailers: session.startup,
            signal: controller.signal,
          });
        } catch {
          session.failedAt = now();
          return;
        }
        session.failedAt = null;
        session.info = info;
        if (!alive(session)) return;
        writeState(dir, session.key, {
          project_id: info.projectId,
          session_name: info.sessionName,
          session_id: info.hubSessionId,
        });
        await putEntry(session, info.block);
        if (alive(session) && info.hubSessionId) loop.follow(session.key, info.hubSessionId);
      }

      const work = new Set();
      const track = (promise) => {
        work.add(promise);
        promise.finally(() => work.delete(promise)).catch(() => {});
        return promise;
      };

      function launch(session) {
        session.running = begin(session).finally(() => {
          session.running = null;
        });
        track(session.running);
      }

      function createSession(key, parentID, directory) {
        const session = {
          key,
          parentID: parentID ?? null,
          directory,
          queue: [],
          startup: [],
          stable: null,
          busy: false,
          waking: false,
          info: null,
          dead: false,
          failedAt: null,
          running: null,
          lastActive: now(),
        };
        sessions.set(key, session);
        return session;
      }

      function open(sessionID, parentID, directory) {
        if (sessions.has(sessionID)) return;
        launch(createSession(sessionID, parentID, directory ?? ctx.location?.directory ?? process.cwd()));
      }

      // A session that was open before the plugin loaded never raised created.
      function adopt(sessionID) {
        if (sessions.has(sessionID)) return undefined;
        const session = createSession(sessionID, null, ctx.location?.directory ?? process.cwd());
        session.running = (async () => {
          let found;
          try {
            found = await ctx.session.get({ sessionID });
          } catch {
            if (sessions.get(sessionID) === session) sessions.delete(sessionID);
            return;
          }
          if (!alive(session)) return;
          session.parentID = found?.parentID ?? null;
          session.directory = found?.location?.directory ?? session.directory;
          await begin(session);
        })().finally(() => {
          session.running = null;
        });
        track(session.running);
        return session;
      }

      function refocus() {
        if (loop.focused()) return;
        const next = [...sessions.values()]
          .filter((session) => !session.parentID && session.info?.hubSessionId && alive(session))
          .sort((a, b) => b.lastActive - a.lastActive)[0];
        if (next) loop.follow(next.key, next.info.hubSessionId);
      }

      // Activity on a root session follows it, and retries a start that failed.
      function touch(session) {
        session.lastActive = now();
        if (session.parentID) return;
        if (session.info?.hubSessionId) {
          loop.follow(session.key, session.info.hubSessionId);
        } else if (!session.info && !session.running && session.failedAt !== null && now() - session.failedAt >= RETRY_START_MS) {
          launch(session);
        }
      }

      async function finish(session) {
        await session.running?.catch(() => {});
        if (session.parentID || !session.info?.hubSessionId) return;
        await hub.callTool("session_end", { session_id: session.info.hubSessionId }, { deadline: "hook" }).catch(() => {});
      }

      const hook = async (name, callback) => {
        if (typeof ctx.session?.hook === "function") await ctx.session.hook(name, callback);
      };

      function inject(session, event) {
        touch(session);
        const seen = new Set();
        const startup = session.startup.splice(0).filter((item) => !seen.has(item.id) && seen.add(item.id));
        if (session.stable) {
          event.system.push({ type: "text", text: session.stable });
          session.stable = null;
        }
        const lines = [...startup.map(noticeLine), ...session.queue.splice(0)];
        if (lines.length) event.system.push({ type: "text", text: pendingText(lines) });
      }

      // A context call for a session the plugin has not met means a turn is
      // already running, so the session starts busy. The call waits a moment for
      // the start, so the first model call of the session carries the block.
      await hook("context", (event) => {
        const session = sessions.get(event.sessionID);
        if (session) return inject(session, event);
        const adopted = adopt(event.sessionID);
        if (!adopted) return undefined;
        adopted.busy = true;
        let timer;
        const limit = new Promise((resolve) => {
          timer = setTimeout(resolve, ADOPT_WAIT_MS);
        });
        return Promise.race([adopted.running?.catch(() => {}), limit]).then(() => {
          clearTimeout(timer);
          if (alive(adopted)) inject(adopted, event);
        });
      });

      await hook("prompt", async (event) => {
        const session = sessions.get(event.sessionID);
        if (!session) return;
        touch(session);
        if (!session.info?.hubSessionId) return;
        try {
          const items = await loop.takePending(session.key, session.info.hubSessionId);
          session.queue.push(...items.map(noticeLine));
        } catch {
          // the items stay pending on the hub for the next prompt
        }
      });

      await hook("compaction", (event) => {
        const session = sessions.get(event.sessionID);
        if (session?.info) event.system.push({ type: "text", text: keepInstruction(session.info) });
      });

      async function onEvent(event) {
        const data = event.data ?? {};
        const session = data.sessionID ? sessions.get(data.sessionID) : undefined;
        switch (event.type) {
          case "session.created":
            open(data.sessionID, data.parentID ?? null, data.location?.directory);
            break;
          case "session.execution.started":
            if (session) {
              session.busy = true;
              touch(session);
            }
            break;
          case "session.execution.succeeded":
          case "session.execution.failed":
          case "session.execution.interrupted":
          case "session.idle":
            if (session) {
              session.busy = false;
              void flush(session);
            }
            break;
          case "session.compaction.ended":
            if (session?.info) await putEntry(session, session.info.block);
            break;
          case "session.deleted":
            if (!session) break;
            session.dead = true;
            sessions.delete(session.key);
            loop.unfollow(session.key);
            refocus();
            track(finish(session));
            break;
          default:
        }
      }

      const events = (async () => {
        while (!controller.signal.aborted && typeof ctx.event?.subscribe === "function") {
          try {
            for await (const event of ctx.event.subscribe({ types: EVENT_TYPES, signal: controller.signal })) {
              await onEvent(event).catch(() => {});
            }
          } catch {
            // a dropped stream is reopened below
          }
          if (!controller.signal.aborted) await pause(1000, controller.signal);
        }
      })();

      const running = loop.run();

      // Settles the loop, the event stream, every start and every session_end
      // before the MCP session closes, so no call follows the DELETE.
      return async () => {
        controller.abort();
        await Promise.allSettled([events, running]);
        while (work.size) await Promise.allSettled([...work]);
        await hub.close();
      };
    },
  };
}
