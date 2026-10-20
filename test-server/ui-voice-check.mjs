#!/usr/bin/env node
/**
 * UI voice smoke test for a running CloudRoom test environment.
 *
 * Drives a real Chrome instance through the DevTools Protocol using only
 * Node 22 built-ins (fetch + WebSocket). No npm dependencies, no browser
 * download. See test-server/README.md for prerequisites.
 */
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const DEFAULT_CHROME =
  "/home/agent/.cache/ms-playwright/chromium-1248/chrome-linux64/chrome";
const DEFAULT_BASE_URL = "https://localhost:8081";
const DEFAULT_TIMEOUT_SECONDS = 40;

const USAGE = `用法:
  node test-server/ui-voice-check.mjs --user <name> --pass <password> --room <sfu-code> [options]

必填:
  --user <name>          登录用户名
  --pass <password>      登录密码
  --room <code>          已存在的 SFU 房间 code

可选:
  --base-url <url>       前端地址（默认 ${DEFAULT_BASE_URL}）
  --timeout <seconds>    整体超时（默认 ${DEFAULT_TIMEOUT_SECONDS}）
  --headless[=false]     默认开启；false 时不传 --headless=new
  -h, --help             显示本帮助

环境变量:
  CHROME_PATH            Chrome 可执行文件（默认 ${DEFAULT_CHROME}）

前置条件:
  测试环境已启动，用户与 SFU 房间已存在。
  若浏览器访问 LiveKit 信令时被沙箱策略拦截（172.17.0.1:8081 → 403），
  用以下方式启动测试环境：
    LIVEKIT_PUBLIC_URL=wss://localhost:8081 LIVEKIT_NODE_IP=127.0.0.1 \\
      ./test-server/start-test-env.sh`;

function parseArgs(argv) {
  const out = { headless: true };
  const valueFlags = {
    "base-url": "baseUrl",
    user: "user",
    pass: "pass",
    room: "room",
    timeout: "timeout",
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "-h" || arg === "--help") {
      console.log(USAGE);
      process.exit(0);
    }
    if (arg === "--headless") {
      out.headless = true;
      continue;
    }
    if (arg === "--no-headless") {
      out.headless = false;
      continue;
    }
    if (arg.startsWith("--headless=")) {
      out.headless = arg.slice("--headless=".length) !== "false";
      continue;
    }
    if (!arg.startsWith("--")) {
      throw new Error(`无法识别的参数: ${arg}\n\n${USAGE}`);
    }
    const eq = arg.indexOf("=");
    const key = eq === -1 ? arg.slice(2) : arg.slice(2, eq);
    const value = eq === -1 ? argv[++i] : arg.slice(eq + 1);
    if (value === undefined || value === "") {
      throw new Error(`参数 --${key} 缺少值\n\n${USAGE}`);
    }
    const mapped = valueFlags[key];
    if (!mapped) throw new Error(`无法识别的参数: --${key}\n\n${USAGE}`);
    out[mapped] = value;
  }
  if (!out.user || !out.pass || !out.room) {
    throw new Error(`--user、--pass、--room 均为必填\n\n${USAGE}`);
  }
  out.baseUrl = (out.baseUrl || DEFAULT_BASE_URL).replace(/\/+$/, "");
  const seconds = Number(out.timeout || DEFAULT_TIMEOUT_SECONDS);
  if (!Number.isFinite(seconds) || seconds <= 0) {
    throw new Error("--timeout 必须是正数秒");
  }
  out.timeoutMs = seconds * 1000;
  return out;
}

const args = parseArgs(process.argv.slice(2));
const chromePath = process.env.CHROME_PATH || DEFAULT_CHROME;
const roomPath = `/${encodeURIComponent(args.room)}`;
const roomUrl = `${args.baseUrl}${roomPath}`;
const loginUrl = `${args.baseUrl}/login`;

const seen = {
  requests: [],
  requestMethods: new Map(),
  responses: [],
  wsCreated: [],
  wsHandshakes: [],
  wsClosed: [],
  consoleErrors: [],
  exceptions: [],
  logErrors: [],
  loadingFailed: [],
  preLoginMe401: 0,
  loginDone: false,
};
let pageUrl = "";
let pageSnapshot = "";
let chrome;
let chromeProfile;
let cdp;
let deadline = 0;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const remaining = () => Math.max(0, deadline - Date.now());

