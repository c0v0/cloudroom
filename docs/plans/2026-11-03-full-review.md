# 2026-11-03 全量功能验证与竞品对标

范围：`backend/`、`frontend/`、`release/`、`test-server/`、`docs/`。验证环境：Linux + Chrome headless + LiveKit（测试环境），生产部署形态未变。

## 1. 验证结果（本次实跑）

| 项 | 结果 |
|---|---|
| `scripts/check.sh` | 18 PASS / 0 FAIL / 0 SKIP |
| 后端单元 + e2e | 41 + 35 |
| 前端 | tsc / eslint / vitest 66 用例 |
| 控制面 `run-tests.sh` | 64/64 |
| 浏览器全流程 `full-e2e` | 47/47 |
| 浏览器功能探针 | 26/26（编辑、删除、搜索、附件上传/下载/配额、公告、未读、E2EE、未开启附件 403） |
| 浏览器界面探针 | 26/26（首页建房、房间入口、语音栏、录音、屏幕共享降级提示、回复、搜索、公告、E2EE、成员、个人设置、管理页、移动端、可访问性、控制台零异常） |

### 1.1 界面探针覆盖

首页建房表单、房间头部入口、消息收发/回复/搜索、附件上传与图片渲染、公告换行/链接/关闭、E2EE 开关、成员面板、管理页四个标签、个人设置项、移动端 375px 无横向溢出、控制台零异常。

## 2. 本次修复（已合入）

| 级别 | 问题 | 位置 |
|---|---|---|
| P0 | 发消息时 `get_member(...).unwrap()` / `list_messages(...).pop().unwrap()`：访客升级与发送并发时可 panic，`panic=abort` 会杀掉整个进程（所有房间） | `backend/src/app/messages.rs` |
| P1 | 附件配额 TOCTOU：并发上传可突破房间配额 | `backend/src/api/rooms.rs`、`infra/repo.rs`（改为带条件的单条 UPDATE） |
| P1 | 静音后离开再重连：UI 显示"取消静音"但麦克风实际在采集（隐私） | `frontend/src/hooks/useLiveKitVoice.ts`、`useP2PVoice.ts` |
| P1 | 静音/取消静音走 `setMicrophoneEnabled`，重开设备导致 rnnoise/码率/DTX 丢失 | 同上（改为 `track.enabled`） |
| P1 | rnnoise worklet 把 480 帧写入 128 帧渲染量子 → `RangeError`，降噪模式静音 | `frontend/src/hooks/denoise.ts`（跨量子队列） |
| P1 | 录音在离开语音/离开页面后无法停止，持续缓存分片 | `frontend/src/pages/Room.tsx` |
| P1 | WS 重连闭包跨 effect 存活，旧 socket 可覆盖新连接并连回旧房间 | `frontend/src/hooks/useRoomSocket.ts` |
| P1 | LiveKit join 失败/被动断开未释放 Room、AudioContext、定时器 | `frontend/src/hooks/useLiveKitVoice.ts` |
| P1 | 屏幕共享无任何样式，视频按原始尺寸撑破聊天列 | `frontend/src/styles.css` |
| P1 | 回复时聚焦到隐藏的文件选择框 | `frontend/src/pages/Room.tsx` |
| P0 | Nginx 模板没有 `/rtc` 反代：SFU 语音与屏幕共享无法连接 | `release/nginx/cloudroom.conf.example` |
| P1 | CSP `script-src 'self'` 会禁用 rnnoise WASM、AudioWorklet(blob)、头像裁剪预览 | 同上 |
| P1 | Nginx `client_max_body_size 3m` < 管理员附件 5 MiB | 同上（8m） |
| P1 | 屏幕共享失败被静默吞掉（浏览器拒绝/无屏幕时无任何提示） | `useLiveKitVoice.ts`、`Room.tsx`（抛出可读错误并显示） |
| P1 | 屏幕共享状态不与浏览器"停止共享"同步，按钮卡在"停止共享" | `useLiveKitVoice.ts`（LocalTrackPublished/Unpublished） |
| P1 | bootstrap 令牌先消费后建号：用户名冲突会永久烧掉首个超管令牌 | `backend/src/api/auth_routes.rs`（消费移入建号事务） |
| P1 | `unmuted` 广播无目标：所有人都会看到"您已被解除禁言"；被禁言者 UI 不刷新 | `app/rooms.rs`、`api/rooms.rs`、`Room.tsx` |
| P2 | `features.screen_share` 文档/脚本仍写 `false` | `docs/security.md`、`release/deployment-zh.md`、`test-server/sfu-verify.sh` |

## 3. 未修复的已知问题（按优先级）

### P1

