# Spike 002 — SFU 经 frontend-server `/rtc/` WSS 隧道

验证 livekit-client 通过 frontend-server 的 `/rtc/*` WSS 隧道连接 LiveKit，
并按 48kHz 采集、128kbps 发布，另一账号订阅到音频。

## 结论（2026-10-19，baseline-20261018）

**未判定（环境受限）**：曾在机器空闲时跑通一次完整端到端（见 Run A），
之后在 load 1.3–2.0 下连续 7 次卡在 "could not establish pc connection"。
无法在本沙箱区分"隧道在高负载下丢帧"与"浏览器 ICE/DTLS 超时"。

## 观测记录

**Run A（成功，load 0.42，全程 44s）** — `node run.mjs`：

```
[step] connect alice ok 4970ms
[step] connect bob ok 20527ms
[step] remote audio alice ok 4421ms   (bytesReceived=17847)
[step] remote audio bob ok 2007ms
[step] measure alice ok 3337ms
checks: connected=true, sampleRate48k=true, remoteAudioFlowing=true,
        bitrateInRange=false（measured 72.6 kbps，期望 96–160）
```

即：**同源 `/rtc/` 隧道 + LiveKit SFU 建连、48kHz 采集、双向音频都成立**；
唯一不达标的是码率——128kbps 是发布上限（`RTCRtpSender` 的 maxBitrate），
而假麦是 beep 音，Opus VBR 实测只用 ~73kbps，故"96–160kbps"这一断言口径不成立
（已改为断言"上限已下发 + 有音频流"）。

**Run B–H（失败，load 1.3–2.0）**：`connect alice`/`connect bob` 报
`ConnectionError: could not establish pc connection`；LiveKit 侧同时出现
`could not send message to participant … signal write failed`。
推断：信令 WS 帧（含 trickle ICE 候选）在负载下停摆 → PC 建不起来；
但未抓包证实。

## 已确认的事实

1. 隧道转发 `/rtc/…` 升级请求到 LiveKit 是通的：Node 裸 WS 打
   `wss://localhost:8081/rtc/v1?access_token=…` 时，LiveKit 记录
   `status:400 path:/rtc/v1 error:"join_request is required"`。
2. 浏览器内 livekit-client 2.21.0 走 v1 路径；Run A 证明其端到端可用。

## 最小复现

```bash
cd docs/archive/spikes/002-sfu-tunnel && npm i && npx playwright install chromium
node run.mjs      # 期望 PASS sfu tunnel 48k；失败时 result.json 记录每步耗时与诊断
```

## 已排查（本轮踩过的坑，均已修）

- media-token 的 `livekit_url` 是局域网 IP `172.17.0.1:8081`，在本沙箱被 HTTP 代理拦截
  → runner 改写成页面同源 host，原值记入 `urlFromBackend`。
- Chromium profile 落在慢速 overlay：`newPage` 140s → 改 tmpfs（`/run/secrets`）。
- `/dev/shm` 仅 64 MiB → `--disable-dev-shm-usage`。
- 前端服务器只把 `GET /rtc/…`（带斜杠）识别为 LiveKit 升级；`GET /rtc`（v0 路径）落到后端 404。
- 白噪声轨（`publishTrack` + `source: Microphone`）在 `bitrate/dtx` 下报
  `Cancelled publication by calling unpublish`，未继续追（改用假麦路径）。
- 建房接口有 20 次/分钟限流，连续重试会 429。

## 下一步（待办）

1. 在正常机器（≥4 vCPU、load < 1）复跑 `node run.mjs`。
2. 若仍失败：抓包对比 `/rtc/` 隧道与直连 LiveKit，确认 101 之后帧是否停摆。
3. 若要验证码率档位：用噪声/音乐等"难压缩"输入，或直接断言
   `sender.getParameters().encodings[0].maxBitrate`。