function timeoutError(label) {
  return new Error(`等待超时: ${label}`);
}

async function waitFor(label, predicate, timeoutMs = remaining()) {
  const end = Date.now() + Math.min(timeoutMs, remaining());
  let lastError;
  while (Date.now() < end) {
    try {
      const value = await predicate();
      if (value) return value;
    } catch (err) {
      lastError = err;
    }
    await sleep(200);
  }
  const suffix = lastError ? ` (${lastError.message})` : "";
  throw timeoutError(`${label}${suffix}`);
}

function relevantRequests() {
  return seen.requests.filter(
    (r) =>
      r.url.includes("/media-token") ||
      r.url.includes("/api/v1/voice/") ||
      r.url.includes("/rtc/"),
  );
}

function relevantResponses() {
  return seen.responses.filter(
    (r) =>
      r.url.includes("/media-token") ||
      r.url.includes("/api/v1/voice/") ||
      r.url.includes("/rtc/"),
  );
}

/** The pre-login auth provider intentionally probes /api/v1/me and gets 401. */
function filteredLogErrors() {
  const out = [];
  let allowed401 = seen.preLoginMe401;
  for (const entry of seen.logErrors) {
    if (allowed401 > 0 && entry.text.includes("401")) {
      allowed401 -= 1;
      continue;
    }
    out.push(entry);
  }
  return out;
}

function allConsoleErrors() {
  return [
    ...seen.consoleErrors,
    ...seen.exceptions,
    ...filteredLogErrors(),
  ];
}

async function pageText() {
  if (!cdp) return "";
  try {
    return (await evaljs("document.body.innerText")) || "";
  } catch {
    return "";
  }
}

async function pageErrorText() {
  if (!cdp) return "";
  try {
    return (await evaljs('document.querySelector(".error")?.innerText || ""')) || "";
  } catch {
    return "";
  }
}

async function refreshPageSnapshot() {
  if (!cdp) return;
  try {
    pageUrl = await evaljs("location.href");
    pageSnapshot = (await pageText()).slice(0, 600);
  } catch {
    // page may be navigating
  }
}

async function printFailure(err) {
  await refreshPageSnapshot();
  console.error(`\nFAIL: ${err.message}`);
  if (pageUrl) console.error(`  page: ${pageUrl}`);
  const uiError = await pageErrorText();
  if (uiError) console.error(`  ui-error: ${uiError}`);
  if (pageSnapshot) console.error(`  page-text: ${pageSnapshot.replace(/\s+/g, " ").trim()}`);

  console.error("  relevant requests:");
  const reqs = relevantRequests();
  const resps = relevantResponses();
  if (reqs.length === 0) console.error("    (none)");
  for (const req of reqs.slice(-20)) {
    const resp = resps.find((r) => r.url === req.url);
    console.error(`    ${req.method} ${req.url}${resp ? ` -> ${resp.status}` : ""}`);
  }
  if (seen.wsHandshakes.length > 0) {
    console.error("  websocket handshakes:");
    for (const ws of seen.wsHandshakes) {
      console.error(`    ${ws.status ?? "?"} ${ws.url}`);
    }
  }
  if (seen.wsClosed.length > 0) {
    console.error("  websocket closes:");
    for (const ws of seen.wsClosed) console.error(`    ${ws.url}`);
  }
  const consoleErrors = allConsoleErrors();
  console.error(`  console errors: ${consoleErrors.length}`);
  for (const entry of consoleErrors.slice(0, 10)) console.error(`    ${entry.text}`);
  if (seen.loadingFailed.length > 0) {
    console.error(`  loadingFailed: ${seen.loadingFailed.length}`);
    for (const entry of seen.loadingFailed.slice(0, 5)) {
      console.error(`    ${entry.errorText || entry.error || "failed"} ${entry.url || ""}`);
    }
  }
}

class CdpClient {
  constructor(wsUrl) {
    this.wsUrl = wsUrl;
    this.ws = null;
    this.nextId = 0;
    this.pending = new Map();
  }

