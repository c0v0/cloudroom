/**
 * Spike 001 运行器：Playwright 驱动真实 Chromium（headless shell）。
 * 前置：test-server/start-test-env.sh 常驻（默认 8080/8081/7880）。
 * 运行：node run.mjs   （先 npm i 与 npx playwright install chromium）
 * 结果：stdout PASS/FAIL，明细写 result.json；失败 exit 1。
 *
 * 沙箱注意：根分区 I/O 极慢，Chromium profile 放到 tmpfs（/run/secrets），
 * 否则 newPage 要 2 分钟以上；首次启动本身约 50s，属预期。
 */
import { chromium } from "playwright";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const ORIGIN = process.env.SPIKE_ORIGIN || "https://localhost:8081";
const BACKEND_LOG = process.env.SPIKE_BACKEND_LOG || "/tmp/cloudroom-test-env/logs/backend.log";
const ROOM = process.env.SPIKE_ROOM || "501";
const USER = { username: "alice", password: "spike-pass-alice" };

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

const context = await chromium.launchPersistentContext(mkdtempSync("/run/secrets/pw-"), {
  timeout: 300000,
  ignoreHTTPSErrors: true,
  args: CHROME_ARGS,
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

await page.goto(ORIGIN + "/manifest.json", { waitUntil: "domcontentloaded" });
await page.addScriptTag({ path: join(here, "probe.js") });

const result = await page.evaluate((opts) => window.__spike001(opts), {
  ...USER,
  bootToken: readBootstrap(),
  roomCode: ROOM,
});

const checks = {
  connected: result.ok === true && ["connected", "completed"].includes(result.iceConnectionState),
  relayOnly: result.pairLocal === "relay" && result.pairRemote === "relay",
  audioFlowing: (result.bytesReceived || 0) > 0,
};
const ok = Object.values(checks).every(Boolean);
const out = { checks, result, consoleErrors, at: new Date().toISOString() };
writeFileSync(join(here, "result.json"), JSON.stringify(out, null, 2));
await context.close();

if (ok) {
  console.log("PASS relay audio");
  process.exit(0);
}
console.log("FAIL relay audio " + JSON.stringify({ checks, stage: result.stage, log: result.log }));
process.exit(1);
