# Spike 003 — @jitsi/rnnoise-wasm @ 48kHz AudioWorklet

验证 rnnoise WASM 在 48kHz AudioContext + AudioWorklet 上按 480 帧稳定处理。

## 结论（2026-10-18，baseline-20261018）

**PASS**（含 1 vCPU 沙箱的注意事项，见下）。

`node run.mjs` → `PASS rnnoise 48k (offline p95=4.6ms p50=1.3ms | …)`，exit 0。

| 检查 | 结果 | 证据（result.json） |
|---|---|---|
| 48kHz 上下文 | PASS | `sampleRate = 48000`（AudioContext 与 worklet 帧长 480） |
| worklet 通路无损 | PASS | passthrough：`outRms 0.11606 ≈ inRms 0.11606` |
| rnnoise 生效 | PASS | 白噪声输入 `inRms 0.11608` → 输出 `outRms 0`（噪声被抑制） |
| 每帧处理成本 | PASS | 离线紧循环 300 帧：`p50 1.3ms / p95 4.6ms / max 19.1ms`（10ms 预算内） |
| 无错误 | PASS | `errors = []`、无 console error |

## 实现要点

- 产物：最小 HTML（Vite 开发服务器，端口 5199）+ Playwright 断言。
- `main.js` 复刻 `frontend/src/hooks/denoise.ts` 的 worklet 与 WASM 加载逻辑；
  另加 `passthrough` 处理器作为对照，并暴露 `offline()` 只测 rnnoise 每帧耗时。
- 输入用**确定性白噪声缓冲**（AudioBufferSource）而非假麦：headless 下
  `--use-fake-device-for-media-stream` 的采样数据不可靠（实测 3s 内仅 2 帧）。
- worklet 必须接 `ctx.destination`（经 0 增益 GainNode）才会被持续拉取。

## 沙箱注意事项

- 实时 worklet 回路（worklet → 主线程 rnnoise → 回传）在 1 vCPU 沙箱下被饿死：
  3s 内只完成 2 帧（正常应为 ~300 帧），因此**实时链路的吞吐/时延未在此环境判定**；
  本 spike 的时延结论来自离线紧循环（同一 WASM、同一 480 帧）。
- 需在正常机器上复跑以确认实时链路吞吐。
