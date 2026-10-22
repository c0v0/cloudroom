# Spike 003 — @jitsi/rnnoise-wasm @ 48kHz AudioWorklet

验证 rnnoise WASM 在 48kHz AudioContext + AudioWorklet 上按 480 帧稳定处理。

## 结论（2026-10-19，baseline-20261018）

**功能 PASS；时延未判定（1 vCPU 沙箱不可信）。**

| 检查 | 结果 | 证据（result.json） |
|---|---|---|
| 48kHz 上下文 | PASS | `sampleRate = 48000`（AudioContext 与 worklet 帧长 480） |
| worklet 通路无损 | PASS | passthrough：`outRms 0.11557 ≈ inRms 0.11602` |
| rnnoise 生效 | PASS | 白噪声输入 `inRms 0.11603` → 输出 `outRms 0` |
| 无错误 | PASS | `errors = []`（除下述一次负载下的 console error） |
| 每帧处理成本 | **未判定** | 同一份代码：空闲时 300 帧紧循环 `p50 1.3ms / p95 4.6ms`（exit 0）；load ≈1.0–1.2 时 `p95 17.6–25.3ms / max 57.9ms`（exit 1） |

时延差异完全来自沙箱 CPU 争用（omp 常驻 ~20% CPU、1 vCPU），不是 rnnoise 本身；
需在正常机器上复跑才有结论。功能性结论（48kHz 通路 + 降噪生效）在两种负载下一致。

## 实现要点

- 产物：最小 HTML（Vite 开发服务器，端口 5199）+ Playwright 断言。
- `main.js` 复刻 `frontend/src/hooks/denoise.ts` 的 worklet 与 WASM 加载逻辑；
  另加 `passthrough` 处理器作对照，并暴露 `offline()` 只测 rnnoise 每帧耗时。
- 输入用**确定性白噪声缓冲**（AudioBufferSource）而非假麦：headless 下
  `--use-fake-device-for-media-stream` 的采样数据不可靠（实测 3s 内仅 2 帧）。
- worklet 必须接 `ctx.destination`（经 0 增益 GainNode）才会被持续拉取。

## 运行

```bash
cd docs/archive/spikes/003-rnnoise-48k && npm i && npx playwright install chromium
node run.mjs   # PASS 时 stdout: PASS rnnoise 48k (offline p95=…ms …)，exit 0
```
