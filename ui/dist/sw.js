// Service worker Auris — PWA installable + cache hors-ligne (#95, #107).
//
// Quatre caches :
// - SHELL  : /assets/* (hashés au build), index.html, manifest, icônes, locales, thèmes.
// - API    : réponses GET /api/* en « network-first » — hors-ligne, on sert la
//            dernière réponse connue (dernier état : /api/state, playlists, favoris…).
// - COVERS : pochettes (/api/library/cover/*, /api/browse/cover) en
//            stale-while-revalidate : instantané si déjà vu, mise à jour en fond.
// - AUDIO  : flux /share/{token}/stream* mis en cache (taille bornée) pour
//            réécoute hors-ligne des morceaux déjà streamés.
// /ws (websocket état + audio live) n'est jamais intercepté.

const SHELL = "auris-shell-v2";
const API = "auris-api-v2";
const COVERS = "auris-covers-v2";
const AUDIO = "auris-audio-v2";
const KEEP = [SHELL, API, COVERS, AUDIO];

// GET /api/* jamais mis en cache (logs, exports, actions, découvertes réseau…).
const API_NO_CACHE = [
  "/api/system/logs",
  "/api/system/update",
  "/api/export",
  "/api/transfer/",
  "/api/upnp/",
  "/api/lastfm/",
  "/api/settings/backup",
  "/api/settings/restore",
  "/api/audio/dop",
];

const AUDIO_MAX_ENTRIES = 30;
const AUDIO_MAX_BYTES = 64 * 1024 * 1024;

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => !KEEP.includes(k)).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

function isCover(url) {
  return url.pathname.startsWith("/api/library/cover/") || url.pathname === "/api/browse/cover";
}

function isAudioStream(url) {
  return url.pathname.startsWith("/share/") && url.pathname.includes("/stream");
}

function apiCacheable(url) {
  if (!url.pathname.startsWith("/api/")) return false;
  return !API_NO_CACHE.some((p) => url.pathname.startsWith(p));
}

async function putBounded(cacheName, request, response) {
  const c = await caches.open(cacheName);
  const keys = await c.keys();
  while (keys.length >= AUDIO_MAX_ENTRIES) {
    await c.delete(keys.shift());
  }
  await c.put(request, response);
}

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== self.location.origin) return;
  if (url.pathname === "/ws" || url.pathname.startsWith("/ws/")) return;

  // 1. Assets hashés du build : cache-first.
  if (url.pathname.startsWith("/assets/")) {
    e.respondWith(
      caches.open(SHELL).then(async (c) => {
        const hit = await c.match(e.request);
        if (hit) return hit;
        const r = await fetch(e.request);
        if (r.ok) c.put(e.request, r.clone());
        return r;
      })
    );
    return;
  }

  // 2. Pochettes : stale-while-revalidate.
  if (isCover(url)) {
    e.respondWith(
      (async () => {
        const c = await caches.open(COVERS);
        const hit = await c.match(e.request);
        const net = fetch(e.request)
          .then((r) => {
            if (r.ok) c.put(e.request, r.clone());
            return r;
          })
          .catch(() => null);
        if (hit) {
          e.waitUntil(net);
          return hit;
        }
        const r = await net;
        return r || Response.error();
      })()
    );
    return;
  }

  // 3. Flux audio des liens de partage : network-first, mise en cache bornée,
  //    repli hors-ligne sur le morceau déjà streamé.
  if (isAudioStream(url)) {
    e.respondWith(
      (async () => {
        try {
          const r = await fetch(e.request);
          if (r.ok) {
            const len = Number(r.headers.get("content-length") || 0);
            if (len === 0 || len <= AUDIO_MAX_BYTES) {
              e.waitUntil(putBounded(AUDIO, e.request, r.clone()));
            }
          }
          return r;
        } catch (err) {
          const hit = await caches.match(e.request, { cacheName: AUDIO });
          if (hit) return hit;
          throw err;
        }
      })()
    );
    return;
  }

  // 4. API read-only : network-first, dernier état connu hors-ligne.
  if (apiCacheable(url)) {
    e.respondWith(
      (async () => {
        try {
          const r = await fetch(e.request);
          if (r.ok) {
            const clone = r.clone();
            caches.open(API).then((c) => c.put(e.request, clone));
          }
          return r;
        } catch (err) {
          const hit = await caches.match(e.request, { cacheName: API });
          if (hit) return hit;
          return new Response(JSON.stringify({ error: "hors-ligne" }), {
            status: 503,
            headers: { "Content-Type": "application/json" },
          });
        }
      })()
    );
    return;
  }

  // 5. Le reste (shell) : network-first, repli cache.
  e.respondWith(
    fetch(e.request)
      .then((r) => {
        if (r.ok) {
          const clone = r.clone();
          caches.open(SHELL).then((c) => c.put(e.request, clone));
        }
        return r;
      })
      .catch(() => caches.match(e.request))
  );
});
