/**
 * Spike 002 运行器：同一页面内 alice/bob 两个 LiveKit 会话，经 frontend-server
 * /rtc/ WSS 隧道连接 LiveKit，48kHz + 128kbps 发布/订阅。
 * 前置：test-server/start-test-env.sh 常驻；frontend/node_modules 已安装。
 * 运行：node run.mjs   （先 npm i 与 npx playwright install chromium）
 *
 * 沙箱调整：media-token 返回的 livekit_url 是配置里的局域网 IP（172.17.0.1），
 * 该地址在前端服务器同源路径下不可用，故统一改成页面 origin 的 host
 * （隧道本来就在同源 /rtc/ 上，与生产同域部署等价），原值记入 result.json。
 */
import { chromium } from "playwright";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const ORIGIN = process.env.SPIKE_ORIGIN || "https://localhost:8081";
const BACKEND_LOG = process.env.SPIKE_BACKEND_LOG || "/tmp/cloudroom-test-env/logs/backend.log";
const ROOM = process.env.SPIKE_ROOM || "502";
const UMD = join(here, "../../frontend/node_modules/livekit-client/dist/livekit-client.umd.js");
const BITRATE = 128000;
const SAMPLE_RATE = 48000;
const ALICE = { username: "alice", password: "spike-pass-alice" };
const BOB = { username: "bob", password: "spike-pass-bob" };

const CHROME_ARGS = [
  "--no-sandbox",
  "--disable-dev-shm-usage",
  "--disable-gpu",
  "--no-first-run",
  "--no-default-browser-check",
  "--disable-extensions",
  "--disable-background-networking",
  "--disable-sync",
  "--renderer-process-limit=1",
  "--use-fake-device-for-media-stream",
  "--use-fake-ui-for-media-stream",
  "--autoplay-policy=no-user-gesture-required",
];

function readBootstrap() {
  try {
    const m = readFileSync(BACKEND_LOG, "utf8").match(/bootstrap=([A-Za-z0-9_-]+)/);
    return m ? m[1] : null;
  } catch {
    return null;
  }
}

/** 把 media-token 的 livekit_url 改写成页面同源 host（隧道入口）。 */
function sameOriginUrl(u) {
  const parsed = new URL(u);
  parsed.host = new URL(ORIGIN).host;
  return parsed.toString();
}

process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0"; // 本地自签证书，仅 spike 用

/** Node 侧复放一个信令 URL，记录 open/首帧/关闭时序，用于区分"隧道"与"客户端"。 */
function wsCheck(rawUrl, label, timeoutMs = 10000) {
  return new Promise((resolve) => {
    const t0 = Date.now();
    const out = {
      label,
      url: rawUrl.replace(/access_token=[^&]+/, "access_token=<redacted>").slice(0, 400),
      events: [],
    };
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(out);
    };
    let ws;
    try {
      ws = new WebSocket(rawUrl);
    } catch (e) {
      out.events.push(["ctor-error", String(e && e.message)]);
      return finish();
    }
    const timer = setTimeout(() => {
      out.events.push(["timeout", Date.now() - t0]);
      try {
        ws.close();
      } catch {}
      finish();
    }, timeoutMs);
    ws.onopen = () => out.events.push(["open", Date.now() - t0]);
    ws.onmessage = (m) => {
      const size =
        typeof m.data === "string"
          ? m.data.length
          : m.data && typeof m.data.byteLength === "number"
            ? m.data.byteLength
            : -1;
      out.events.push(["msg", Date.now() - t0, size]);
      try {
        ws.close();
      } catch {}
      finish();
    };
    ws.onerror = (e) => out.events.push(["error", Date.now() - t0, String((e && e.message) || e)]);
    ws.onclose = (e) => {
      out.events.push(["close", Date.now() - t0, e.code, e.reason]);
      finish();
    };
  });
}

const context = await chromium.launchPersistentContext(mkdtempSync("/run/secrets/pw-"), {
  timeout: 300000,
  ignoreHTTPSErrors: true,
  args: CHROME_ARGS,
});
// 记录 livekit-client 实际使用的信令 URL，并支持把 URL 拿去 Node 侧 A/B 复放。
await context.addInitScript(() => {
  window.__wsUrls = [];
  const Orig = window.WebSocket;
  function Patched(url, protocols) {
    try {
      window.__wsUrls.push(String(url));
    } catch {
      /* 忽略 */
    }
    return protocols === undefined ? new Orig(url) : new Orig(url, protocols);
  }
  Patched.prototype = Orig.prototype;
  Patched.CONNECTING = Orig.CONNECTING;
  Patched.OPEN = Orig.OPEN;
  Patched.CLOSING = Orig.CLOSING;
  Patched.CLOSED = Orig.CLOSED;
  window.WebSocket = Patched;
});
await context.grantPermissions(["microphone"], { origin: ORIGIN });
const page = await context.newPage();
page.setDefaultTimeout(120000);
page.setDefaultNavigationTimeout(120000);
const consoleErrors = [];
page.on("console", (m) => {
  if (m.type() === "error") consoleErrors.push(m.text());
});
page.on("pageerror", (e) => consoleErrors.push("pageerror: " + e.message));

const step = async (label, fn, ms = 240000) => {
  const t = Date.now();
  try {
    const r = await Promise.race([
      fn(),
      new Promise((_, rej) => setTimeout(() => rej(new Error(label + " 超时 " + ms + "ms")), ms)),
    ]);
    console.log(`[step] ${label} ok ${Date.now() - t}ms`);
    return r;
  } catch (e) {
    console.log(`[step] ${label} FAIL ${Date.now() - t}ms: ${e.message}`);
    throw e;
  }
};

