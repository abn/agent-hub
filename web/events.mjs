// The waiting badge, the freshness stream behind it, and the notification the
// stream raises when work starts waiting while the app is in the background.

import { api } from "./api.mjs";
import { prefs } from "./prefs.mjs";

const badge = document.getElementById("tab-badge");

export async function refreshBadge() {
  try {
    const data = await api("/api/v1/home");
    const count = data.waiting || 0;
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
  if (!("Notification" in window) || Notification.permission !== "granted") return;
  if (!("serviceWorker" in navigator)) return;
  navigator.serviceWorker.ready
    .then((registration) => {
      if (registration.active) registration.active.postMessage({ type: "waiting", count });
    })
    .catch(() => {});
}

// Read the server's freshness stream. It carries no event data; each tick just
// refetches the badge. A dropped stream reconnects, and the slow poll covers
// any gap. The loop also waits for a token, so entering one in Settings starts
// the stream without a reload.
let streamRunning = false;
export async function startStream() {
  if (streamRunning) return;
  if (!prefs.token) {
    setTimeout(startStream, 15000);
    return;
  }
  streamRunning = true;
  try {
    const response = await fetch("/api/v1/stream", {
      headers: { Authorization: "Bearer " + prefs.token, Accept: "text/event-stream" },
    });
    if (!response.ok || !response.body) throw new Error("stream unavailable");
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const frames = buffer.split("\n\n");
      buffer = frames.pop();
      if (frames.some((frame) => frame.includes("event: tick"))) refreshBadge();
    }
  } catch {}
  streamRunning = false;
  setTimeout(startStream, 15000);
}
