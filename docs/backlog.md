# 待办（backlog）

> 规则（AGENTS.md §1.5）：范围外发现的问题只记这里，不顺手修/重构/升级依赖。
> 格式：`- [ ] 问题 — 影响 — 证据（命令或 file:line）— 建议`

## 工程

- [ ] 前端缺 lint 门禁（无 ESLint 配置）— 只有类型层检查（`tsc -b`），风格/规则类问题无机器检查 — 证据：`frontend/package.json` 无 `lint` 脚本 — 建议：引入 `eslint` + `typescript-eslint` 后把 `npm run lint` 接进 `scripts/check.sh`（check.sh 已实现“检测到即运行”）
- [ ] Rust 侧依赖漏洞/许可门禁依赖外部工具 — `cargo-audit` / `cargo-deny` 未安装，`scripts/check.sh` 对应步骤 SKIP — 证据：`cargo audit --version` 报 command not found — 建议：`cargo install cargo-audit cargo-deny --locked`（安装前先问，属全局工具）
- [ ] `release/livekit-server` 是第三方二进制（约 51MB，未入库）— 干净机器上 `test-server/run-tests.sh` 与常驻测试环境不可用（`scripts/check.sh` 会 SKIP 集成测试）— 证据：`.gitignore` 忽略该文件 — 建议：在 `release/部署安装指南.md` 固定版本号 + sha256，或补下载脚本

## 产品 / 媒体

- （空）

## 已完成

- [x] 2026-10-21 修正文档中的 `test-server/run-tests.sh` 旧问题：当前脚本只使用已定义的 `ok`/`bad`/`check`，无未定义 `pass`/`fail`；`./run-tests.sh` 实跑 64 通过 / 0 失败。
- [x] 2026-10-19 工程底座：as-is 快照 tag、密钥扫描（工作区 + 全历史）、`scripts/check.sh` 一键验收、`scripts/repro.sh` 干净复现、`scripts/smoke.sh` 冒烟骨架、本地 hooks、docs 骨架、AGENTS.md 第 2 节
