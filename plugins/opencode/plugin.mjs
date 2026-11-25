// The OpenCode v2 plugin: a default export of { id, setup }. The factory takes
// its collaborators so a test can run the plugin against a fake hub.

import { keepInstruction, startSession, subagentText } from "./bundle.mjs";
import { resume, startNew } from "./child.mjs";
import { refusal, resolveConfig } from "./config.mjs";
import { readConfigFile, rekey, writeCredential } from "./credential.mjs";
import { noticeBlock, noticeLine, WAKE_COOLDOWN_SECONDS } from "./format.mjs";
import { AGENT, createHub } from "./hub.mjs";
import { createLoop, sleep } from "./loop.mjs";
import { gatherGit } from "./resolve.mjs";
import { readState, stateDir, writeState } from "./state.mjs";

const MCP_NAME = "agent-hub";
const ENTRY_KEY = "agent-hub";
// MCP tools reach the model as <server>_<tool>. The hook also fires for the
// codemode `execute` wrapper around them, which carries code and not a call.
const INNER_TOOL = /^agent[-_]hub_(.+)$/u;
const ENROL_KEY = "enrol";
const LEAVES_KEY = "leaves";
const STATUS_GAP_MS = 30_000;
// A leave that failed is tried again on a later event, at most this often and
// this many times in all.
const LEAVE_GAP_MS = 5_000;
const LEAVE_ATTEMPTS = 5;

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