1. **离开后可无限次无邀请重进**，且房主无法再移除（`app/rooms.rs:246-249`、`:515-520`）。缺少封禁/黑名单原语。
2. **WS 事件循环里每条事件 1–6 次同步 DB 调用**（`api/ws.rs`），与全局 `Mutex<Connection>` 叠加，是单机吞吐上限。
3. **消息补齐上限 200 且合并只追加**（`Room.tsx:193-211`、`lib/messages.ts`）：长断线后永久缺口；补历史/搜索结果会插到列表末尾。
4. **@提及只高亮不通知**（`Room.tsx:274-281`）：桌面通知不区分提及/回复（回复有单独分支）。
5. **键盘无法触达消息操作**（`styles.css:873-883` hover-only）、**弹窗无 dialog 语义/焦点陷阱/Escape**、**说话状态仅用颜色**、**浅色主题顶栏对比度 ≈1.1:1**。
6. **P2P 协商无 perfect-negotiation**（`useP2PVoice.ts:247-276`），同时加入可能 glare。
7. **LiveKit 对 P2P 房间也会加载**（≈530 kB raw / 138 kB gzip），rnnoise 同样静态引入。

### P2（以下已在 2026-11-03 优化批次中处理）

- `install.sh` 重复执行会覆盖运维改过的 nginx/systemd；升级无 `.prev` 回滚。**已修**：存在即跳过并写 `.example`，二进制保留 `.prev`。
- systemd 加固补充 `PrivateDevices`/`ProtectClock`/`RestrictNamespaces`/`MemoryDenyWriteExecute` 等；`LimitNPROC` 提到 4096。**已修**。
- LiveKit 模板改为 `port_range_start/end: 50000-50100`，并文档化 `node_ip`。**已修**。
- Nginx：WS 独立 location（3600s）与普通 API（60s）分离；`/assets/` immutable 长缓存；`listen ... http2` 改为 `http2 on`。**已修**。
- 备份命令加 `umask 077`，补充升级/回滚章节。**已修**。
- 健康检查 SQLite 故障返回 503。**已修**。
- 附件删除消息不回收配额与文件（房间配额只增不减，直到删房）。**未修**（需要附件引用计数）。
- 无 Prometheus/结构化指标。**未修**。
- `react-router-dom` 2 个 moderate 通告（SSR hydration / Link 反斜杠重定向），本 SPA 不可达；修复需升 v7 大版本。**未修**。

## 3.1 优化批次（2026-11-03 晚）

| 项 | 变更 |
|---|---|
| 提及通知 | `@昵称` 命中时标题为「提到了你」，房间静音仍提醒；与回复通知互斥，不重复弹 |
| 设备选择 | 个人设置「声音」新增麦克风/扬声器下拉；`setSinkId` 即时生效，麦克风变化在语音中重建发布；设备被拔出时回退系统默认 |
| 消息一致性 | `mergeMessages` 按 `seq` 排序并接受编辑/删除/回应更新；断线补齐循环到 `hello.last_seq`（最多 20 页） |
| 可访问性 | 消息操作改 `opacity`（可聚焦）+ `:focus-within`；个人设置/房间详情弹窗 `role=dialog` + 焦点陷阱 + Esc + 焦点恢复；消息区 `role=log`；成员头像 `aria-label` 含在线/说话状态；浅色主题顶栏与状态色改用变量 |
| 渲染性能 | 说话电平仅在变化 >0.02 时刷新（150ms）；提及正则按成员列表缓存；`MessageItem` memo |
| 后端可观测性 | `GET /health` 在 SQLite 故障时返回 503；WS 连接数超限返回 1008 关闭帧 |

## 4. 竞品对标（2026-11）

对标对象：Jitsi Meet、Element Call、Discord 语音频道、LiveKit 原生能力。

