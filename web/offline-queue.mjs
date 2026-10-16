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
  await tryRegisterSync();
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

export async function flushQueue() {
  const items = await listQueue();
  if (!items.length) {
    window.dispatchEvent(new CustomEvent("agent-hub:queue-flushed", { detail: { count: 0 } }));
    return 0;
  }
  let sent = 0;
  for (const item of items) {
    try {
      await api(item.url, {
        method: item.method,
        body: item.body == null ? undefined : item.body,
      });
      await remove(item.id);
      sent += 1;
    } catch (error) {
      if (error && error.status) {
        await remove(item.id);
        continue;
      }
      break;
    }
  }
  if (sent > 0) toast("Queued requests sent.");
  window.dispatchEvent(new CustomEvent("agent-hub:queue-flushed", { detail: { count: sent } }));
  return sent;
}

async function tryRegisterSync() {
  try {
    if (!("serviceWorker" in navigator)) return;
    const reg = await navigator.serviceWorker.ready;
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