  async connect() {
    this.ws = new WebSocket(this.wsUrl);
    await new Promise((resolve, reject) => {
      this.ws.onopen = resolve;
      this.ws.onerror = () => reject(new Error(`无法连接 Chrome DevTools: ${this.wsUrl}`));
    });
    this.ws.onmessage = (event) => this.onMessage(event);
    this.ws.onerror = () => {};
  }

  onMessage(event) {
    let message;
    try {
      message = JSON.parse(String(event.data));
    } catch {
      return;
    }
    if (message.id) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(new Error(JSON.stringify(message.error)));
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    handleCdpEvent(message.method, message.params || {});
  }

  send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      // already closed
    }
  }
}

function handleCdpEvent(method, params) {
  if (method === "Runtime.consoleAPICalled") {
    if (params.type === "error") {
      seen.consoleErrors.push({
        text: (params.args || []).map((a) => a.value ?? a.description ?? "").join(" "),
      });
    }
  } else if (method === "Runtime.exceptionThrown") {
    const details = params.exceptionDetails || {};
    seen.exceptions.push({
      text: `${details.text || ""} ${details.exception?.description || ""}`.trim(),
    });
  } else if (method === "Log.entryAdded") {
    const entry = params.entry || {};
    if (entry.level === "error") {
      seen.logErrors.push({ text: entry.text || "", url: entry.url || "" });
    }
  } else if (method === "Network.requestWillBeSent") {
    const url = params.request?.url || "";
    const requestMethod = params.request?.method || "GET";
    if (params.requestId) seen.requestMethods.set(params.requestId, requestMethod);
    if (url.includes("/api/v1/") || url.includes("/rtc/")) {
      seen.requests.push({ method: requestMethod, url });
    }
  } else if (method === "Network.responseReceived") {
    const url = params.response?.url || "";
    const status = params.response?.status;
    if (url.includes("/api/v1/") || url.includes("/rtc/")) {
      seen.responses.push({
        url,
        status,
        method: seen.requestMethods.get(params.requestId) || "",
      });
    }
    if (!seen.loginDone && status === 401 && url.endsWith("/api/v1/me")) {
      seen.preLoginMe401 += 1;
    }
  } else if (method === "Network.loadingFailed") {
    seen.loadingFailed.push({
      errorText: params.errorText,
      url: params.requestId,
    });
  } else if (method === "Network.webSocketCreated") {
    seen.wsCreated.push({ requestId: params.requestId, url: params.url });
  } else if (method === "Network.webSocketHandshakeResponseReceived") {
    seen.wsHandshakes.push({
      requestId: params.requestId,
      url:
        seen.wsCreated.find((w) => w.requestId === params.requestId)?.url || "",
      status: params.response?.status,
    });
  } else if (method === "Network.webSocketClosed") {
    seen.wsClosed.push({
      requestId: params.requestId,
      url:
        seen.wsCreated.find((w) => w.requestId === params.requestId)?.url || "",
    });
  }
}

async function evaljs(expression) {
  const result = await cdp.send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.text || "页面脚本执行失败");
  }
  return result.result?.value;
}

async function navigate(url) {
  await cdp.send("Page.navigate", { url });
  await waitFor(`加载 ${url}`, async () => (await evaljs("document.readyState")) === "complete");
  await sleep(500);
}

async function getPageTarget(debugBase) {
  const list = await (await fetch(`${debugBase}/json/list`)).json();
  const page = list.find((target) => target.type === "page");
  if (page) return page;
  const created = await (
    await fetch(`${debugBase}/json/new?about:blank`, { method: "PUT" })
  ).json();
  return created;
}

