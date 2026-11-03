// 双用户探针：B（访客）触发输入中/举手/语音表情/已读，A（房主）验证 UI。
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const BASE = "https://localhost:8081";
const BOOT = process.env.BOOT || (readFileSync("/tmp/cloudroom-test-env/bootstrap-url", "utf8").split("bootstrap=")[1] || "").trim();
const PASS = "correct-horse-battery";
const stamp = String(Date.now()).slice(-6);
const room = `tw${stamp}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = [];
const rec = (n, ok, d = "") => out.push(`${ok ? "PASS" : "FAIL"}  ${n}${d ? "  — " + String(d).slice(0, 200) : ""}`);

class CDP {
  constructor(ws) { this.ws = ws; this.n = 0; this.p = new Map(); this.errs = [];
    ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && this.p.has(m.id)) { const q = this.p.get(m.id); this.p.delete(m.id); m.error ? q.rej(new Error(JSON.stringify(m.error))) : q.res(m.result); } else if (m.method === "Runtime.exceptionThrown") this.errs.push(String(m.params.exceptionDetails?.exception?.description || "").slice(0, 120)); }; }
  static async open(u) { const ws = new WebSocket(u); await new Promise((r, j) => { ws.onopen = r; ws.onerror = () => j(new Error("ws")); }); return new CDP(ws); }
  send(m, p = {}) { const id = ++this.n; return new Promise((res, rej) => { this.p.set(id, { res, rej }); this.ws.send(JSON.stringify({ id, method: m, params: p })); }); }
  async eval(e) { let r; try { r = await this.send("Runtime.evaluate", { expression: e, awaitPromise: true, returnByValue: true, userGesture: true }); } catch { return undefined; } if (r.exceptionDetails) return { __err: String(r.exceptionDetails.exception?.description || r.exceptionDetails.text).slice(0, 160) }; return r.result.value; }
  async waitFor(e, ms = 8000) { const t0 = Date.now(); while (Date.now() - t0 < ms) { if (await this.eval(`(function(){try{return !!(${e})}catch(_){return false}})()`) === true) return true; await sleep(200); } return false; }
}
const procs = [];
async function launch(port) {
  const prof = mkdtempSync(path.join(os.tmpdir(), `tw-${port}-`));
  const p = spawn(process.env.CHROME_PATH, ["--headless=new", `--remote-debugging-port=${port}`, "--no-sandbox", "--no-first-run",
    "--disable-dev-shm-usage", "--ignore-certificate-errors", "--allow-insecure-localhost",
    "--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream",
    "--autoplay-policy=no-user-gesture-required", "--window-size=1200,900",
    `--user-data-dir=${prof}`, "about:blank"], { stdio: "ignore" });
  procs.push(p);
  for (let i = 0; i < 80; i++) { try { await (await fetch(`http://127.0.0.1:${port}/json/version`)).json(); return; } catch { await sleep(250); } }
}
async function pageWs(port) { for (let i = 0; i < 40; i++) { const l = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); const p = l.find((t) => t.type === "page"); if (p?.webSocketDebuggerUrl) return p.webSocketDebuggerUrl; await sleep(250); } }
const mk = async (port) => {
  const c = await CDP.open(await pageWs(port));
  await c.send("Page.enable"); await c.send("Runtime.enable");
  c.goto = async (u) => { await c.send("Page.navigate", { url: u }); for (let i = 0; i < 100; i++) { await sleep(200); if (await c.eval("document.readyState") === "complete") break; } await sleep(900); };
  return c;
};

