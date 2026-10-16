import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { enqueue, listQueue, flushQueue, queuedCount } from "../../web/offline-queue.mjs";

function mockFetch(handler) {
  globalThis.fetch = vi.fn(async (url, init) => handler(String(url), init || {}));
}

beforeEach(() => {
  mockFetch(() => new Response(null, { status: 204 }));
});

afterEach(async () => {
  mockFetch(() => new Response(null, { status: 204 }));
  await flushQueue();
});

describe("offline queue", () => {
  it("stores a request and lists it in order", async () => {
    await enqueue({ method: "POST", url: "/api/v1/questions/1/answer", body: JSON.stringify({ body: "hi" }) });
    await enqueue({ method: "POST", url: "/api/v1/approvals/2/decision", body: JSON.stringify({ decision: "approve" }) });
    const items = await listQueue();
    expect(items.length).toBe(2);
    expect(items[0].url).toContain("questions");
    expect(items[1].url).toContain("approvals");
  });

  it("flushes queued requests in order and clears the queue", async () => {
    await enqueue({ method: "POST", url: "/api/v1/questions/1/answer", body: JSON.stringify({ body: "first" }) });
    await enqueue({ method: "POST", url: "/api/v1/questions/2/answer", body: JSON.stringify({ body: "second" }) });
    const seen = [];
    mockFetch((url, init) => {
      seen.push(JSON.parse(init.body).body);
      return new Response(null, { status: 204 });
    });
    const sent = await flushQueue();
    expect(sent).toBe(2);
    expect(await queuedCount()).toBe(0);
    expect(seen).toEqual(["first", "second"]);
  });

  it("stops flushing on a network failure and keeps remaining items", async () => {
    await enqueue({ method: "POST", url: "/api/v1/questions/1/answer", body: JSON.stringify({ body: "first" }) });
    await enqueue({ method: "POST", url: "/api/v1/questions/2/answer", body: JSON.stringify({ body: "second" }) });
    let calls = 0;
    mockFetch(() => {
      calls += 1;
      if (calls === 1) throw new TypeError("Failed to fetch");
      return new Response(null, { status: 204 });
    });
    const sent = await flushQueue();
    expect(sent).toBe(0);
    expect(await queuedCount()).toBe(2);
  });
});
