# 技术债务清单（2026-10-20）

- 收口分支：`refactor/layered-structure` @ `10e46af`
- 合并目标：`main`
- 范围：只记录已知未修项；本轮不改业务代码。

> 收口前置确认：编译门、单测/集成、运行时（HTTP/WS/DB）、语音（token/101/connected/publish/leave）、前端主路径均已覆盖；本轮只做文档收口，未重跑。未覆盖项见文末。

## 1. 既有缺陷：`GET /api/v1/me/rooms` 500

- 现象：登录后获取“我的房间”返回 500，浏览器控制台出现 `Failed to load resource: 500`。
- 复现：登录态 `curl http://127.0.0.1:8080/api/v1/me/rooms` 返回：
  `{"error":{"code":"internal_error","message":"Invalid column type Integer at index: 14, name: last_seq"}}`
- 根因：`backend/src/api/me.rs:217-229` 的 SELECT 只有 17 列，缺少 `announcement`、`max_members`；`backend/src/infra/repo.rs:315-338` 的 `row_to_room` 按 18 列读取，索引 14 期望 `announcement`，实际取到 `last_seq` INTEGER。
- 归因：基线 `923e216` 已存在相同 SELECT 与 `row_to_room` 调用，**非本次分层重构引入**。
- 影响：Home 房间列表失败；不影响直接打开房间 URL，但会污染控制台日志。
- 建议：补 SELECT 的 `announcement, max_members`，或复用 `ROOM_COLS`；补一条 `/api/v1/me/rooms` 回归测试。

## 2. Clippy 警告

- `backend`：**37 条**（`cargo clippy --all-targets --locked --message-format=short`，不含汇总行）。
- `frontend-server`：**1 条**。
- 影响：`scripts/check.sh` 默认使用 `-D warnings`，当前 clippy 门会失败；CI 严格模式下不可用。
- 建议：先以 `CLIPPY_DENY=0` 保留基线，再按 lint 分类分批清零；不要用 `#[allow]` 批量掩盖。

## 3. 前端构建 chunk > 500 kB

- 当前构建产物：`frontend/dist/assets/index-ByKcRgdy.js` 约 **775 KB**（gzip 约 221.5 KB）；Vite 输出 `chunks are larger than 500 kB` 警告。
- 影响：首屏加载体积偏大；弱网/移动端体验下降。
- 建议：对 LiveKit、rnnoise/管理页做动态 `import()`；必要时配置 `manualChunks`，目标单 chunk < 500 KB。

## 4. 接口层直连基础设施

- 统计：`grep -rF 'crate::infra::repo' backend/src/api | wc -l` = **188 处**（全量 `crate::infra` 为 196 处，另含 `auth`/`avatars`/`livekit` 等适配器）。
- 主要分布：`api/rooms.rs` 95、`api/ws.rs` 36、`api/admin.rs` 32、`api/auth_routes.rs` 15、`api/me.rs` 11。
- 影响：接口层与 SQLite 仓储耦合；简单读写可接受，但多步用例绕过应用层会削弱可测试性。
- 建议：只把多步业务用例上移到 `app/`；简单单条读写保留，避免为分层制造空转封装。

## 5. 发布产物未使用 musl

- `rustup target list --installed` 仅显示 `x86_64-unknown-linux-gnu`，缺少 `x86_64-unknown-linux-musl`。
- `release/cloudroom-server` 当前为 GNU 动态链接；`ldd` 依赖 `libgcc_s.so.1`、`libm.so.6`、`libc.so.6`、`ld-linux-x86-64.so.2`。
- 影响：与 `docs/deployment.md` 的静态 musl 发布预期不一致；目标机 glibc 版本不匹配时可能无法启动。
- 建议：安装 musl target 与 `musl-gcc`，在 `scripts/build-release.sh`/CI 中固定 `--target x86_64-unknown-linux-musl`，并检查 `ldd` 输出 `not a dynamic executable`。

## 6. 前端缺测试框架，语音回归未固化

- `frontend/package.json` 只有 `dev/build/typecheck/preview`，没有 `test`/`lint`。
- 本轮 SFU 语音回归使用临时 CDP 脚本，未入库；无法在 CI 中自动防止“加入语音无响应”回归。
- 建议：引入 Vitest（单元）或 Playwright（浏览器）测试；至少固化“SFU 房间点击加入 → media-token 200 → `/rtc/` 101 → connected → leave 断开”的用例。

## 7. 未验证

- 干净 clone 重建：未从空目录按 README 完整复现依赖安装、构建、测试与启动。
- 生产 TLS：未验证真实证书、Nginx/TLS 终止、浏览器信任链与 WSS 同源路径。
- 真实麦克风：仅使用 Chrome fake device；未验证真实设备权限、回声消除/降噪与不同采样率。