| 能力 | CloudRoom | Jitsi | Element Call | Discord | 结论 |
|---|---|---|---|---|---|
| 语音房间 | ✅ P2P + SFU | ✅ | ✅ | ✅ | 持平 |
| 屏幕共享 | ✅ 仅 SFU | ✅ | ✅ | ✅ | P2P 缺 |
| E2EE | ✅ 可选（服务端持钥） | ✅ 可选 | ✅ 默认 | ❌ | 模型弱于 Element |
| 录制 | ✅ 本地混音 | ✅ Jibri | ⚠️ | ⚠️ | 够用 |
| 文字消息 | ✅ | ✅ | ✅ | ✅ | 持平 |
| 消息编辑/删除/搜索 | ✅ | ⚠️ | ✅ | ✅ | 持平 |
| 附件 | ✅ 图片/PDF（申请制） | ✅ | ✅ | ✅ | 持平 |
| @提及通知 | ❌ 仅高亮 | ✅ | ✅ | ✅ | **缺** |
| 正在输入/已读 | ❌ | ✅ | ✅ | ✅ | **缺** |
| 举手/语音表情 | ❌ | ✅ | ✅ | ✅ | **缺** |
| 设备选择（麦克风/扬声器） | ❌ | ✅ | ✅ | ✅ | **缺** |
| 推送/PWA 离线 | ❌ | ✅ | ✅ | ✅ | **缺** |
| 房间密码/等候室 | ❌（一次性邀请） | ✅ | ✅ 大厅 | ✅ | 部分替代 |
| 日历/预约 | ❌ | ✅ | ✅ | ✅ | 非目标 |
| 线程/转发 | ❌ | ❌ | ✅ | ✅ | 可选 |
| Markdown/代码块 | ❌ | ⚠️ | ✅ | ✅ | **缺** |
| 实时字幕/AI 摘要 | ❌ | ✅ 插件 | ⚠️ | ✅ | 远期 |
| 国际化 | ❌ 仅中文 | ✅ | ✅ | ✅ | **缺** |
| 多机水平扩展 | ❌ 单机 SQLite + 内存 Hub | ✅ | ✅ 多 SFU | ✅ | 架构边界 |

来源：Jitsi 2026 功能清单（jitsi.expert）、Element Call 2025-12 发布说明（element.io）、LiveKit 文档。

## 5. 建议的下一步（按投入产出）

1. ~~提及通知 + 通知去重~~、~~设备选择器~~、~~消息按 `seq` 合并 + 补齐循环~~、~~弹窗语义/键盘可达/`role=log`~~、~~bootstrap 令牌同事务~~、~~部署包补齐~~ —— 均已在 2026-11-03 优化批次完成。
2. **LiveKit 动态加载**：P2P 房间不再下载 530 kB（`useVoiceSession` 静态引入 `livekit-client`）。
3. **房间封禁表**：一条 `room_bans` 迁移 + join 校验，解决"离开后可无邀请重进"。
4. **WS/DB 并发**：把每个 WS 事件的多次同步 DB 调用合并为一次事务，评估连接池替换全局 `Mutex<Connection>`。
5. **附件配额回收**：删除消息/房间时按引用计数回收文件与 `attachment_bytes`。
6. **可观测性**：Prometheus 指标端点 + 结构化访问日志字段。
7. **新功能**（需产品确认）：正在输入/已读、举手、房间密码/等候室、PWA/推送、Markdown、线程。

## 6. 2026-11-04 收口批次

已合入（提交见 git log）：

| 项 | 内容 |
|---|---|
| 房间封禁 | 迁移 0012 `room_bans`；移出即封禁；无邀请重进 403；有效邀请兑换后解封 |
| 房主转让 | 校验 `room.state == active`，并 bump `last_seq` |
| 短 id | `util::short_id` 替代 `&id[..8]`，非 ASCII/短 id 不再 panic |
| 房间号生成 | 防除零；随机字节一次系统调用 |
| 附件回收 | 删除消息时若无其它引用：删记录、回退配额、删文件 |
| 孤儿文件 | sweeper 清理无 DB 记录且 >24h 的附件文件 |
| 推送清理 | 禁用用户时删除其推送订阅 |
| Hub 索引 | `user→rooms` 索引，登出不再全房间扫描 |
| 社交限流 | typing 1.5s / hand 300ms / 语音表情 500ms |
| API 超时 | 前端 20s 超时 + 401 全局登出 |
| 邀请自动加入 | ref 防重（StrictMode/手动点击竞态） |
| 跨房间重置 | SPA 切换 `/:code` 时清空消息/成员/草稿/搜索/已读 |
| 重连快照 | `hello` 后刷新房间设置/公告/禁言 |
| 时钟校准 | 在线状态用 `hello.now` 校准，不再依赖本机时钟 |
| 降噪资源 | `DenoiseEngine` 关闭/重绑时停止采集轨道 |
| 发言扇出 | 授权/取消改为本地更新 grants，不再全员 GET |
| LiveKit 懒加载 | `join()` 内动态导入，P2P 房间不下载 ~530 kB |
| P2P 健壮性 | offer 失败清理半成品；glare 先 rollback |
| e2e | 消息编辑/删除/搜索；附件开关/上传下载；E2EE 密钥 |
| 双用户探针 | 15 项，含封禁回归 |
| 部署 | install.sh 校验 LiveKit sha256；Web Push（ring 纯 Rust） |

仍未做（大改造）：A2 WS/DB 批量事务、B3 消息虚拟化、B14 i18n、A10 会话滑动续期、B5 `useVoiceSession` memo、D3/D4/D5。
