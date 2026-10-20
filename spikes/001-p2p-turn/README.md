# Spike 001 — P2P relay-only 建连（LiveKit 内嵌 TURN）

验证 `/api/v1/rooms/{code}/ice-servers` 签发的 LiveKit TURN 短期凭证，能否让
两个 `iceTransportPolicy: "relay"` 的 `RTCPeerConnection` 建连并传输 Opus 音频。
SDP 同页直接交换，隔离出"TURN 凭证 + relay 通路"这一个风险点。

## 结论（2026-10-18，baseline-20261018，LiveKit 1.13.5）

**FAIL：凭证算法正确、TURN 分配成功，但 relay↔relay 无法通连。**
当前实现下 P2P mesh 在"两端都必须走中继"（relay-only、对称 NAT 对对称 NAT）时不可用。

## 证据

`node run.mjs`（result.json）：

- `iceServers = ["turn:172.17.0.1:3478?transport=udp"]`，`log: ["ice",200,1]` → 凭证端点正常。
- `localCandidates = [{type:"relay", protocol:"udp", address:"172.17.0.1", port:50206, url:"turn:…:3478?transport=udp"}]`
  → TURN **Allocate 成功**，即后端 `b62(SHA256(secret|pid|expiry))` 的凭证被 LiveKit 接受
  （与 `livekit/pkg/service/turn.go:computePassword` 一致）。
- `iceConnectionState = "disconnected"`、`pairLocal/pairRemote = null`、`bytesReceived = 0`、`iceErrors = []`
  → 候选拿到了但中继数据被静默丢弃（无 ICE 错误）。

## 根因（LiveKit 内嵌 TURN 的策略，源码引用）

1. `pkg/service/turn.go` 的 `permissionHandler` 用 `isRestrictedPeerIP` 判定：
   私网地址（含 172.16.0.0/12）默认**拒绝 CreatePermission**，除非配置
   `turn.allow_restricted_peer_cidrs`。
2. `pkg/service/turnrelay.go` 的 `turnRelayPolicy.allows()`：当目标是本机 IP
   （`localIPs`，含 node IP）且端口不在 **RTC 媒体端口范围**（本环境 50000-50100）时，
   `turnRelayPacketConn.WriteTo` **静默丢弃**。而 relay↔relay 的目标端口是 TURN relay
   端口（本环境 50200-50250），必然落在范围外。

即：内嵌 TURN 只允许"中继 → 本机 SFU 媒体端口"的流量；两个客户端都用中继候选时，
A 的 relay 要向 B 的 relay（同一台 LiveKit 的 502xx 端口）发包，被策略丢弃。

## 影响

- 两端都需中继时（relay-only、对称 NAT）P2P 建连失败 → README 中
  "跨 NAT 使用 LiveKit 内嵌 TURN" 的能力边界需要在文档里写清。
- 单侧中继（对端是 host/srflx 公网候选）不受这两条策略影响，仍可能连上；本 spike 未覆盖。

## 复现

```bash
cd spikes/001-p2p-turn && npm i && npx playwright install chromium
node run.mjs      # 期望（当前实现）FAIL relay audio；明细见 result.json
```

## 未做的对照实验（待办）

- 自建 coturn / LiveKit standalone TURN 作为 relay 时是否可通（另开 spike 验证）。
- `turn.allow_restricted_peer_cidrs` 只解除第 1 条限制，第 2 条（媒体端口范围）仍会丢弃
  relay↔relay 包，故未单独做该对照。