let out = { checks: {}, at: new Date().toISOString() };
try {
  await step("goto", () => page.goto(ORIGIN + "/manifest.json", { waitUntil: "domcontentloaded" }));
  await step("inject", async () => {
    await page.addScriptTag({ path: UMD });
    await page.addScriptTag({ path: join(here, "probe.js") });
  });

  const alice = await step("aliceSetup", () =>
    page.evaluate((o) => window.__spike002.aliceSetup(o), { ...ALICE, bootToken: readBootstrap(), roomCode: ROOM }),
  );
  if (!alice.ok) throw new Error("aliceSetup failed: " + JSON.stringify(alice));

  const tokA = await step("alice media-token", () =>
    page.evaluate((o) => window.__spike002.mediaToken(o), { csrf: alice.csrf, roomCode: ROOM }),
  );
  if (tokA.status !== 200) throw new Error("alice media-token failed: " + JSON.stringify(tokA));

  const bob = await step("bobSetup", () =>
    page.evaluate((o) => window.__spike002.bobSetup(o), {
      ...BOB,
      regToken: alice.regInvite,
      inviteToken: alice.roomInvite,
      roomCode: ROOM,
    }),
  );
  if (!bob.ok) throw new Error("bobSetup failed: " + JSON.stringify(bob));

  const tokB = await step("bob media-token", () =>
    page.evaluate((o) => window.__spike002.mediaToken(o), { csrf: bob.csrf, roomCode: ROOM }),
  );
  if (tokB.status !== 200) throw new Error("bob media-token failed: " + JSON.stringify(tokB));

  const urlA = sameOriginUrl(tokA.body.url);
  const urlB = sameOriginUrl(tokB.body.url);

  const connA = await step("connect alice", () =>
    page.evaluate((o) => window.__spike002.connect(o), {
      name: "alice",
      url: urlA,
      token: tokA.body.token,
      bitrate: BITRATE,
      sampleRate: SAMPLE_RATE,
      websocketTimeout: 240000,
      peerConnectionTimeout: 120000,
      useNoise: false,
    }),
    420000,
  );
  const connB = await step("connect bob", () =>
    page.evaluate((o) => window.__spike002.connect(o), {
      name: "bob",
      url: urlB,
      token: tokB.body.token,
      bitrate: BITRATE,
      sampleRate: SAMPLE_RATE,
      websocketTimeout: 240000,
      peerConnectionTimeout: 120000,
      useNoise: false,
    }),
    420000,
  );

  const remoteA = await step("remote audio alice", () =>
    page.evaluate((o) => window.__spike002.waitRemoteAudio(o), { name: "alice", timeoutMs: 25000 }),
  );
  const remoteB = await step("remote audio bob", () =>
    page.evaluate((o) => window.__spike002.waitRemoteAudio(o), { name: "bob", timeoutMs: 25000 }),
  );
  const measured = await step("measure alice", () =>
    page.evaluate((o) => window.__spike002.measure(o), { name: "alice", sampleMs: 3000 }),
  );

  out = {
    checks: {
      connected: connA.state === "connected" && connB.state === "connected",
      sampleRate48k: !!(connA.mic && connA.mic.sampleRate === SAMPLE_RATE),
      // 128kbps 是发布上限（Opus VBR + beep 输入实测远低于上限，故断言上限已下发）
      bitrateCeilingApplied: connA.maxBitrate === BITRATE,
      audioFlowing: !!(measured.ok && measured.kbps > 40),
      remoteAudioFlowing: remoteA.ok === true && remoteB.ok === true,
    },
    urlFromBackend: { alice: tokA.body.url, bob: tokB.body.url },
    urlUsed: { alice: urlA, bob: urlB },
    connA,
    connB,
    remoteA,
    remoteB,
    measured,
    consoleErrors,
    at: new Date().toISOString(),
  };

  await step("disconnect", () => page.evaluate(() => window.__spike002.disconnect()));
} catch (e) {
  out.error = String(e && e.message ? e.message : e);
  out.consoleErrors = consoleErrors;
  try {
    const urls = await page.evaluate(() => window.__wsUrls || []);
    out.signalUrls = urls.map((u) => u.replace(/access_token=[^&]+/, "access_token=<redacted>"));
    const raw = urls[urls.length - 1];
    if (raw) {
      out.replayViaTunnel = await wsCheck(raw, "tunnel");
      const direct = new URL(raw);
      direct.protocol = "ws:";
      direct.host = "127.0.0.1:7880";
      out.replayDirect = await wsCheck(direct.toString(), "direct");
    }
  } catch (e2) {
    out.diagError = String(e2 && e2.message ? e2.message : e2);
  }
}

const REQUIRED = ["connected", "sampleRate48k", "bitrateCeilingApplied", "audioFlowing", "remoteAudioFlowing"];
const ok = REQUIRED.every((k) => out.checks[k] === true);
writeFileSync(join(here, "result.json"), JSON.stringify(out, null, 2));
await context.close();

if (ok) {
  console.log(
    `PASS sfu tunnel 48k (maxBitrate=${out.connA.maxBitrate} measured=${out.measured.kbps}kbps remoteBytes=${out.remoteA.bytesReceived}/${out.remoteB.bytesReceived})`,
  );
  process.exit(0);
}
console.log(
  "FAIL sfu tunnel 48k " +
    JSON.stringify({ checks: out.checks, error: out.error, connA: out.connA, measured: out.measured, remoteA: out.remoteA, remoteB: out.remoteB }),
);
process.exit(1);
