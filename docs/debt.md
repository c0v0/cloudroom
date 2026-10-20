# 技术债务清单（2026-10-20）

- 收口分支：`fix/me-rooms-500` @ `cbb3f3e`（已合并到 `main`）
- 合并目标：`main`
- 范围：只记录已知未修项；本轮不改业务代码。

> 收口前置确认：编译门、单测/集成、运行时（HTTP/WS/DB）、语音（token/101/connected/publish/leave）、前端主路径均已覆盖；本轮只做文档收口，未重跑。未覆盖项见文末。

## 1. Clippy 警告

- `backend`：**37 条**（`cargo clippy --all-targets --locked --message-format=short`，不含汇总行）。
- `frontend-server`：**1 条**。
- 影响：`scripts/check.sh` 默认使用 `-D warnings`，当前 clippy 门会失败；CI 严格模式下不可用。
- 建议：先以 `CLIPPY_DENY=0` 保留基线，再按 lint 分类分批清零；不要用 `#[allow]` 批量掩盖。

## 2. 前端构建 chunk > 500 kB

- 当前构建产物：`frontend/dist/assets/index-ByKcRgdy.js` 约 **775 KB**（gzip 约 221.5 KB）；Vite 输出 `chunks are larger than 500 kB` 警告。
- 影响：首屏加载体积偏大；弱网/移动端体验下降。
- 建议：对 LiveKit、rnnoise/管理页做动态 `import()`；必要时配置 `manualChunks`，目标单 chunk < 500 KB。

## 3. 接口层直连基础设施

- 统计：`grep -rF 'crate::infra::repo' backend/src/api | wc -l` = **189 处**（收口前 188；本次修复新增 1 处 `room_cols` 引用。全量 `crate::infra` 为 197 处）。
- 主要分布（`infra::repo`）：`api/rooms.rs` 93、`api/ws.rs` 36、`api/admin.rs` 32、`api/auth_routes.rs` 15、`api/me.rs` 7；其余 6 处位于 `extract`/`json`/`routes`。
- 影响：接口层与 SQLite 仓储耦合；简单读写可接受，但多步用例绕过应用层会削弱可测试性。
- 建议：只把多步业务用例上移到 `app/`；简单单条读写保留，避免为分层制造空转封装。

## 4. 发布产物未使用 musl

- `rustup target list --installed` 仅显示 `x86_64-unknown-linux-gnu`，缺少 `x86_64-unknown-linux-musl`。
- `release/cloudroom-server` 当前为 GNU 动态链接；`ldd` 依赖 `libgcc_s.so.1`、`libm.so.6`、`libc.so.6`、`ld-linux-x86-64.so.2`。
- 影响：与 `docs/deployment.md` 的静态 musl 发布预期不一致；目标机 glibc 版本不匹配时可能无法启动。
- 建议：安装 musl target 与 `musl-gcc`，在 `scripts/build-release.sh`/CI 中固定 `--target x86_64-unknown-linux-musl`，并检查 `ldd` 输出 `not a dynamic executable`。

## 5. 前端缺测试框架，语音回归未固化

- `frontend/package.json` 只有 `dev/build/typecheck/preview`，没有 `test`/`lint`。
- 本轮 SFU 语音回归使用临时 CDP 脚本，未入库；无法在 CI 中自动防止“加入语音无响应”回归。
- 建议：引入 Vitest（单元）或 Playwright（浏览器）测试；至少固化“SFU 房间点击加入 → media-token 200 → `/rtc/` 101 → connected → leave 断开”的用例。

## 6. 未验证

- 干净 clone 重建：未从空目录按 README 完整复现依赖安装、构建、测试与启动。
- 生产 TLS：未验证真实证书、Nginx/TLS 终止、浏览器信任链与 WSS 同源路径。
- 真实麦克风：仅使用 Chrome fake device；未验证真实设备权限、回声消除/降噪与不同采样率。
