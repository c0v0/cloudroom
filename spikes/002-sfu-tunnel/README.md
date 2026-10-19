# Spike 002 — SFU 经 frontend-server `/rtc/` WSS 隧道

验证 livekit-client 通过 frontend-server 的 `/rtc/*` WSS 隧道连接 LiveKit，
并按 48kHz 采集、128kbps 发布，另一账号订阅到音频且码率达标。

## 结论（2026-10-18，baseline-20261018）

**未完成判定（环境阻塞）**：本沙箱（1 vCPU，load ≈ 9–10）无法完成浏览器侧信令握手，
但已确认隧道转发本身是通的；无法区分"环境超时"与"真实缺陷"。

## 已确认的事实

1. 隧道转发 `/rtc/…` 升级请求到 LiveKit 是通的：Node 裸 WS 打
   `wss://localhost:8081/rtc/v1?access_token=…` 时，LiveKit 日志记录
   `status:400 path:/rtc/v1 error:"join_request is required"`
   —— 请求已到达 LiveKit 并被其协议层解析。
2. 浏览器内 livekit-client 2.21.0 走 v1 路径（`/rtc/v1?…&join_request=…`）；
   LiveKit 侧出现 `removing participant without connection … reason:"JOIN_FAILED"`
   （clientInfo 显示 HeadlessChrome 156 / sdk 2.21.0），客户端侧约 120s 后报
   `could not establish signal connection: Abort handler called`。
3. 每一步浏览器操作耗时 30–130s（`[step]` 打点），`media-token` 等后端调用在浏览器里
   也慢到 20–40s，而同样调用用 curl 只需 3–5ms —— 瓶颈在浏览器进程被 CPU 饥饿。

## 最小复现

```bash
cd spikes/002-sfu-tunnel && npm i && npx playwright install chromium
node run.mjs
```

失败时 `result.json` 会记录：每步耗时、`urlFromBackend`/`urlUsed`、
以及浏览器实际使用的信令 URL 在"隧道"与"直连 LiveKit"两侧的 Node 复放结果
（`replayViaTunnel` / `replayDirect`）。

## 已排查（本轮踩过的坑，均已修）

- media-token 返回的 `livekit_url` 是局域网 IP `172.17.0.1:8081`，在本沙箱被 HTTP 代理
  拦截（403/挂起）→ runner 统一改写成页面同源 host，原值记入 `urlFromBackend`。
- Chromium profile 落在慢速 overlay 上：`newPage` 140s → 改用 tmpfs（`/run/secrets`）。
- `/dev/shm` 仅 64 MiB → 所有 runner 加 `--disable-dev-shm-usage`。
- headless shell 在负载下网络服务反复崩溃 → 改用完整 Chromium（`channel: "chromium"`）。
- 前端服务器只把 `GET /rtc/…`（带斜杠）识别为 LiveKit 升级：`GET /rtc`（livekit-client
  的 v0 路径）会落到后端 404（后端日志 `GET /rtc status=404`）。v1 路径不受影响。

## 下一步（待办）

1. 在正常配置的机器（≥4 vCPU、无代理拦截）重跑 `node run.mjs`。
2. 若仍失败：用 Node 侧 A/B 复放（`replayViaTunnel` vs `replayDirect`）对比，并在
   前端服务器与 LiveKit 之间抓包，确认 101 之后的帧是否有丢失。
3. 可选：前端服务器把 `/rtc`（无斜杠）也纳入 LiveKit 升级判定，兼容 livekit-client 的
   v0 路径。
