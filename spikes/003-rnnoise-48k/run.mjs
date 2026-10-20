/**
 * Spike 003 运行器：起 Vite 静态页 -> Playwright 打开 -> 在页面内跑 rnnoise 48kHz 管线。
 * 运行：node run.mjs   （先 npm i && npx playwright install chromium）
 */
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.SPIKE_VITE_PORT || 5199);
const PAGE_URL = `http://127.0.0.1:${PORT}/`;

let viteLog = "";
const vite = spawn(process.execPath, [join(here, "node_modules/vite/bin/vite.js"), "--port", String(PORT), "--strictPort", "--host", "127.0.0.1"], {
  cwd: here,
  stdio: ["ignore", "pipe", "pipe"],
});
vite.stdout.on("data", (d) => (viteLog += d));
vite.stderr.on("data", (d) => (viteLog += d));

async function waitServer(ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(PAGE_URL);
      if (r.ok) return true;
    } catch {
      /* 还没起来 */
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  return false;
}

let out = { checks: {}, at: new Date().toISOString() };
try {
  if (!(await waitServer(90000))) throw new Error("vite 未就绪: " + viteLog.slice(-500));

  const context = await chromium.launchPersistentContext(mkdtempSync("/run/secrets/pw-"), {
  timeout: 300000,
    ignoreHTTPSErrors: true,
    args: [
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
    ],
  });
  await context.grantPermissions(["microphone"], { origin: `http://127.0.0.1:${PORT}` });
  const page = await context.newPage();
  page.setDefaultTimeout(120000);
  page.setDefaultNavigationTimeout(120000);
  const consoleErrors = [];
  page.on("console", (m) => {
    if (m.type() === "error") consoleErrors.push(m.text());
  });
  page.on("pageerror", (e) => consoleErrors.push("pageerror: " + e.message));

  await page.goto(PAGE_URL, { waitUntil: "load" });
  await page.waitForFunction(() => window.__spike003 !== undefined, null, { timeout: 120000 });
  const rn = await page.evaluate(() => window.__spike003.run({ mode: "rnnoise", seconds: 3 }));
  const pt = await page.evaluate(() => window.__spike003.run({ mode: "passthrough", seconds: 2 }));
  const offline = await page.evaluate(() => window.__spike003.offline({ frames: 300 }));

  out = {
    checks: {
      ranAt48k: rn.sampleRate === 48000 && pt.sampleRate === 48000,
      passthroughPassesAudio: pt.outRms > 0.05 && Math.abs(pt.outRms - pt.inRms) < 0.02,
      rnnoiseSuppresses: rn.outRms < pt.outRms / 2,
      rnnoiseP95Under10ms: typeof offline.p95ms === "number" && offline.p95ms < 10,
      noErrors:
        (rn.errors || []).length === 0 && (pt.errors || []).length === 0 && consoleErrors.length === 0,
    },
    rnnoise: rn,
    passthrough: pt,
    offline,
    consoleErrors,
    at: new Date().toISOString(),
  };
  await context.close();
} catch (e) {
  out.error = String(e && e.message ? e.message : e);
}

const REQUIRED = ["ranAt48k", "passthroughPassesAudio", "rnnoiseSuppresses", "rnnoiseP95Under10ms", "noErrors"];
const ok = REQUIRED.every((k) => out.checks[k] === true);
writeFileSync(join(here, "result.json"), JSON.stringify(out, null, 2));
vite.kill("SIGTERM");

if (ok) {
  console.log(
    `PASS rnnoise 48k (offline p95=${out.offline.p95ms}ms p50=${out.offline.p50ms}ms | worklet out=${out.rnnoise.outRms} in=${out.rnnoise.inRms} frames=${out.rnnoise.frames} | passthrough out=${out.passthrough.outRms} in=${out.passthrough.inRms})`,
  );
  process.exit(0);
}
console.log(
  "FAIL rnnoise 48k " + JSON.stringify({ checks: out.checks, rnnoise: out.rnnoise, passthrough: out.passthrough, offline: out.offline, error: out.error }),
);
process.exit(1);
