// One long-poll loop per plugin instance. It follows the most recently active
// root session, peeks without advancing anything, and drains only when the
// batch may wake an idle agent. Everything else stays pending on the hub.

import { notices } from "./format.mjs";

const IDLE_MS = 2000;
const QUICK_RETURN_MS = 500;
const BACKOFF_START_MS = 1000;
const BACKOFF_MAX_MS = 30_000;

export function sleep(ms, signal) {
  return new Promise((resolve) => {
    if (signal?.aborted) return resolve();
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      signal?.removeEventListener("abort", done);
      resolve();
    }
    signal?.addEventListener("abort", done, { once: true });
  });
}

export function createLoop({
  hub,
  wake = () => process.env.HUB_WAKE ?? null,
  clock = () => Date.now() / 1000,
  readState,
  deliver,
  signal,
  wait = 60,
  pause = sleep,
}) {
  let focus = null;
  let current = null;
  const pendingSeen = new Set();
  const peeked = new Set();

  const interrupt = () => current?.abort();

  function follow(key, hubSessionId) {
    if (focus?.key === key && focus.hubSessionId === hubSessionId) return;
    if (focus && focus.key !== key) peeked.delete(focus.key);
    focus = { key, hubSessionId, after: focus?.key === key ? focus.after : undefined };
    interrupt();
  }

  function unfollow(key) {
    pendingSeen.delete(key);
    peeked.delete(key);
    if (focus?.key !== key) return;
    focus = null;
    interrupt();
  }

  const hasPending = (key) => pendingSeen.has(key);

  // Pulls what is waiting for a session now, for the next prompt to deliver. When
  // the loop has not watched this session, one cheap peek says whether to drain.
  // A failed drain leaves the pending mark in place.
  async function takePending(key, hubSessionId) {
    const shared = AbortSignal.timeout(hub.timeouts.hook);
    if (!pendingSeen.has(key)) {
      if (peeked.has(key)) return [];
      const { count } = await hub.peek({ session: hubSessionId, wait: 0, signal: shared, deadline: "hook" });
      if (!count) return [];
    }
    const items = await hub.drain(hubSessionId, { signal: shared });
    pendingSeen.delete(key);
    return items;
  }

  async function step(target, ctrl) {
    const started = Date.now();
    const { items, cursor } = await hub.peek({ session: target.hubSessionId, wait, after: target.after, signal: ctrl.signal });
    peeked.add(target.key);
    if (!items.length) {
      if (cursor) target.after = cursor;
      if (Date.now() - started < QUICK_RETURN_MS) await pause(IDLE_MS, ctrl.signal);
      return;
    }
    const next = cursor || target.after;
    const state = readState(target.key);
    const decision = notices({ hub_wake: wake(), now: clock(), last_wake_at: state.last_wake_at ?? null, items });
    pendingSeen.add(target.key);
    if (!decision.wake) {
      target.after = next;
      return;
    }
    // A failed drain throws before the cursor moves, so the next peek sees the items again.
    const drained = await hub.drain(target.hubSessionId, { signal: ctrl.signal });
    target.after = next;
    pendingSeen.delete(target.key);
    if (!drained.length) return;
    const ordered = [...drained.filter((i) => i.direct), ...drained.filter((i) => !i.direct)];
    await deliver(target.key, ordered);
  }

  async function run() {
    let backoff = BACKOFF_START_MS;
    while (!signal.aborted) {
      const target = focus;
      const ctrl = new AbortController();
      current = ctrl;
      const onAbort = () => ctrl.abort();
      signal.addEventListener("abort", onAbort, { once: true });
      try {
        if (!target || !target.hubSessionId) {
          await pause(IDLE_MS, ctrl.signal);
        } else {
          await step(target, ctrl);
          backoff = BACKOFF_START_MS;
        }
      } catch {
        if (!ctrl.signal.aborted) {
          await pause(backoff, ctrl.signal);
          backoff = Math.min(backoff * 2, BACKOFF_MAX_MS);
        }
      } finally {
        signal.removeEventListener("abort", onAbort);
        if (current === ctrl) current = null;
      }
    }
  }

  return { run, follow, unfollow, hasPending, takePending, focused: () => focus?.key ?? null };
}