async function startChrome() {
  if (!fs.existsSync(chromePath)) {
    throw new Error(
      `找不到 Chrome: ${chromePath}\n请设置 CHROME_PATH 或恢复 Playwright 浏览器缓存。`,
    );
  }
  chromeProfile = fs.mkdtempSync(path.join(os.tmpdir(), "cloudroom-ui-voice-"));
  const chromeArgs = [
    "--no-sandbox",
    "--disable-gpu",
    "--disable-dev-shm-usage",
    "--disable-background-networking",
    "--disable-sync",
    "--no-first-run",
    "--no-default-browser-check",
    "--ignore-certificate-errors",
    "--use-fake-device-for-media-stream",
    "--use-fake-ui-for-media-stream",
    "--autoplay-policy=no-user-gesture-required",
    "--remote-allow-origins=*",
    "--remote-debugging-port=0",
    `--user-data-dir=${chromeProfile}`,
    "about:blank",
  ];
  if (args.headless) chromeArgs.unshift("--headless=new");

  chrome = spawn(chromePath, chromeArgs, {
    stdio: ["ignore", "ignore", "pipe"],
    detached: true,
  });
  let stderr = "";
  let settled = false;
  const wsUrl = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error(`Chrome DevTools 启动超时\n${stderr.slice(-2000)}`));
      }
    }, 15000);
    chrome.stderr.on("data", (chunk) => {
      stderr = (stderr + chunk.toString()).slice(-20000);
      const match = stderr.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match && !settled) {
        settled = true;
        clearTimeout(timer);
        resolve(match[1]);
      }
    });
    chrome.on("exit", (code) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(new Error(`Chrome 提前退出 (code ${code})\n${stderr.slice(-2000)}`));
      }
    });
  });
  const debugBase = `http://127.0.0.1:${new URL(wsUrl).port}`;
  const target = await getPageTarget(debugBase);
  return { target, debugBase };
}

async function cleanup() {
  cdp?.close();
  if (chrome?.pid) {
    try {
      process.kill(-chrome.pid, "SIGTERM");
    } catch {
      chrome.kill("SIGTERM");
    }
    await sleep(500);
    try {
      process.kill(-chrome.pid, "SIGKILL");
    } catch {
      // already exited
    }
  }
  if (chromeProfile) {
    try {
      fs.rmSync(chromeProfile, { recursive: true, force: true });
    } catch {
      // best effort
    }
  }
}