// The body of an `enrol` result, whichever way the host hands it over: the
// structured object, or a text part holding the JSON.
function enrolBody(value, depth = 0) {
  if (value == null || depth > 3) return null;
  if (typeof value === "string") {
    try {
      return enrolBody(JSON.parse(value), depth + 1);
    } catch {
      return null;
    }
  }
  if (typeof value !== "object") return null;
  if (value.status === "pending" && value.token && value.member && value.status_url) return value;
  const parts = Array.isArray(value.content) ? value.content.map((part) => part?.text) : [];
  for (const next of [value.structuredContent, value.output, value.result, ...parts]) {
    const found = enrolBody(next, depth + 1);
    if (found) return found;
  }
  return null;
}

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
      const directory = ctx.location?.directory ?? process.cwd();
      const config = resolveConfig({ options: ctx.options ?? {}, env, topLevel: gatherGit(directory).top_level });
      if (!config.url) return undefined;
      // A resolver state that rules out connecting: the block says why and no
      // hub call is made.
      const refused = refusal(config);

      const dir = stateDir(env);
      const hub = createHub({ url: config.url, token: config.token, fetch: fetchImpl, timeouts });
      const controller = new AbortController();
      const sessions = new Map();
      const wakes = new Map();

      // The MCP entry sends a token only when the resolver found one it may use.
      // Registering it again, with the entry replaced, reconnects with a new
      // initialize, which is how an approved member's token takes effect.
      async function registerMcp(replace) {
        if (typeof ctx.mcp?.transform !== "function") return;
        await ctx.mcp.transform((editor) => {
          if (editor.get(MCP_NAME) && !replace) return;
          const headers = { "X-Agent-Id": AGENT };
          if (config.state === "ok" && config.token) headers.Authorization = `Bearer ${config.token}`;
          editor.set(MCP_NAME, { type: "remote", url: `${config.url}/mcp`, headers });
        });
      }

      // Every host API beyond the context hook is optional: a host that lacks one
      // loses that feature and the plugin still follows the session.
      await registerMcp(false);

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
        session.queue.push(...items.map((item) => noticeLine(item, hub.url)));
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

      // The session ref a hub call carries for this OpenCode session: the child
      // session of a subagent, else the root's own. A subagent with no confirmed
      // child session has none: its calls name no session and the hub records the
      // member and the agent only, never the parent's session.
      function refOf(session) {
        if (session.parentID) {
          return session.child ? { project_id: session.child.projectId, name: session.child.name } : null;
        }
        return session.info ? { project_id: session.info.projectId, name: session.info.sessionName } : null;
      }

      // A subagent gets its own hub session under the parent's. The hub picks the
      // name: an exclusive start that finds one held is refused and the next is
      // tried. Without a started parent the subagent works under none.
      async function beginChild(session) {
        const parent = sessions.get(session.parentID);
        await parent?.running?.catch(() => {});
        const info = parent?.info;
        if (!alive(session)) return;
        if (!info?.hubSessionId) {
          session.failedAt = now();
          return;
        }
        const start = (name, exclusive) =>
          hub.callTool("session_start", { project_id: info.projectId, session_name: name, exclusive }, { deadline: "startup", signal: controller.signal });
        // A child the host reopens, here or after a restart, takes its own name again.
        const known = session.child ?? readState(dir, session.key).child ?? null;
        try {
          const child = known
            ? await resume({ start, name: known.name })
            : await startNew({ start, parent: info.sessionName, agentType: session.agent, agentId: session.key });
          session.child = { name: child.name, projectId: info.projectId, sessionId: child.sessionId };
          writeState(dir, session.key, { child: { name: child.name } });
        } catch {
          session.failedAt = now();
          return;
        }
        session.failedAt = null;
        if (alive(session)) await putEntry(session, subagentText(info.projectId, session.child.name));
      }

      async function begin(session) {
        if (session.parentID) return beginChild(session);
        if (refused) {
          await putEntry(session, noticeBlock(refused));
          return;
        }
        let started;
        try {
          started = await startSession({
            hub,
            cwd: session.directory,
            env,
            now: now(),
            trailers: session.startup,
            notices: config.notices,
            signal: controller.signal,
          });
        } catch {
          session.failedAt = now();
          return;
        }
        if (started.notice) {
          session.failedAt = now();
          if (alive(session)) await putEntry(session, noticeBlock(started.notice));
          return;
        }
        const info = started;
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

      function createSession(key, parentID, directory, agent = null) {
        const session = {
          key,
          parentID: parentID ?? null,
          agent,
          child: null,
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

      function open(sessionID, parentID, directory, agent) {
        if (sessions.has(sessionID)) return;
        launch(createSession(sessionID, parentID, directory ?? ctx.location?.directory ?? process.cwd(), agent));
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
          session.agent = found?.agent ?? null;
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

      // A subagent whose own start failed tries again, at most once a minute.
      function retryChild(session) {
        if (session.child || session.running || session.failedAt === null || now() - session.failedAt < RETRY_START_MS) return;
        launch(session);
      }

      // Activity on a root session follows it, and retries a start that failed.
      function touch(session) {
        session.lastActive = now();
        if (session.parentID) {
          retryChild(session);
          return;
        }
        if (session.info?.hubSessionId) {
          loop.follow(session.key, session.info.hubSessionId);
        } else if (!session.info && !session.running && session.failedAt !== null && now() - session.failedAt >= RETRY_START_MS) {
          launch(session);
        }
      }

      // The hub session is left, never ended: it stays live for whoever else is
      // in it, and ending it is the work being done for everyone. A leave the hub
      // did not take is kept in the plugin state, with no token, and tried again on
      // a later event; a child's mapping goes only when its leave has succeeded.
      const stored = readState(dir, LEAVES_KEY).pending;
      let leaves = Array.isArray(stored)
        ? stored.filter((entry) => entry && typeof entry.project_id === "string" && typeof entry.name === "string")
        : [];
      let queued = false;
      let passes = Promise.resolve();
      const saveLeaves = () => writeState(dir, LEAVES_KEY, { pending: leaves });

      async function pass() {
        queued = false;
        for (const entry of [...leaves]) {
          if (entry.attempts && now() - (entry.tried_at ?? 0) < LEAVE_GAP_MS) continue;
          try {
            await hub.callTool("session_leave", { session: { project_id: entry.project_id, name: entry.name } }, { deadline: "hook" });
          } catch {
            entry.attempts = (entry.attempts ?? 0) + 1;
            entry.tried_at = now();
            if (entry.attempts >= LEAVE_ATTEMPTS) leaves = leaves.filter((e) => e !== entry);
            saveLeaves();
            // A hub that does not answer is not asked again for each entry.
            break;
          }
          leaves = leaves.filter((e) => e !== entry);
          saveLeaves();
          if (entry.key) writeState(dir, entry.key, { child: null });
        }
      }

      // Passes run one after another, so a leave added while one is running is
      // taken by the next.
      const settleLeaves = () => {
        queued = true;
        passes = passes.then(pass, pass);
        return passes;
      };

      const retryLeaves = () => {
        if (leaves.length && !queued) track(settleLeaves());
      };

      async function finish(session) {
        await session.running?.catch(() => {});
        const ref = refOf(session);
        if (!ref) return;
        leaves.push({ project_id: ref.project_id, name: ref.name, key: session.parentID ? session.key : null, attempts: 0 });
        saveLeaves();
        await settleLeaves();
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
        const lines = [...startup.map((item) => noticeLine(item, hub.url)), ...session.queue.splice(0)];
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
        retryLeaves();
        await pollEnrolment();
        const session = sessions.get(event.sessionID);
        if (!session) return;
        touch(session);
        if (!session.info?.hubSessionId) return;
        try {
          const items = await loop.takePending(session.key, session.info.hubSessionId);
          session.queue.push(...items.map((item) => noticeLine(item, hub.url)));
        } catch {
          // the items stay pending on the hub for the next prompt
        }
      });

      await hook("compaction", (event) => {
        const session = sessions.get(event.sessionID);
        if (session?.info) event.system.push({ type: "text", text: keepInstruction(session.info) });
      });

      // Hub calls the model makes through MCP carry the session without the model
      // naming it: the session of the OpenCode session making the call, and a
      // subagent's own child session. Only the inner call is changed; the codemode
      // wrapper around it holds code. A call that names a session, or starts one,
      // is left alone.
      const toolHook = async (name, callback) => {
        if (typeof ctx.tool?.hook === "function") await ctx.tool.hook(name, callback);
      };

      await toolHook("execute.before", async (event) => {
        const inner = INNER_TOOL.exec(event.tool ?? "");
        const input = event.input;
        if (!inner || inner[1] === "session_start") return;
        if (!input || typeof input !== "object" || Array.isArray(input) || input.session != null) return;
        const session = sessions.get(event.sessionID);
        if (!session) return;
        if (session.parentID) retryChild(session);
        // The first call of a session may come before its start has finished.
        let timer;
        await Promise.race([
          session.running?.catch(() => {}),
          new Promise((resolve) => {
            timer = setTimeout(resolve, ADOPT_WAIT_MS);
          }),
        ]);
        clearTimeout(timer);
        const ref = refOf(session);
        if (ref) input.session = ref;
      });

      // An enrolment that is waiting for the operator: the token is stored the
      // moment the hub hands it over, and its status is followed until the
      // member is approved.
      let pending = null;
      let polling = false;
      const remembered = readState(dir, ENROL_KEY).pending;
      if (remembered?.url && remembered.member && remembered.status_url) {
        const stored = readConfigFile(config.configPath).credentials.find(
          (c) => c.url === remembered.url && c.member === remembered.member,
        );
        if (stored?.token) pending = { ...remembered, token: stored.token, checkedAt: 0 };
      }

      function adoptEnrolment(body) {
        if (!body) return;
        try {
          writeCredential(config.configPath, config.url, body.member, body.token);
        } catch {
          return;
        }
        pending = { url: config.url, member: body.member, status_url: body.status_url, token: body.token, checkedAt: 0 };
        writeState(dir, ENROL_KEY, { pending: { url: config.url, member: body.member, status_url: body.status_url } });
        hub.setToken(body.token);
        for (const session of sessions.values()) {
          if (!session.parentID && !session.info && !session.running) launch(session);
        }
      }

      await toolHook("execute.after", async (event) => {
        const inner = INNER_TOOL.exec(event.tool ?? "");
        if (inner?.[1] !== "enrol") return;
        adoptEnrolment(enrolBody(event.output) ?? enrolBody(event.result));
      });

      // Looked at on a prompt, at most every 30 seconds. An approval under
      // another name renames the stored credential, and the MCP entry is
      // registered again with the token, which reconnects the model's tools.
      async function pollEnrolment() {
        if (!pending || polling || now() - pending.checkedAt < STATUS_GAP_MS) return;
        polling = true;
        pending.checkedAt = now();
        try {
          const target = new URL(pending.status_url);
          if (target.origin !== new URL(config.url).origin) return;
          target.searchParams.set("wait", "0");
          const res = await fetchImpl(target, {
            headers: { Authorization: `Bearer ${pending.token}`, "X-Agent-Id": AGENT, Accept: "application/json" },
            signal: AbortSignal.timeout(hub.timeouts.hook),
          });
          if (res.status === 401) {
            // Refused or revoked: nothing more to wait for.
            pending = null;
            writeState(dir, ENROL_KEY, { pending: null });
            return;
          }
          if (!res.ok) return;
          const body = await res.json();
          if (body.status !== "approved") return;
          const approved = typeof body.member === "string" && body.member ? body.member : pending.member;
          if (approved !== pending.member) {
            try {
              rekey(config.configPath, pending.url, pending.member, approved);
            } catch {
              // the token still works under the old name
            }
          }
          config.member = approved;
          config.token = pending.token;
          config.state = "ok";
          pending = null;
          writeState(dir, ENROL_KEY, { pending: null });
          hub.setToken(config.token);
          await registerMcp(true);
          for (const session of sessions.values()) {
            if (!session.parentID && !session.info && !session.running) launch(session);
          }
        } catch {
          // looked at again on a later prompt
        } finally {
          polling = false;
        }
      }

      async function onEvent(event) {
        retryLeaves();
        const data = event.data ?? {};
        const session = data.sessionID ? sessions.get(data.sessionID) : undefined;
        switch (event.type) {
          case "session.created":
            open(data.sessionID, data.parentID ?? null, data.location?.directory, data.agent);
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
