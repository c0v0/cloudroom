# spikes — 归档的媒体链路可行性验证

三个独立 spike，验证 baseline-20261018 上"半成品"状态中最不确定的媒体链路。
每个 spike 只依赖 `test-server/start-test-env.sh` 拉起的本地环境（release/ 预编译产物），
不改 app 代码；判定结果写在各目录的 `result.json`。本目录已归档，不再扩展。

| # | 目录 | 验证内容 | 运行命令 | 结论 |
|---|---|---|---|---|
| 1 | `001-p2p-turn/` | `/ice-servers` 的 LiveKit TURN 凭证 + relay-only 建连 + Opus 收包 | `node run.mjs` | **FAIL**：分配成功但 relay↔relay 被 LiveKit 内嵌 TURN 策略丢弃（见目录 README） |
| 2 | `002-sfu-tunnel/` | livekit-client 经 frontend-server `/rtc/` 隧道，48kHz/128kbps 发布订阅 | `node run.mjs` | **未判定**：空闲时跑通一次双向音频，之后负载下连续 7 次卡在 PC 建连（见目录 README） |
| 3 | `003-rnnoise-48k/` | @jitsi/rnnoise-wasm 在 48kHz AudioWorklet 稳定处理 | `node run.mjs` | **功能 PASS / 时延未判定**：48kHz 通路无损、噪声被抑制；离线 p95 空闲 4.6ms、负载下 17.6–25.3ms |

## 重跑命令

从仓库根执行；002 需要先启动 `test-server/start-test-env.sh`，003 会自行拉起 Vite：

```bash
cd docs/archive/spikes/001-p2p-turn && node run.mjs
cd docs/archive/spikes/002-sfu-tunnel && node run.mjs
cd docs/archive/spikes/003-rnnoise-48k && node run.mjs
```

前置（每个 spike 目录内）：

```bash
npm i
npx playwright install chromium   # 本环境 CDN 下载失败时见下
```

本环境（沙箱）实测：`npx playwright install chromium` 走 `cdn.playwright.dev` 会中断，
可改用直连 Chrome for Testing 的 Google CDN 后手动解包：

```bash
curl -sSL -o /tmp/chs.zip https://storage.googleapis.com/chrome-for-testing-public/156.0.8078.4/linux64/chrome-headless-shell-linux64.zip
python3 -c "import zipfile;zipfile.ZipFile('/tmp/chs.zip').extractall('$HOME/.cache/ms-playwright/chromium_headless_shell-1248')"
touch "$HOME/.cache/ms-playwright/chromium_headless_shell-1248/INSTALLATION_COMPLETE"
sudo npx playwright install-deps chromium
```

测试环境：`test-server/start-test-env.sh`（前台常驻，Ctrl-C/`stop` 关闭），
健康检查 `curl -sk https://localhost:8081/api/v1/health` 应为
`{"checks":{"livekit":"configured","sqlite":"ok"},"status":"ok"}`。

## 2026-10-22 重跑结果

测试环境健康检查为 `{"checks":{"livekit":"configured","sqlite":"ok"},"status":"ok"}`，但两个浏览器实验均在启动阶段被环境阻塞，未产生新的媒体结论，原有 `result.json` 保持不变：

```text
002: node run.mjs
browserType.launchPersistentContext: Executable doesn't exist at
/home/agent/.cache/ms-playwright/chromium_headless_shell-1248/chrome-headless-shell-linux64/chrome-headless-shell

003: node run.mjs
Fail rnnoise 48k: same missing chromium_headless_shell-1248
```

原因：Playwright 默认寻找 headless shell；本机只有 `chromium-1248/chrome-linux64/chrome` 完整浏览器，runner 未覆盖 `executablePath`。修复方式：按脚本前置执行 `npx playwright install chromium`，或给 runner 显式指定已安装的 Chrome。测试环境已在重跑后停止。

## 本沙箱的实测限制（影响运行时间与稳定性）

- 1 vCPU / 约 1.1 GiB 内存，load average 长期 1.0：Chromium 首次启动约 50s，
  每个 `page.evaluate` 步骤 1–120s 不等；`run.mjs` 已把超时放大并在 stdout 打点。
- `/` 是慢速 overlay：Chromium profile 必须放 tmpfs（`run.mjs` 用
  `mkdtempSync("/run/secrets/pw-")`），否则 `newPage` 要 2 分钟以上。
- `/dev/shm` 只有 64 MiB：所有 runner 都带 `--disable-dev-shm-usage`。
- 前端服务器 `--rtc-target` 只在 `GET /rtc/…`（带斜杠）时把升级请求原样转发给
  LiveKit；`GET /rtc`（无斜杠，livekit-client 的 v0 路径）会落到后端并 404。

