// Agent Hub service worker: offline shell for static assets, live for the API.
const SHELL = "agent-hub-shell-v3";
const ASSETS = [
  "/",
  "/app.js",
  "/app.css",
  "/tokens.css",
  "/crypto.mjs",
  "/manifest.webmanifest",
  "/icon.svg",
];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(SHELL).then((cache) => cache.addAll(ASSETS)));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(keys.filter((key) => key !== SHELL).map((key) => caches.delete(key)))
    )
  );
  self.clients.claim();
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (
    event.request.method !== "GET" ||
    url.origin !== self.location.origin ||
    url.pathname.startsWith("/api/")
  ) {
    return;
  }
  event.respondWith(
    caches.match(event.request).then((cached) => cached || fetch(event.request))
  );
});

self.addEventListener("message", (event) => {
  const data = event.data || {};
  if (data.type !== "waiting") return;
  const count = Number(data.count) || 0;
  const items = count === 1 ? "item" : "items";
  event.waitUntil(
    self.registration.showNotification("Agent Hub", {
      body: `An agent reported new work. ${count} ${items} waiting on you.`,
      icon: "/icon.svg",
      badge: "/icon.svg",
      tag: "agent-hub-waiting",
    })
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const inbox = self.location.origin + "/#/inbox";
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((clients) => {
      for (const client of clients) {
        if (new URL(client.url).origin !== self.location.origin) continue;
        client.focus();
        if (typeof client.navigate === "function") client.navigate(inbox);
        return;
      }
      return self.clients.openWindow(inbox);
    })
  );
});
