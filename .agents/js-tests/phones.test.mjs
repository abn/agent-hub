// Settings > Phones following the hub while it is open: what the screen shows
// after the stream ticks, a code runs out or a refetch races a tick. The hub is
// a stand-in that answers `GET /api/v1/devices` from a listing the test moves,
// and the stream is a stand-in whose ticks the test fires.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const hub = vi.hoisted(() => ({
  listing: { enabled: true, relay: false, devices: [] },
  pairing: null,
  // While set, a devices read takes its answer when the read starts and hands
  // it back only once the gate opens: a read served before a later change.
  gate: null,
}));

const stream = vi.hoisted(() => ({ watchers: new Set(), ticks: 0, open: true }));

vi.mock("../../web/api.mjs", () => ({
  api: vi.fn(async (path) => {
    if (path === "/api/v1/devices") {
      const answer = structuredClone(hub.listing);
      if (hub.gate) await hub.gate;
      return answer;
    }
    if (path === "/api/v1/devices/pairing") return structuredClone(hub.pairing);
    return {};
  }),
}));

vi.mock("../../web/events.mjs", () => ({
  onTick: (watcher) => {
    stream.watchers.add(watcher);
    return () => stream.watchers.delete(watcher);
  },
  tickCount: () => stream.ticks,
  streamOpen: () => stream.open,
  refreshBadge: () => {},
  onArtifactLive: () => () => {},
}));

// The router sets the theme colour on every render; the real document has it.
if (!document.querySelector('meta[name="theme-color"]')) {
  const meta = document.createElement("meta");
  meta.name = "theme-color";
  document.head.appendChild(meta);
}

const { phonesScreen } = await import("../../web/phones.mjs");
const { render, setScreens } = await import("../../web/router.mjs");

function tick() {
  stream.ticks += 1;
  for (const watcher of [...stream.watchers]) watcher();
}

const add = () => document.querySelector("main .phones-add");
const settle = () => vi.advanceTimersByTimeAsync(0);

function phone(id, name, state, extra = {}) {
  return {
    id,
    name,
    kind: "android",
    state,
    check_string: state === "pending" ? "aaaa bbbb cccc dddd" : null,
    paired_at: new Date(Date.now() - 60_000).toISOString(),
    confirmed_at: state === "active" ? new Date(Date.now() - 30_000).toISOString() : null,
    revoked_at: null,
    last_seen_at: null,
    checked_in_at: null,
    ...extra,
  };
}

async function openScreen() {
  await render();
  await settle();
  expect(add()).not.toBeNull();
}

beforeEach(async () => {
  vi.useFakeTimers({ now: new Date("2026-11-27T10:00:00Z") });
  hub.listing = { enabled: true, relay: false, devices: [] };
  hub.gate = null;
  stream.open = true;
  setScreens({ phones: (_params, gen) => phonesScreen(gen) });
  window.location.hash = "#/phones";
});

afterEach(async () => {
  // Leave Add a phone with nothing held, for the next case.
  add()?.querySelector('[data-action="phones-close"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await settle();
  vi.useRealTimers();
});

describe("Add a phone follows the hub", () => {
  it("asks the hub when the code's time is up, so a phone that claimed it at the last moment is shown", async () => {
    stream.open = false;
    await openScreen();
    hub.pairing = {
      code: "TND-AAAA-BBBB-CCCC",
      uri: "tendril://pair?x",
      address: null,
      qr: { size: 21, path: "" },
      expires_at: new Date(Date.now() + 10_000).toISOString(),
    };
    add().querySelector('[data-action="phones-issue"]').dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(add().querySelector('[role="status"]').textContent).toBe("Waiting for the phone");

    // The last poll before the code runs out finds nothing; the phone claims
    // the code after it.
    await vi.advanceTimersByTimeAsync(9_500);
    hub.listing.devices = [phone("dev_1", "Pixel 9", "pending")];
    await vi.advanceTimersByTimeAsync(600);
    await vi.advanceTimersByTimeAsync(5_000);

    expect(add().textContent).toContain("Pixel 9 wants to pair");
    expect(add().textContent).not.toContain("The code ran out");
  });

  it("refetches once after it opens when the stream ticked while its first read was in flight", async () => {
    hub.listing.devices = [phone("dev_2", "Pixel 9", "active")];
    let open;
    hub.gate = new Promise((resolve) => {
      open = resolve;
    });
    const opening = render();
    await settle();
    // The read was answered before the phone checked in; the tick lands
    // while it is still on its way back.
    hub.listing.devices = [phone("dev_2", "Pixel 9", "active", { last_seen_at: new Date().toISOString(), checked_in_at: new Date().toISOString() })];
    tick();
    hub.gate = null;
    open();
    await opening;
    await settle();

    const row = document.querySelector("main .phones-paired .phones-meta");
    expect(row.textContent).toContain("seen");
  });

  it("keeps focus on a phone's Unpair when another phone joins the list", async () => {
    hub.listing.devices = [phone("dev_3", "Pixel 9", "active")];
    await openScreen();
    const unpair = /** @type {HTMLElement} */ (document.querySelector('[data-action="phones-revoke"][data-id="dev_3"]'));
    unpair.focus();
    expect(document.activeElement).toBe(unpair);

    hub.listing.devices = [phone("dev_4", "Pixel 8", "active"), phone("dev_3", "Pixel 9", "active")];
    tick();
    await settle();

    expect(document.querySelectorAll("main .phones-row")).toHaveLength(2);
    const focused = /** @type {HTMLElement} */ (document.activeElement);
    expect(focused.dataset.action).toBe("phones-revoke");
    expect(focused.dataset.id).toBe("dev_3");
  });
});