try {
  await launch(9360); await launch(9361);
  const A = await mk(9360);
  const B = await mk(9361);

  // A 注册 + 建房
  await A.goto(`${BASE}/register?bootstrap=${BOOT}`);
  await A.eval(`(()=>{const set=(s,v)=>{const e=document.querySelector(s);Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,v);e.dispatchEvent(new Event('input',{bubbles:true}))};set('#reg-username','a${stamp}');set('#reg-password','${PASS}');set('#reg-confirm','${PASS}');document.querySelector('form button').click();return true})()`);
  rec("A 注册", await A.waitFor(`location.pathname === '/'`, 9000));
  const created = await A.eval(`(async () => {
    const me = await (await fetch('/api/v1/me',{credentials:'same-origin'})).json();
    const r = await fetch('/api/v1/room-requests',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','x-csrf-token':me.csrf_token},body:JSON.stringify({kind:'create_room',note:'two user',desired_code:'${room}',media_mode:'sfu'})});
    const j = await r.json(); return { status:r.status, code:j.room&&j.room.code };
  })()`);
  rec("A 建房", created && created.status === 200, JSON.stringify(created));
  await A.goto(`${BASE}/${room}`);
  await A.waitFor(`!!document.querySelector('[placeholder*="输入消息"]')`, 15000);

  // A 发一条消息（供 B 已读）
  await A.eval(`(()=>{const el=document.querySelector('[placeholder*="输入消息"]');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,'hello-from-A');el.dispatchEvent(new Event('input',{bubbles:true}));const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim()==='发送');b&&b.click();return true})()`);
  rec("A 消息已发送", await A.waitFor(`document.body.innerText.includes('hello-from-A')`, 6000));

  // A 生成邀请
  const invite = await A.eval(`(async () => {
    const me = await (await fetch('/api/v1/me',{credentials:'same-origin'})).json();
    const r = await fetch('/api/v1/rooms/${room}/invites',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','x-csrf-token':me.csrf_token},body:JSON.stringify({expires_in:'1h'})});
    const j = await r.json(); return { status:r.status, token:(j.invite&&j.invite.token)||j.token, url:j.url };
  })()`);
  rec("A 生成邀请", !!(invite && invite.token), JSON.stringify(invite));

  // B 以访客加入
  await B.goto(`${BASE}/${room}?invite=${invite.token}`);
  rec("B 加入房间", await B.waitFor(`!!document.querySelector('[placeholder*="输入消息"]')`, 15000), await B.eval(`location.pathname`));

  // B 输入 → A 看到「正在输入」
  await B.eval(`(()=>{const el=document.querySelector('[placeholder*="输入消息"]');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,'typing now');el.dispatchEvent(new Event('input',{bubbles:true}));return true})()`);
  rec("A 看到正在输入", await A.waitFor(`!!document.querySelector('.typing-indicator')`, 5000), await A.eval(`(document.querySelector('.typing-indicator')||{}).innerText`));

  // B 加入语音 → 举手 → 表情
  await B.eval(`(()=>{const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim()==='加入语音');b&&b.click();return true})()`);
  const inVoice = await B.waitFor(`document.body.innerText.includes('离开语音')`, 25000);
  rec("B 加入语音", inVoice);
  await B.eval(`(()=>{const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim().startsWith('举手'));b&&b.click();return true})()`);
  rec("A 看到 B 举手", await A.waitFor(`!!document.querySelector('.hand-badge')`, 6000), await A.eval(`document.querySelectorAll('.hand-badge').length`));
  await B.eval(`(()=>{const b=[...document.querySelectorAll('.emoji-btn')][2];b&&b.click();return true})()`);
  rec("A 看到语音表情", await A.waitFor(`!!document.querySelector('.voice-reaction')`, 3000), await A.eval(`document.querySelectorAll('.voice-reaction').length`));

  // B 已读 → A 的消息显示「已读 1」
  rec("A 看到已读回执", await A.waitFor(`document.body.innerText.includes('已读 1')`, 8000), await A.eval(`document.body.innerText.match(/已读 \\d+/)?.[0]`));

  rec("A 控制台异常", A.errs.length === 0, JSON.stringify(A.errs.slice(0, 2)));
  rec("B 控制台异常", B.errs.length === 0, JSON.stringify(B.errs.slice(0, 2)));
} catch (e) { rec("FATAL", false, String(e)); }
finally { for (const p of procs) { try { p.kill("SIGKILL"); } catch {} } }
console.log(out.join("\n"));
