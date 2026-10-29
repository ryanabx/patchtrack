// No service worker any more: the app shows the bridge's store and has
// nothing to do offline. This one replaces the worker an earlier version
// installed, which served the app from its cache: it drops that cache,
// unregisters itself and reloads the pages it controlled from the network.

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith("patchtrack-")).map((k) => caches.delete(k))))
      .then(() => self.registration.unregister())
      .then(() => self.clients.matchAll({ type: "window" }))
      .then((pages) => pages.forEach((p) => p.navigate(p.url))),
  );
});
