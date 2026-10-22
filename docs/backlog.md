# 待办（backlog）

> 规则（AGENTS.md §1.5）：范围外发现的问题只记这里，不顺手修/重构/升级依赖。
> 格式：`- [ ] 问题 — 影响 — 证据（命令或 file:line）— 建议`

## 工程

- [ ] `release/livekit-server` 是第三方二进制（约 51MB，未入库）— 干净机器上 `test-server/run-tests.sh` 与常驻测试环境不可用（`scripts/check.sh` 会 SKIP 集成测试）— 证据：`.gitignore` 忽略该文件 — 建议：在 `release/部署安装指南.md` 固定版本号 + sha256，或补下载脚本

## 产品 / 媒体

- （空）

## 已完成

- [x] 2026-10-22 前端 ESLint 门禁：`eslint src --max-warnings=0`，js/ts 推荐规则 + `react-hooks`，`exhaustive-deps=error`；`scripts/check.sh` 缺配置或出现 warning 即失败。初次 9 problems（6 errors / 3 warnings）已清零。
- [x] 2026-10-22 Rust 依赖漏洞门禁：安装 `cargo-audit 0.22.2`，`scripts/check.sh` 同时扫描 backend 与 frontend-server 的锁文件；h2/quinn-proto/rustls/rustls-webpki/webpki-root-certs 已升级到修复版本。
- [x] 2026-10-21 修正文档中的 `test-server/run-tests.sh` 旧问题：当前脚本只使用已定义的 `ok`/`bad`/`check`，无未定义 `pass`/`fail`；`./run-tests.sh` 实跑 64 通过 / 0 失败。
- [x] 2026-10-19 工程底座：as-is 快照 tag、密钥扫描（工作区 + 全历史）、`scripts/check.sh` 一键验收、`scripts/repro.sh` 干净复现、`scripts/smoke.sh` 冒烟骨架、本地 hooks、docs 骨架、AGENTS.md 第 2 节
