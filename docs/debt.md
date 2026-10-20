# 技术债务清单（2026-10-20）

- 收口分支：`refactor/app-batch1-join`（已合并到 `main`）
- 合并目标：`main`
- 范围：只记录已知未修项；本轮不改业务代码。

> 收口验证：`scripts/check.sh` 结果 **16 PASS / 0 FAIL / 2 SKIP**（ESLint 未配置、cargo-audit 未安装）；`test-server/ui-voice-check.mjs` 实跑 PASS。未覆盖项见文末。

## 1. ~~前端构建 chunk > 500 kB~~（已完成）

- 入口 `dist/assets/index-*.js` 为 **13,652 B raw / 5,056 B gzip**；首屏 entry + `react-vendor` 合计 **177,499 B raw / 58,357 B gzip**。
- `Room`、`Admin`、`Settings` 已改为懒加载；静态首屏页面保持 eager。

## 2. 接口层直连基础设施（Batch 1 已完成）

- 统计：`grep -rF 'crate::infra::repo' backend/src/api | wc -l` = **169 处**（Batch 1 前 189；join membership 上移 20 处）。
- 主要分布（`infra::repo`）：`api/rooms.rs` 73、`api/ws.rs` 36、`api/admin.rs` 32、`api/auth_routes.rs` 15、`api/me.rs` 7；其余 6 处位于 `extract`/`json`/`routes`。
- Batch 1：join membership 已完成；WS 30 处多步事件延后，待 WS 专项测试后再迁移。
- 影响：接口层与 SQLite 仓储耦合；简单读写可接受，但多步用例绕过应用层会削弱可测试性。
- 建议：只把多步业务用例上移到 `app/`；简单单条读写保留，避免为分层制造空转封装。

## 3. 前端仅覆盖纯逻辑；组件/hooks/DOM 未测；UI 语音冒烟未接入 CI

- 已配置 Vitest 2.1.9 + jsdom 26.1.0，24 个 pure-logic 用例覆盖 roomCode/roles/mergeMessages/time。
- 尚无组件、hooks、DOM 交互测试，也没有 UI 快照测试。
- SFU 语音冒烟 `test-server/ui-voice-check.mjs` 已固化，但未接入 CI。
- 建议：补组件/hooks 测试；UI 语音冒烟在具备 Chrome + 测试服务的 CI 环境中接入。

## 4. 未验证

- ~~干净 clone 重建~~（已完成：clean clone 中 `scripts/build-musl.sh backend` 构建通过并产出 static-pie）。
- 静态产物未在无 glibc 环境验证：当前仅在宿主 glibc 环境运行通过。
- 生产 TLS：未验证真实证书、Nginx/TLS 终止、浏览器信任链与 WSS 同源路径。
- 真实麦克风：仅使用 Chrome fake device；未验证真实设备权限、回声消除/降噪与不同采样率。

## 已接受的构建特征（非待办）

- LiveKit 懒加载 chunk `livekit-*.js` 约 **530 KB raw / 138 KB gzip**；Vite 的 >500 KB 警告为预期，不调整 `chunkSizeWarningLimit`；主入口仍 <500 KB。

## 已接受的 lint allow（9 条，非待办）

以下 lint 均有精确到方法/函数的 `#[allow(clippy::xxx)]`，无模块级/全局 allow；`scripts/check.sh` 已固定 `-D warnings`。

| lint | 位置 | 接受理由 |
|---|---|---|
| `clippy::should_implement_trait` ×5 | `backend/src/domain/models.rs` 的 5 个 `from_str` | 现有方法返回 `Option<Self>`；实现 `FromStr` 会改变公开错误契约 |
| `clippy::manual_async_fn` ×2 | `backend/src/api/extract.rs` 的 2 个 `from_request_parts` | trait 签名显式使用 `impl Future`；改成 `async fn` 会改变 impl 签名 |
| `clippy::too_many_arguments` ×1 | `backend/src/infra/repo.rs::update_room_settings` | 现有公开仓储签名；参数分组为 struct 属于单独 API 改动 |
| `clippy::type_complexity` ×1 | `backend/src/infra/repo.rs::message_reactions` | 现有返回契约；类型别名/重构属于单独 API 改动 |
