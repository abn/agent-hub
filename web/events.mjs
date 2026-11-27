// The waiting badge, the freshness stream behind it, and the notification the
// stream raises when work starts waiting while the app is in the background.

import { api } from "./api.mjs";
import { needsYouCount, readSnoozes } from "./now-model.mjs";
import { alertsEnabled, prefs } from "./prefs.mjs";

const badge = document.getElementById("tab-badge");

// The badge counts what Now reads, so a screen that holds that payload hands
// it over rather than asking for the same thing again. It is the asks that
// need the reader now: one that can wait, by its deadline or a snooze on this
// device, is not counted, the same as Now's own count.
export async function refreshBadge(known) {
  try {
    const data = known || (await api("/api/v1/home"));
    const count = needsYouCount(data, { snoozes: readSnoozes() });
    badge.hidden = count === 0;
    badge.textContent = String(count);
    noteWaiting(count);
  } catch {
    badge.hidden = true;
  }
}

let knownWaiting = null;

function noteWaiting(count) {
  if (knownWaiting === null) {
    knownWaiting = count;
    return;
  }
  const grew = count > knownWaiting;
  knownWaiting = count;
  if (grew && document.hidden) showWaitingNotification(count);
}

function showWaitingNotification(count) {
  // The reader's switch comes first. The worker cannot read local storage, so
  // deciding here is the only place the opt-out can be honoured. H21.
  if (!alertsEnabled()) return;
  if (!("Notification" in window) || Notification.permission !== "granted") return;
  if (!("serviceWorker" in navigator)) return;
  navigator.serviceWorker.ready
    .then((registration) => {
      if (registration.active) registration.active.postMessage({ type: "waiting", count });
    })
    .catch(() => {});
}

// Read the server's freshness stream. A plain tick refetches the badge; an
// event naming one artifact's live version is handed to whoever is watching
// that artifact, so a live preview refetches without a poll. A dropped stream
// reconnects, and the slow poll covers any gap. The loop also waits for a
// token, so entering one in Settings starts the stream without a reload.
let streamRunning = false;

// Watchers of a live artifact, by artifact id. An entry is added when a viewer
// opens and removed when it closes.
const liveWatchers = new Set();

/** Subscribe to live writes. Returns an unsubscribe function. */
export function onArtifactLive(watcher) {
  liveWatchers.add(watcher);
  return () => liveWatchers.delete(watcher);
}

function dispatchLive(payload) {
  for (const watcher of liveWatchers) {
    try {
      watcher(payload);
    } catch {}
  }
}

// Screens that refetch on a plain tick while they are open.
const tickWatchers = new Set();

// How many plain ticks have arrived. A screen reads it before a fetch and
// again after it, so a tick that landed while the fetch was out is not lost.
let ticks = 0;

/** The number of plain ticks so far. */
export function tickCount() {
  return ticks;
}

/** Run `watcher` on every plain tick. Returns an unsubscribe function. */
export function onTick(watcher) {
  tickWatchers.add(watcher);
  return () => tickWatchers.delete(watcher);
}

let streamConnected = false;

/** Whether the freshness stream is connected now, so a screen can poll when it is not. */
export function streamOpen() {
  return streamConnected;
}

export async function startStream() {
  if (streamRunning) return;
  if (!prefs.token) {
    setTimeout(startStream, 15000);
    return;
  }
  streamRunning = true;
  try {
    // Not routed through api.mjs's api(): the stream never returns JSON and
    // is read as a raw body, but it needs the same document-relative
    // resolution so it still reaches the hub under a path prefix.
    const response = await fetch(new URL("api/v1/stream", document.baseURI), {
      headers: { Authorization: "Bearer " + prefs.token, Accept: "text/event-stream" },
    });
    if (!response.ok || !response.body) throw new Error("stream unavailable");
    streamConnected = true;
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const frames = buffer.split("\n\n");
      buffer = frames.pop();
      for (const frame of frames) handleFrame(frame);
    }
  } catch {}
  streamConnected = false;
  streamRunning = false;
  setTimeout(startStream, 15000);
}

/** One SSE frame: a live write goes to its watchers, anything else is a tick. */
function handleFrame(frame) {
  if (frame.includes("event: artifact-live")) {
    const line = frame.split("\n").find((entry) => entry.startsWith("data:"));
    if (!line) return;
    try {
      dispatchLive(JSON.parse(line.slice(5).trim()));
    } catch {}
    return;
  }
  if (!frame.includes("event: tick")) return;
  ticks += 1;
  refreshBadge();
  for (const watcher of tickWatchers) {
    try {
      watcher();
    } catch {}
  }
}
