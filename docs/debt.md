# 技术债务清单（2026-10-21）

- 收口分支：`refactor/admin-tabs`（已合并到 `main`）
- 合并目标：`main`
- 范围：只记录已知未修项；本轮不改业务代码。

> 收口验证：`scripts/check.sh` 结果 **18 PASS / 0 FAIL / 0 SKIP**（ESLint 与 cargo-audit 已启用）；`test-server/ui-voice-check.mjs` 实跑 PASS。未覆盖项见文末。

## 1. ~~前端构建 chunk > 500 kB~~（已完成）

- 入口 `dist/assets/index-*.js` 为 **13,652 B raw / 5,056 B gzip**；首屏 entry + `react-vendor` 合计 **177,499 B raw / 58,357 B gzip**。
- `Room`、`Admin`、`Settings` 已改为懒加载；静态首屏页面保持 eager。

## 2. 接口层直连基础设施（Batch 1-8 已完成）

- 统计：`grep -rF 'crate::infra::repo' backend/src/api | wc -l` = **95 处**（Batch 1 前 189；Batch 1-8 共上移 94 处）。
- 主要分布（`infra::repo`）：`api/ws.rs` 36、`api/rooms.rs` 20、`api/admin.rs` 15、`api/auth_routes.rs` 15、`api/me.rs` 3；`api/routes.rs`/`api/json.rs`/`api/extract.rs` 各 2。
- Batch 1 join membership、Batch 2 message send、Batch 3 room request creation、Batch 4 settings / member lifecycle、Batch 5 mutes / invite revocation、Batch 6 direct room、Batch 7 avatar、Batch 8 admin writes 已完成。
- WS 30 处（RoomMute/Speak/Reaction/Presence）已评估，继续保持 `api/ws.rs`：事件顺序、广播与传输状态机耦合，不作为迁移待办；其余 WS 6 处为 auth/单条读取。新增 `ws_mute_speak_and_reaction_sequences` 锁定时序。
- 其余为合理直连（读取/单条/transport/auth）。
- 影响：接口层与 SQLite 仓储耦合；简单读写可接受，但多步用例绕过应用层会削弱可测试性。
- 建议：只把多步业务用例上移到 `app/`；简单单条读写保留，避免为分层制造空转封装。

## 3. 前端 Admin tabs 与消息列表已测；媒体 hooks 与 UI 冒烟 CI 未覆盖

- 已配置 Vitest 2.1.9 + jsdom 26.1.0；24 个 pure-logic + 31 个组件/hooks 用例（共 55）：App 角色导航/路由守卫、Register 邀请流程、Settings 密码校验、useRoomSocket 连接/重连/清理、useVoiceSession P2P/SFU 选择、MessageList 消息顺序/合并去重/时间/空态/系统消息/回复引用/长文本/交互回调、Admin tab 切换/邀请状态与 409/用户权限与空态。
- Room 消息列表与 Admin 邀请/用户 tab 均已抽为展示组件，不再列为未测；Room 整体（socket/语音集成）与媒体 hooks（useMicTest/useP2PVoice/useLiveKitVoice）仍无组件测试，也没有 UI 快照测试；按计划避免为覆盖率堆 mock。
- SFU 语音冒烟 `test-server/ui-voice-check.mjs` 已固化，但未接入 CI。
- 后端 WS 事件时序已有 `ws_mute_speak_and_reaction_sequences` 覆盖。
- 建议：后续评估媒体 hooks；UI 语音冒烟在具备 Chrome + 测试服务的 CI 环境中接入。

## 4. 未验证

- 生产 TLS：未验证真实证书、Nginx/TLS 终止、浏览器信任链与 WSS 同源路径。
- 真实麦克风：仅使用 Chrome fake device；未验证真实设备权限、回声消除/降噪与不同采样率。

## 5. ~~离开/被踢成员仍可读取房间~~（已修复）

- `app::access::active_member_room` 已检查 `left_at`；离开/被踢后 HTTP 房间、消息、设置、成员、禁言、私聊、ICE/media-token 等端点统一返回 **403**。
- WS 新连接在快照阶段拒绝并关闭；已建立连接在下一次文本事件检测到 `hub.is_connected == false` 后关闭。
- join/rejoin：主动离开（`left_reason='left'` 或迁移前 NULL）仍可免邀请恢复；被移除（`left_reason='removed'`）必须使用 kick 之后创建的邀请，并受 `max_members` 限制。
- 迁移边界：迁移前的历史 left 行 `left_reason` 为 NULL，无法区分主动离开与被踢，统一按主动离开兼容；升级验证已确认该行为。
- 前端沿用现有 403 处理：下次加载进入非成员/邀请加入视图；404/410 显示错误与返回首页。

## 6. ~~已使用邀请撤销返回 200 但 no-op~~（已修复）

- `app::invites::ensure_revocable` 在房主/管理员撤销前检查 `consumed_at` 与 `expires_at`：已使用/已过期返回 **409 conflict**，不修改记录；未使用且未过期的重复撤销仍返回 200（幂等）。
- 不存在、房间/类型不匹配仍为 404；响应结构未改。
- 前端管理页只在 `active` 邀请上显示撤销按钮，已有错误提示可展示 409；未新增前端字段。

## 7. ~~房主转让后前任房主仍为 owner~~（已修复）

- `transfer_owner` 现在先把目标以外的所有 owner 角色降为 member，再设置目标 owner 和 `rooms.owner_user_id`；标准房间只保留一个 owner。直接房仍保留双方 owner 的既有例外。
- 前端收到 `owner_transferred` 后调用 `loadRoom()`，双方角色、`my_role` 和能力即时刷新。
- 历史数据需按 `docs/plans/2026-10-21-owner-transfer.md` 的备份/更新/回滚 SQL 手动修正；当前工作区无部署数据库，未执行数据变更。
- 部署步骤：在部署库副本上先运行统计 SQL，确认 `owner_user_id IS NULL` 的标准房清单并创建备份表，再执行计划中的降级 SQL；本次未执行。直接房（`rooms.kind='direct'`）不受影响，保留双方 owner。

## 8. 发布验证遗留（2026-10-21）

- README 构建差异已文档化：README 开发命令生成非 PIE 静态二进制（本地验证 6,923,208 B、哈希与发布不同）；发布统一使用 `./scripts/build-musl.sh all`，产物 static-pie；`docs/deployment.md` 已注明两者差异。
- 自签 TLS 浏览器验证未通过：Chrome for Testing 156 未授权临时 CA 时显示 `ERR_CERT_AUTHORITY_INVALID`；尝试 system CA 与 Chrome profile NSS 导入后仍失败。未使用 `--ignore-certificate-errors` 伪造通过；生产 CA 证书仍未验证。`ui-voice-check.mjs` 的 ignore 参数只用于运行时语音冒烟，不代表 TLS 信任通过。
- `release/nginx/cloudroom.conf.example`、systemd unit 仅做静态检查；环境无 `nginx`/`systemd-analyze`，未做配置语法或实际启动验证。systemd unit 已删除不支持的 `ExecReload`，配置/二进制变更使用 `systemctl restart`；SIGHUP 热重载未实现。
- `release/install.sh` 通过 `bash -n`；未执行实际安装（避免改写系统）。部署文档已逐段检查，生产证书、Nginx/systemd、安全组与真实 LiveKit TURN 尚未实际部署验证。

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