async function run() {
  const { target } = await startChrome();
  cdp = new CdpClient(target.webSocketDebuggerUrl);
  await cdp.connect();
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Network.enable");
  await cdp.send("Log.enable");
  await cdp.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `(() => {
      try {
        const Orig = window.RTCPeerConnection;
        window.__voicePCs = [];
        window.RTCPeerConnection = new Proxy(Orig, {
          construct(target, args) {
            const pc = Reflect.construct(target, args);
            window.__voicePCs.push(pc);
            return pc;
          },
        });
      } catch (err) {
        window.__voiceInjectError = String(err);
      }
    })();`,
  });

  deadline = Date.now() + args.timeoutMs;

  await navigate(loginUrl);
  await waitFor("登录表单", () => evaljs("!!document.querySelector('#login-username')"));
  await evaljs(`(() => {
    const set = (selector, value) => {
      const el = document.querySelector(selector);
      const proto = HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    set('#login-username', ${JSON.stringify(args.user)});
    set('#login-password', ${JSON.stringify(args.pass)});
    document.querySelector('form.card button[type="submit"]').click();
    return true;
  })()`);
  await waitFor("登录成功跳转", async () => (await evaljs("location.pathname")) === "/");
  seen.loginDone = true;

  await navigate(roomUrl);
  await waitFor("进入房间", async () => (await evaljs("location.pathname")) === roomPath);
  await waitFor(
    "加入语音按钮",
    () =>
      evaljs(
        `!![...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '加入语音')`,
      ),
  );
  const roomLoaded = await evaljs(
    "document.body.innerText.includes('SFU') && document.body.innerText.includes('加入语音')",
  );
  if (!roomLoaded) throw new Error("断言失败: 房间页未显示 SFU / 加入语音");

  await evaljs(`(() => {
    const b = [...document.querySelectorAll('button')].find((x) => x.textContent.trim() === '加入语音');
    if (!b) throw new Error('加入语音按钮不存在');
    b.click();
    return true;
  })()`);

  const mediaTokenPath = `/api/v1/rooms/${encodeURIComponent(args.room)}/media-token`;
  await waitFor("POST media-token 200", () => {
    const response = seen.responses.find(
      (r) =>
        r.method === "POST" &&
        r.url.includes(mediaTokenPath) &&
        r.status === 200,
    );
    return response || null;
  });
  await waitFor("/rtc/ WebSocket 101", () => {
    const handshake = seen.wsHandshakes.find(
      (h) => h.url.includes("/rtc/") && h.status === 101,
    );
    return handshake || null;
  });
  await waitFor("RTCPeerConnection connected", async () => {
    const states = await evaljs(
      "(window.__voicePCs || []).map((pc) => pc.connectionState)",
    );
    return states.includes("connected");
  });
  await waitFor("UI 语音中", async () => {
    const text = await pageText();
    return text.includes("语音中（") && text.includes("离开语音");
  });

  const publish = await waitFor("发布 audio 轨道 / outbound RTP", async () => {
    const result = await evaljs(`(async () => {
      const pcs = window.__voicePCs || [];
      const out = [];
      for (const pc of pcs) {
        const stats = await pc.getStats();
        let packetsSent = 0;
        stats.forEach((r) => {
          if (r.type === 'outbound-rtp' && r.kind === 'audio') {
            packetsSent += r.packetsSent || 0;
          }
        });
        const audioSender = pc.getSenders().some(
          (s) => s.track?.kind === 'audio' && s.track?.enabled,
        );
        const audioSendonly = pc.getTransceivers().some(
          (t) => t.sender?.track?.kind === 'audio' &&
            (t.direction === 'sendonly' || t.currentDirection === 'sendonly'),
        );
        out.push({ state: pc.connectionState, packetsSent, audioSender, audioSendonly });
      }
      return {
        pcs: out,
        published: out.some((p) => p.packetsSent > 0 || p.audioSender || p.audioSendonly),
      };
    })()`);
    return result.published ? result : null;
  });

  const closesBefore = seen.wsClosed.length;
  await evaljs(`(() => {
    const b = [...document.querySelectorAll('button')].find((x) => x.textContent.trim() === '离开语音');
    if (!b) throw new Error('离开语音按钮不存在');
    b.click();
    return true;
  })()`);
  await waitFor("离开后 RTCPeerConnection closed", async () => {
    const states = await evaljs(
      "(window.__voicePCs || []).map((pc) => pc.connectionState)",
    );
    return states.length > 0 && states.every((s) => s === "closed");
  });
  await waitFor("离开后 /rtc/ WebSocket 关闭", () =>
    seen.wsClosed.slice(closesBefore).some((w) => w.url.includes("/rtc/")),
  );
  await waitFor("离开语音 UI", async () => {
    const text = await pageText();
    return text.includes("加入语音") && !text.includes("语音中（");
  });

  const consoleErrors = allConsoleErrors();
  if (consoleErrors.length > 0) {
    throw new Error(`断言失败: console error = ${consoleErrors.length}`);
  }

  const token = seen.responses.find(
    (r) =>
      r.method === "POST" &&
      r.url.includes(mediaTokenPath) &&
      r.status === 200,
  );
  const handshake = seen.wsHandshakes.find(
    (h) => h.url.includes("/rtc/") && h.status === 101,
  );
  const close = seen.wsClosed.find((w) => w.url.includes("/rtc/"));
  const packets = publish.pcs.reduce((sum, p) => sum + p.packetsSent, 0);

  console.log("\nUI voice smoke: PASS");
  console.log(`  base-url: ${args.baseUrl}`);
  console.log(`  login: ${args.user} (room ${roomPath})`);
  console.log(`  media-token: ${token.method} ${token.status} ${token.url}`);
  console.log(`  /rtc/: ${handshake.status} ${handshake.url.split("?")[0]}`);
  console.log(`  pc: ${publish.pcs.map((p) => p.state).join(", ")}`);
  console.log(
    `  publish: outbound audio RTP ${packets} packets; sender=${publish.pcs.some((p) => p.audioSender)}; sendonly=${publish.pcs.some((p) => p.audioSendonly)}`,
  );
  console.log(`  leave: PC closed; LiveKit WS closed (${close.url.split("?")[0]})`);
  console.log(
    `  console: 0 errors (ignored ${seen.preLoginMe401} pre-login /api/v1/me 401)`,
  );
  console.log(`  timeout: ${args.timeoutMs / 1000}s`);
}

let exitCode = 0;
try {
  await run();
} catch (err) {
  exitCode = 1;
  await printFailure(err);
} finally {
  await cleanup();
}
process.exit(exitCode);
