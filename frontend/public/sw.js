/* CloudRoom PWA service worker：离线壳 + 静态资源运行时缓存。
 * 只缓存同源 GET；/api/ 与 WebSocket 永不缓存。 */
const SHELL = "cloudroom-shell-v5";
const ASSETS = "cloudroom-assets-v5";
const SHELL_FILES = ["/", "/manifest.json", "/icon.svg", "/icon-192.png", "/icon-512.png", "/apple-touch-icon.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(SHELL).then((cache) => cache.addAll(SHELL_FILES)).then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k !== SHELL && k !== ASSETS).map((k) => caches.delete(k))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.startsWith("/api/")) return;

  // 带 hash 的构建产物：缓存优先（内容不可变）。
  if (url.pathname.startsWith("/assets/")) {
    event.respondWith(
      caches.match(req).then(
        (hit) =>
          hit ||
          fetch(req).then((res) => {
            // Never cache an SPA fallback HTML response under an asset URL.
            const type = (res.headers.get("content-type") || "").toLowerCase();
            if (res.ok && res.type === "basic" && !type.includes("text/html")) {
              const copy = res.clone();
              void caches.open(ASSETS).then((c) => c.put(req, copy));
            }
            return res;
          }),
      ),
    );
    return;
  }

  // 页面导航：网络优先，离线回退到缓存的壳。
  if (req.mode === "navigate") {
    event.respondWith(
      fetch(req)
        .then((res) => {
          if (res.ok && res.type === "basic") {
            const copy = res.clone();
            void caches.open(SHELL).then((c) => c.put("/", copy));
          }
          return res;
        })
        .catch(() => caches.match("/")),
    );
  }
});

// Web Push：后端发送 {title, body, url, tag}，点击后聚焦/打开对应房间。
self.addEventListener("push", (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    data = {};
  }
  const title = data.title || "CloudRoom";
  event.waitUntil(
    (async () => {
      // 任何页面窗口存在时都由应用内通知负责；仅当页面全部关闭时才弹系统推送。
      // 背景标签页也会显示应用内通知，避免与 Web Push 重复。
      const clients = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      if (clients.length > 0) return;
      // 同一 tag（房间/线程）只保留一条通知，并累计条数，避免短时间刷屏。
      const tag = data.tag || "cloudroom";
      const existing = await self.registration.getNotifications({ tag });
      const count = existing.length ? Number(existing[0].data?.count || 1) + 1 : 1;
      const body = count > 1
        ? (data.body ? data.body + "（" + count + " 条新消息）" : count + " 条新消息")
        : data.body || "";
      await self.registration.showNotification(title, {
        body,
        tag,
        icon: "/icon-192.png",
        badge: "/icon-192.png",
        data: { url: data.url || "/", count },
      });
    })(),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const raw = (event.notification.data && event.notification.data.url) || "/";
  let target;
  try {
    target = new URL(raw, self.location.origin);
  } catch {
    return;
  }
  if (target.origin !== self.location.origin) return;
  event.waitUntil(
    self.clients
      .matchAll({ type: "window", includeUncontrolled: true })
      .then((list) => {
        for (const client of list) {
          if (client.url.startsWith(self.location.origin) && "focus" in client) {
            if ("navigate" in client) void client.navigate(target.href);
            return client.focus();
          }
        }
        return self.clients.openWindow(target.href);
      }),
  );
});

