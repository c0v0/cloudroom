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

### P2

- `install.sh` 重复执行会覆盖运维改过的 nginx/systemd；升级无 `.prev` 回滚。
- systemd 可再加 `PrivateDevices`/`ProtectClock`/`RestrictNamespaces`/`MemoryDenyWriteExecute` 等；`LimitNPROC=512` 与 tokio 阻塞池上限相同。
- LiveKit 模板只配了 `udp_port: 50000`，文档却写 50000–50100；`node_ip` 未文档化。
- Nginx：WS 与普通 API 共用 3600s 读超时；静态资源无 immutable 缓存头；`listen ... http2` 在 nginx ≥1.25 已废弃。
- 备份命令无 `umask 077`（SQLite 备份默认 0644）。
- 附件删除消息不回收配额与文件（房间配额只增不减，直到删房）。
- 无 Prometheus/结构化指标；健康检查 SQLite 故障仍返回 200。
- `react-router-dom` 有 2 个 moderate 通告（SSR hydration / Link 反斜杠重定向），本 SPA 不可达；修复需升 v7 大版本。

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

1. **提及通知 + 通知去重**（前端 30 行）：复用现有 `pushNotify` 与 `highlightMentions`，把 `@我` 的消息单独成条并高亮标题。
2. **设备选择器**（`enumerateDevices` + `setSinkId`）：Settings 已有声音设置面板，增加两个下拉即可。
3. **消息合并按 `seq` 插入 + 补齐循环**：修 P1-4，顺带修搜索跳转错位。
4. **弹窗语义 + 消息操作键盘可达 + `role=log`**：一次 CSS/ARIA 改动覆盖大部分可访问性问题。
5. **LiveKit 动态加载**：P2P 房间不再下载 530 kB。
6. **房间封禁表**：一条 `room_bans` 迁移 + join 校验，解决 P1-1。
7. **bootstrap 令牌与建号同事务**：10 行改动。
8. **部署包补齐**：`.prev` 回滚、`umask 077` 备份、LiveKit 端口范围、静态资源缓存头。
