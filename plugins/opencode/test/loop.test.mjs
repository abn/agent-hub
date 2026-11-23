import { afterEach, describe, expect, it } from "vitest";
import { createHub } from "../hub.mjs";
import { createLoop } from "../loop.mjs";
import { startFakeHub } from "./fake-hub.mjs";

let fake;
let controller;
afterEach(async () => {
  controller?.abort();
  await fake?.stop();
});

const until = async (check, ms = 2000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (check()) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error("timed out waiting for the condition");
};

async function setup({ wake = "direct", clock = () => 1_000_000, deliverFails = false } = {}) {
  fake = await startFakeHub({ pollMs: 40 });
  controller = new AbortController();
  const hub = createHub({ url: fake.url, token: "t" });
  const store = {};
  const delivered = [];
  const loop = createLoop({
    hub,
    wake: () => wake,
    clock,
    readState: (key) => store[key] ?? {},
    deliver: async (key, items) => {
      delivered.push({ key, ids: items.map((i) => i.id) });
      if (deliverFails) throw new Error("delivery failed");
      store[key] = { ...store[key], last_wake_at: Math.floor(clock()) };
    },
    signal: controller.signal,
    wait: 60,
    pause: (ms, signal) => new Promise((r) => {
      const t = setTimeout(r, Math.min(ms, 20));
      signal?.addEventListener("abort", () => (clearTimeout(t), r()), { once: true });
    }),
  });
  const running = loop.run();
  return { loop, store, delivered, running };
}

const item = (id, direct, kind = direct ? "answer" : "signal") => ({ id, kind, title: `t ${id}`, project_id: "proj", direct });

