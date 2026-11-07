"use strict";
// Offline copy for songden.
// - The page and the playlists: network first, last good copy when offline. Each
//   fresh playlist list also prunes saved songs that are on no playlist any more.
// - Songs: the first play streams from the server and saves the whole file in the
//   background; after that they play from the device. Safari asks for audio in
//   byte ranges, so saved songs are answered with 206 slices.
// - Covers: cache first.
// Requests other than GET (signing in, the parents' page) are never touched.

const SHELL = "songden-shell-v1";
const AUDIO = "songden-audio-v1";
const COVERS = "songden-covers-v1";
const SHELL_FILES = ["/", "/manifest.webmanifest", "/icon-180.png", "/icon-192.png", "/icon-512.png"];
const saving = new Set();

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(SHELL).then((c) => c.addAll(SHELL_FILES)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  const keep = new Set([SHELL, AUDIO, COVERS]);
  event.waitUntil(
    caches.keys()
      .then((names) => Promise.all(names.filter((n) => !keep.has(n)).map((n) => caches.delete(n))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== self.location.origin) return;
  const path = url.pathname;
  if (path.startsWith("/api/audio/")) event.respondWith(song(event, path));
  else if (path === "/api/playlists") event.respondWith(playlists(event));
  else if (path.startsWith("/api/cover/") || path.startsWith("/api/pcover/")) event.respondWith(cacheFirst(event.request, COVERS));
  else if (path.startsWith("/api/parents/")) return;
  else event.respondWith(networkFirst(event.request, SHELL));
});

async function networkFirst(request, cacheName) {
  const cache = await caches.open(cacheName);
  try {
    const res = await fetch(request);
    if (res.ok) await cache.put(request, res.clone());
    return res;
  } catch (err) {
    const hit = await cache.match(request, { ignoreSearch: true });
    if (hit) return hit;
    throw err;
  }
}

async function cacheFirst(request, cacheName) {
  const cache = await caches.open(cacheName);
  const hit = await cache.match(request);
  if (hit) return hit;
  const res = await fetch(request);
  if (res.ok) await cache.put(request, res.clone());
  return res;
}

async function playlists(event) {
  const res = await networkFirst(event.request, SHELL);
  if (res.ok) event.waitUntil(res.clone().json().then((data) => prune(new Set(data.keep))).catch(() => {}));
  return res;
}

// Drop saved songs and song covers that are on no playlist any more.
async function prune(keep) {
  for (const name of [AUDIO, COVERS]) {
    const cache = await caches.open(name);
    for (const req of await cache.keys()) {
      const path = new URL(req.url).pathname;
      if (!path.startsWith("/api/audio/") && !path.startsWith("/api/cover/")) continue;
      if (!keep.has(path.split("/").pop())) await cache.delete(req);
    }
  }
}

async function song(event, path) {
  const cache = await caches.open(AUDIO);
  const hit = await cache.match(path);
  if (hit) return slice(hit, event.request.headers.get("range"));
  if (!saving.has(path)) {
    saving.add(path);
    event.waitUntil(
      fetch(path)
        .then((res) => (res.status === 200 ? cache.put(path, res) : undefined))
        .catch((err) => console.warn("could not save", path, err))
        .finally(() => saving.delete(path)),
    );
  }
  return fetch(event.request);
}

async function slice(cached, range) {
  const blob = await cached.blob();
  const type = cached.headers.get("content-type") || "audio/mpeg";
  const m = range && /^bytes=(\d*)-(\d*)$/.exec(range);
  if (!m) {
    return new Response(blob, { status: 200, headers: { "Content-Type": type, "Content-Length": blob.size, "Accept-Ranges": "bytes" } });
  }
  let start, end;
  if (m[1] === "") {
    start = Math.max(blob.size - Number(m[2]), 0);
    end = blob.size - 1;
  } else {
    start = Number(m[1]);
    end = m[2] === "" ? blob.size - 1 : Math.min(Number(m[2]), blob.size - 1);
  }
  if (start > end || start >= blob.size) {
    return new Response(null, { status: 416, headers: { "Content-Range": `bytes */${blob.size}` } });
  }
  const part = blob.slice(start, end + 1);
  return new Response(part, {
    status: 206,
    headers: {
      "Content-Type": type,
      "Content-Length": part.size,
      "Content-Range": `bytes ${start}-${end}/${blob.size}`,
      "Accept-Ranges": "bytes",
    },
  });
}
