// Offline answer queue: stores POST requests that could not be sent because
// the browser was offline (or the request failed to reach the hub), and
// replays them in order once connectivity returns.

import { api } from "./api.mjs";
import { toast } from "./toast.mjs";

const DB_NAME = "agent-hub-offline-queue";
const STORE = "requests";
const DB_VERSION = 1;
const SYNC_TAG = "agent-hub-offline-queue";

const hasIDB = typeof indexedDB !== "undefined";

let memoryQueue = [];
let memoryNext = 1;

function openDB() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE)) {
        db.createObjectStore(STORE, { keyPath: "id", autoIncrement: true });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

export async function enqueue(request) {
  const item = {
    method: request.method || "POST",
    url: request.url,
    body: request.body == null ? null : request.body,
    createdAt: Date.now(),
  };
  let id;
  if (!hasIDB) {
    id = memoryNext++;
    memoryQueue.push({ id, ...item });
  } else {
    const db = await openDB();
    id = await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE, "readwrite");
      const store = tx.objectStore(STORE);
      const req = store.add(item);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }
  // Background sync is optional and must not hold the caller: the answer is
  // stored, so the reader is released whether or not a worker is active.
  tryRegisterSync();
  return id;
}

export async function listQueue() {
  if (!hasIDB) {
    return [...memoryQueue].sort((a, b) => a.id - b.id);
  }
  const db = await openDB();
  const rows = await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readonly");
    const store = tx.objectStore(STORE);
    const req = store.getAll();
    req.onsuccess = () => resolve(req.result || []);
    req.onerror = () => reject(req.error);
  });
  return rows.sort((a, b) => a.id - b.id);
}

export async function remove(id) {
  if (!hasIDB) {
    memoryQueue = memoryQueue.filter((item) => item.id !== id);
    return;
  }
  const db = await openDB();
  await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    const req = store.delete(id);
    req.onsuccess = () => resolve();
    req.onerror = () => reject(req.error);
  });
}

export async function queuedCount() {
  return (await listQueue()).length;
}

// Replays run one at a time, whatever raised them (the online event, a worker
// message, a caller). A row is claimed before its POST, so an overlapping
// trigger never sends the same answer twice.
let flushChain = Promise.resolve(0);
const claimed = new Set();

const LOCK = "offline-queue";
const REPLAY_DEADLINE_MS = 15000;

// Where Web Locks exist the replay is origin-wide, so two tabs cannot send the
// same row; otherwise the per-tab chain is the guard. The lock settles with
// what the replay settles with, a count.
export function flushQueue() {
  const locked = () =>
    typeof navigator !== "undefined" && navigator.locks && typeof navigator.locks.request === "function"
      ? navigator.locks.request(LOCK, replay).then((count) => count)
      : replay();
  const run = flushChain.then(locked, locked);
  flushChain = run.catch(() => 0);
  return run;
}

async function replay() {
  const items = (await listQueue()).filter((item) => !claimed.has(item.id));
  if (!items.length) {
    window.dispatchEvent(new CustomEvent("agent-hub:queue-flushed", { detail: { count: 0 } }));
    return 0;
  }
  let sent = 0;
  for (const item of items) {
    claimed.add(item.id);
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), REPLAY_DEADLINE_MS);
    try {
      await api(item.url, {
        method: item.method,
        body: item.body == null ? undefined : item.body,
        signal: controller.signal,
      });
      await remove(item.id);
      sent += 1;
    } catch (error) {
      if (error && error.status) {
        await remove(item.id);
        continue;
      }
      break;
    } finally {
      clearTimeout(timer);
      claimed.delete(item.id);
    }
  }
  if (sent > 0) toast("Queued requests sent.");
  window.dispatchEvent(new CustomEvent("agent-hub:queue-flushed", { detail: { count: sent } }));
  return sent;
}

async function tryRegisterSync() {
  try {
    if (!("serviceWorker" in navigator)) return;
    /** @type {ServiceWorkerRegistration & { sync?: { register(tag: string): Promise<void> } }} */
    const reg = await Promise.race([
      navigator.serviceWorker.ready,
      new Promise((resolve) => setTimeout(resolve, 2000)),
    ]);
    if (reg && reg.sync && typeof reg.sync.register === "function") {
      await reg.sync.register(SYNC_TAG);
    }
  } catch {
    // Background sync is optional; the online event still flushes.
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("online", () => {
    flushQueue().catch(() => {});
  });
  if (navigator.serviceWorker && typeof navigator.serviceWorker.addEventListener === "function") {
    navigator.serviceWorker.addEventListener("message", (event) => {
      const data = event.data || {};
      if (data.type === "flush-offline-queue") flushQueue().catch(() => {});
    });
  }
}