describe("the long-poll loop", () => {
  it("peeks the followed session and drains once on a direct item", async () => {
    const { loop, store, delivered } = await setup();
    loop.follow("ses_1", "s_01");
    await until(() => fake.peeks.length > 0);
    expect(fake.peeks[0]).toEqual({ session: "s_01", wait: "60", after: null });
    fake.pending.push(item("evt_1", true));
    await until(() => delivered.length === 1);
    expect(delivered[0]).toEqual({ key: "ses_1", ids: ["evt_1"] });
    expect(fake.count("notify_wait")).toBe(1);
    expect(store.ses_1.last_wake_at).toBe(1_000_000);
  });

  it("delivers direct items first when the drain carries both", async () => {
    const { loop, delivered } = await setup();
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", false), item("evt_2", true));
    await until(() => delivered.length === 1);
    expect(delivered[0].ids).toEqual(["evt_2", "evt_1"]);
  });

  it("leaves an ambient item pending and never drains it", async () => {
    const { loop, delivered } = await setup();
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", false));
    await until(() => loop.hasPending("ses_1"));
    await new Promise((r) => setTimeout(r, 150));
    expect(fake.count("notify_wait")).toBe(0);
    expect(delivered).toEqual([]);
    expect(fake.pending).toHaveLength(1);
    expect(fake.peeks.at(-1).after).toBe("cur:evt_1");
  });

  it("wakes on any item when HUB_WAKE is all", async () => {
    const { loop, delivered } = await setup({ wake: "all" });
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", false));
    await until(() => delivered.length === 1);
  });

  it("never wakes when HUB_WAKE is never", async () => {
    const { loop, delivered } = await setup({ wake: "never" });
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", true));
    await until(() => loop.hasPending("ses_1"));
    expect(fake.count("notify_wait")).toBe(0);
    expect(delivered).toEqual([]);
  });

  it("holds a second wake inside the 300 second cooldown", async () => {
    let now = 1_000_000;
    const { loop, delivered } = await setup({ clock: () => now });
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", true));
    await until(() => delivered.length === 1);
    now += 299;
    fake.pending.push(item("evt_2", true));
    await until(() => loop.hasPending("ses_1"));
    expect(fake.count("notify_wait")).toBe(1);
    expect(delivered).toHaveLength(1);
  });

  it("wakes again once the cooldown has passed", async () => {
    let now = 1_000_000;
    const { loop, delivered } = await setup({ clock: () => now });
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", true));
    await until(() => delivered.length === 1);
    now += 300;
    fake.pending.push(item("evt_2", true));
    await until(() => delivered.length === 2);
  });

  it("moves its peek to a newly followed session", async () => {
    const { loop } = await setup();
    loop.follow("ses_1", "s_01");
    await until(() => fake.peeks.some((p) => p.session === "s_01"));
    loop.follow("ses_2", "s_02");
    await until(() => fake.peeks.some((p) => p.session === "s_02"));
  });

  it("stops polling when aborted", async () => {
    const { loop, running } = await setup();
    loop.follow("ses_1", "s_01");
    await until(() => fake.peeks.length > 0);
    controller.abort();
    await running;
    const seen = fake.peeks.length;
    await new Promise((r) => setTimeout(r, 150));
    expect(fake.peeks.length).toBe(seen);
  });

  it("keeps polling after a failed peek and still delivers", async () => {
    const { loop, delivered } = await setup();
    fake.failPeeks = 2;
    fake.pending.push(item("evt_1", true));
    loop.follow("ses_1", "s_01");
    await until(() => delivered.length === 1);
    expect(fake.peeks.length).toBeGreaterThanOrEqual(3);
  });

  it("keeps the pending mark and the cursor when the drain fails, then drains on the retry", async () => {
    const { loop, delivered } = await setup();
    fake.failTools.notify_wait = 1;
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", true));
    await until(() => fake.count("notify_wait") >= 1);
    expect(loop.hasPending("ses_1")).toBe(true);
    await until(() => delivered.length === 1);
    expect(delivered[0].ids).toEqual(["evt_1"]);
    expect(fake.peeks.slice(0, 2).map((p) => p.after)).toEqual([null, null]);
    expect(loop.hasPending("ses_1")).toBe(false);
  });

  it("peeks once for a session it has not watched, and drains only if something is pending", async () => {
    const { loop } = await setup();
    expect(await loop.takePending("ses_other", "s_02")).toEqual([]);
    expect(fake.peeks).toEqual([{ session: "s_02", wait: "0", after: null }]);
    expect(fake.count("notify_wait")).toBe(0);
    fake.pending.push(item("evt_1", false));
    const items = await loop.takePending("ses_other", "s_02");
    expect(items.map((i) => i.id)).toEqual(["evt_1"]);
    expect(fake.count("notify_wait")).toBe(1);
  });

  it("keeps the pending mark when the prompt-time drain fails", async () => {
    const { loop } = await setup();
    loop.follow("ses_1", "s_01");
    fake.pending.push(item("evt_1", false));
    await until(() => loop.hasPending("ses_1"));
    fake.failTools.notify_wait = 1;
    await expect(loop.takePending("ses_1", "s_01")).rejects.toThrow();
    expect(loop.hasPending("ses_1")).toBe(true);
    expect((await loop.takePending("ses_1", "s_01")).map((i) => i.id)).toEqual(["evt_1"]);
    expect(loop.hasPending("ses_1")).toBe(false);
  });

  it("sends the cursor of the previous reply, not an item id, so a held peek is not answered at once", async () => {
    fake = await startFakeHub({ pollMs: 200 });
    controller = new AbortController();
    const hub = createHub({ url: fake.url, token: "t" });
    const loop = createLoop({
      hub,
      wake: () => "never",
      clock: () => 1_000_000,
      readState: () => ({}),
      deliver: async () => {},
      signal: controller.signal,
      wait: 60,
      pause: (ms, signal) => new Promise((r) => {
        const t = setTimeout(r, Math.min(ms, 20));
        signal?.addEventListener("abort", () => (clearTimeout(t), r()), { once: true });
      }),
    });
    const running = loop.run();
    fake.pending.push(item("evt_ans", false));
    loop.follow("ses_1", "s_01");
    await until(() => fake.peeks.length >= 2);
    expect(fake.peeks[1].after).toBe("cur:evt_ans");
    await new Promise((r) => setTimeout(r, 500));
    expect(fake.peeks.length).toBeLessThanOrEqual(5);
    controller.abort();
    await running;
  });
});
