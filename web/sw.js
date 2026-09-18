// Agent Hub service worker: offline shell for static assets, live for the API.
//
// The hub stamps both values below when it serves this file. VERSION is a
// digest of the embedded assets, so an upgrade that changes any of them
// changes this file, which is what makes a browser install the new worker and
// drop the shell it replaces. PRECACHE and ON_DEMAND together are the static
// routes the hub serves, so the offline shell cannot drift from what the app
// loads.
const VERSION = "{{version}}";
const PRECACHE = "{{assets}}".split(",");
const ON_DEMAND = "{{on_demand}}".split(",").filter(Boolean);
const SHELL = `agent-hub-shell-${VERSION}`;

// addAll is all or nothing, and a worker whose install fails never activates.
// The shell is small, so it is required. ON_DEMAND is the large runtime only
// some pages load: it is tried here, and cached on first use if this misses,
// so a dropped connection on a phone costs a diagram offline, not the app.
self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(SHELL).then(async (cache) => {
      await cache.addAll(PRECACHE);
      await Promise.allSettled(ON_DEMAND.map((path) => cache.add(path)));
    })
  );
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
    caches.match(event.request).then((cached) => {
      if (cached) return cached;
      if (!ON_DEMAND.includes(url.pathname)) return fetch(event.request);
      return fetch(event.request).then((response) => {
        if (response.ok) {
          const copy = response.clone();
          event.waitUntil(caches.open(SHELL).then((cache) => cache.put(event.request, copy)));
        }
        return response;
      });
    })
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
