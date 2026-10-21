# 技术债务清单（2026-10-21）

- 收口分支：`refactor/app-batch8-admin`（已合并到 `main`）
- 合并目标：`main`
- 范围：只记录已知未修项；本轮不改业务代码。

> 收口验证：`scripts/check.sh` 结果 **16 PASS / 0 FAIL / 2 SKIP**（ESLint 未配置、cargo-audit 未安装）；`test-server/ui-voice-check.mjs` 实跑 PASS。未覆盖项见文末。

## 1. ~~前端构建 chunk > 500 kB~~（已完成）

- 入口 `dist/assets/index-*.js` 为 **13,652 B raw / 5,056 B gzip**；首屏 entry + `react-vendor` 合计 **177,499 B raw / 58,357 B gzip**。
- `Room`、`Admin`、`Settings` 已改为懒加载；静态首屏页面保持 eager。

## 2. 接口层直连基础设施（Batch 1-8 已完成）

- 统计：`grep -rF 'crate::infra::repo' backend/src/api | wc -l` = **95 处**（Batch 1 前 189；Batch 1-8 共上移 94 处）。
- 主要分布（`infra::repo`）：`api/ws.rs` 36、`api/rooms.rs` 20、`api/admin.rs` 15、`api/auth_routes.rs` 15、`api/me.rs` 3；`api/routes.rs`/`api/json.rs`/`api/extract.rs` 各 2。
- Batch 1 join membership、Batch 2 message send、Batch 3 room request creation、Batch 4 settings / member lifecycle、Batch 5 mutes / invite revocation、Batch 6 direct room、Batch 7 avatar、Batch 8 admin writes 已完成。
- 剩余：WS 36 个调用中 30 处多步事件延后；其余为合理直连（读取/单条/transport/auth）。
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

## 5. ~~离开/被踢成员仍可读取房间~~（已修复）

- `app::access::active_member_room` 已检查 `left_at`；离开/被踢后 HTTP 房间、消息、设置、成员、禁言、私聊、ICE/media-token 等端点统一返回 **403**。
- WS 新连接在快照阶段拒绝并关闭；已建立连接在下一次文本事件检测到 `hub.is_connected == false` 后关闭。
- join/rejoin：主动离开（`left_reason='left'` 或迁移前 NULL）仍可免邀请恢复；被移除（`left_reason='removed'`）必须使用 kick 之后创建的邀请，并受 `max_members` 限制。
- 迁移边界：迁移前的历史 left 行 `left_reason` 为 NULL，无法区分主动离开与被踢，统一按主动离开兼容；升级验证已确认该行为。
- 前端沿用现有 403 处理：下次加载进入非成员/邀请加入视图；404/410 显示错误与返回首页。

## 6. 已使用邀请撤销返回 200 但 no-op（既有，未修）

- `revoke_invite` 对已消费邀请仍返回 `{"ok":true}`；repo 的 `UPDATE ... WHERE consumed_at IS NULL` 实际不更新，`revoked_at` 保持 null。
- 影响：管理端可能误判撤销成功；已使用邀请本身已无法再次兑换。
- Batch 5 特征化测试锁定现状；是否改为 4xx（如 409 conflict / 422）需另行决定，并同步前端撤销成功的提示逻辑。

## 7. 房主转让后前任房主仍为 owner（既有，待评估）

- `transfer_owner` 只把目标用户设为 owner 并更新 `rooms.owner_user_id`，不降级前任 owner；`GET /rooms/{code}` 中两者的 `my_role` 都是 `owner`。
- Batch 8 测试已锁定现状。是否应把前任降为 member，涉及多 owner 语义、权限与前端展示，需单独评估。

## 已接受的并发/claim_code 语义（非待办）

- 直接房创建：整个 DB 编排在 `Db::with` 的单个 SAVEPOINT 内，并由全局 `Mutex<Connection>` 串行化；`claim_code` 是单条 `INSERT OR IGNORE`，不开启嵌套事务。
- 相同用户对的重复/并发请求命中 `direct_pairs`，返回同一房间（200），不是 409。
- 这是现有语义，Batch 6 原样保留，不视为缺陷；改动会涉及事务/并发契约，需独立决策。

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
