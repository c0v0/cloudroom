# 用户可见能力（features）

> 规则（AGENTS.md「执行轮」）：改动涉及用户可见入口时，实现会话必须在这里更新一行。
> 每条能力必须给出“怎么验证”，验证方式要么是可跑命令，要么是可复现步骤。

## 现状

| 能力 | 入口 | 验证 |
|---|---|---|
| 服务启动与健康检查 | `cloudroom-server --config <cfg>`；`GET /api/v1/health` | `scripts/smoke.sh` |
| 公共配置 | `GET /api/v1/config/public` | `scripts/smoke.sh` |
| 全流程控制面（bootstrap/登录/建房审批/邀请/消息/禁言/删除/房间号释放/WS 隧道） | HTTP API + WSS | `test-server/run-tests.sh` |
| 媒体凭证（p2p ice-servers / sfu media-token 与模式错配 409） | `GET /rooms/{code}/ice-servers`、`POST /rooms/{code}/media-token` | `test-server/run-tests.sh` |
| 前端页面与 SPA 回退、API 反代 | 浏览器（Vite 构建产物） | 先 `scripts/build-musl.sh all` 并准备 `release/livekit-server`，再 `test-server/start-test-env.sh` 后访问 8081 |
| 离开/被踢成员访问拒绝 | 房间 HTTP/WS/语音端点 | `cd backend && cargo test --test e2e left_member` |
| 被踢后重新加入需新邀请 | 房间 join 流程 | `cd backend && cargo test --test e2e kicked_rejoin_requires_new_invite` |
| 邀请撤销终态（已用/过期 409；已撤销重复操作幂等 200） | 管理端邀请撤销 | `cd backend && cargo test --test e2e admin_invite_revoke_and_audit` |
| 房主转让后单一 owner | 管理端房间详情「转让房主」（super_admin） | `cd backend && cargo test --test e2e admin_reject_force_delete_and_transfer` |
| 踢人/离开/禁用/重置密码/自助改密/删除房间/过期时撤销 LiveKit 媒体（best-effort） | 房间成员管理与 SFU 媒体面 | `cd backend && cargo test --test e2e livekit_`；真实 LiveKit 按 `docs/plans/2026-10-23-livekit-revocation.md` 手动复核 |
| 消息列表展示与交互 | Room 消息列表 | `cd frontend && npm test -- src/components/MessageList.test.tsx` |
| 消息编辑/删除/搜索 | Room 消息项「编辑/删除」、房间头部「搜索」 | 编辑自己的消息后显示「已编辑」；删除后显示占位；搜索面板可命中 |
| 房间通知覆盖 | 房间头部「静音通知」 | 按用户+房间服务端保存；跨设备同步；房间静音仅抑制普通消息，@/回复仍推送；`room_notification_prefs_roundtrip` |
| 推送窗口聚合 | 同一用户+房间/线程的 Web Push | 服务端 `digest_seconds`（默认 3s）内合并为一条，正文附「N 条新消息」；`digest_body_only_adds_count_for_multiple_messages` |
| 推送合并 | 同一房间/线程的 Web Push | 同 tag 通知只保留一条并累计「N 条新消息」；页面打开时不弹系统推送；`node --check frontend/public/sw.js` |
| 消息收藏 | 消息操作「收藏/已收藏」；首页「我的收藏」 | 跨房间私有收藏；正文搜索、多选批量删除、深链定位；`bookmarks_roundtrip_is_private_to_user` |
| 通知偏好/免打扰 | 设置「通知范围」「免打扰」 | 服务端保存 `all/mentions/none` 与 DND 截止时间；桌面通知、提示音、Web Push 一致过滤；`notification_prefs_roundtrip_and_validation`、`push_preferences_filter_targets` |
| 全局搜索/命令面板 | `Ctrl/⌘+K` 或首页搜索 | 房间过滤 + 跨房间消息搜索；消息结果直达 `?msg=&seq=` 并滚动定位；`CommandPalette.test.tsx`、`cargo test --test e2e` 搜索回归 |
| 消息保留/房间导出 | 房间设置「导出聊天记录」；`rooms.message_retention_days` | 导出 JSON 含成员与消息；保留期到期软删除正文并清理附件；`cargo test --test e2e message_retention_` |
| 链接预览 | 消息中的 http(s) 链接 | 服务端安全抓取 OG 标题/描述/站点；公网 IP 固定、禁内网重定向、512 KiB 上限；失败静默隐藏；`cargo test --test e2e link_preview_` |
| 自定义 Emoji 反应 | 消息操作表情选择器 | 固定 unicode + 已上传 `:name:` 均可回应；聚合计数与取消切换沿用现有逻辑；`cargo test --test e2e ws_` |
| 自定义 Emoji | 管理后台「Emoji」；消息正文 `:name:` | 管理员上传 ≤128px 图片；消息内渲染为图片；代码块不替换；`cargo test --test e2e custom_emoji_` |
| 消息草稿/定时发送 | 输入框自动保存草稿；「定时」按钮 | 草稿按房间存 localStorage；定时消息 30 天内、每房间最多 20 条，后台每 30 秒投递；`cargo test --test e2e full_flow_e2e` |
| 全局收件箱 | 首页「收件箱」 | 聚合所有房间未读、@提及、线程未读；点击跳转房间/线程；`cargo test --test e2e full_flow_e2e` |
| 消息固定/转发 | 房主消息操作「固定」；任意成员「转发」 | 固定消息显示在频道顶部并可取消；转发到已加入的其他房间，显示「转发自 /房间」；`cargo test --test e2e full_flow_e2e` |
| 两步验证（TOTP） | 个人设置「两步验证」；登录第二步 | 启用后返回恢复码；登录密码通过后要求动态码/恢复码；管理员重置密码会清除 TOTP；`cargo test --test e2e totp_` |
| OIDC 单点登录 | 登录页「使用 SSO 登录」 | 配置 `[auth.oidc]` 后启用；state/nonce/PKCE、ID token 签名校验；首次登录按配置自动建号；`cargo test --test e2e oidc_login_redirect_callback_and_auto_create` |
| 房间 Webhook | 房主「Webhook」卡片 | 创建令牌（只显示一次）→ 外部 `POST /api/v1/webhooks/{token}` 发消息；撤销后 404；`cargo test --test e2e webhook_ingest_and_revoke` |
| 消息线程 | 消息操作「线程」→ 线程面板 | 根消息显示回复数与未读徽标；线程内回复不进入主频道；面板收到新回复实时刷新并标记已读；`cargo test --test e2e full_flow_e2e` |
| 跨房间搜索 | 首页「搜索所有房间」 | 输入 ≥3 字符走 FTS5，1–2 字符走 LIKE；只返回当前加入的正常房间；`cargo test --test e2e full_flow_e2e` |
| 房间附件（申请制） | 建房申请「允许附件」+ 输入框「附件」 | 未开启的房间上传返回 403；支持图片/音频/视频/PDF；图片缩略图、音频/视频内嵌播放、PDF 按需 blob 预览；超配额 409；`cargo test --test e2e attachment_` |
| 屏幕共享（仅 SFU） | 语音栏「共享屏幕」 | SFU 房间加入语音后可共享，远端显示画面 |
| 本地录音 | 语音栏「录音/停止录音」 | 录音结束自动下载 webm（远端混音 + 本地麦克风） |
| 房间公告 | 房间设置「房间公告」+ 顶部横幅 | 支持换行与链接，可关闭，公告更新后重新显示 |
| 媒体加密（房间可选，服务器持钥） | 房间设置「媒体加密（服务器持钥）」 | 开启后 media-token 返回密钥，客户端以 E2EE 连接 |
| 消息历史分页与重连补齐 | Room 消息列表「加载更早的消息」 | 刷新长历史房间默认显示最新 50 条；点「加载更早的消息」按 `before` 游标取更早一页；断线重连循环按 `after` 补齐到 `hello.last_seq`，消息按 `seq` 排序 |
| 提及通知 | 消息正文 `@昵称` | 被 @ 时桌面通知正文包含「提到了你」，即使房间静音也提醒；与回复通知不重复 |
| 设备选择 | 个人设置「声音」→ 麦克风 / 扬声器 | 选择后扬声器立即切换；麦克风在语音中自动重建；不支持 `setSinkId` 的浏览器会禁用扬声器下拉 |
| 键盘操作消息 | 消息项操作区 | Tab 聚焦消息内控件时显示「回复/复制/编辑/删除」，不再仅 hover 可见 |
| 正在输入 / 已读回执 | 输入框上方、自己消息下方 | 输入时节流广播 `typing`（4s 过期）；停在底部时上报 `read`，自己消息显示「已读 N」；重连后从 `hello.reads` 恢复；`CHROME_PATH=<chrome> node test-server/two-user-probe.mjs` |
| 举手 / 语音表情 | 语音栏「举手」「👍👏🎉😂❤️🔥」 | 举手状态随 `hello.hands` 下发、成员列表显示 ✋；表情在房间右下角飘屏 2.6s；`CHROME_PATH=<chrome> node test-server/two-user-probe.mjs` |
| Markdown | 消息输入框（支持 Shift+Enter 换行） | `**粗体**`、`*斜体*`、`` `行内代码` ``、```` ```代码块``` ````、`~~删除线~~`、链接；不解析 HTML |
| PWA / 离线壳 | 浏览器安装提示、断网后刷新 | `sw.js` 缓存应用壳与 `/assets/`；离线时导航回退到缓存的壳 |
| 回声消除实际状态提示 | Room 语音栏、声音设置 | `cd frontend && npm test -- src/hooks/audioCapture.test.ts`；真实设备按 `docs/deploy-checklist.md` 第 6 节手动验证 |
| 后台推送（Web Push） | 个人设置「声音」→「后台推送」 | 服务端配置 `[push]` 后开关可见；开启后关闭页面，被 @ 或被回复仍收到系统通知；`cargo test --lib` 覆盖 VAPID 签名与 RFC 8291 报文布局 |
| 访客转正 | 房主在成员列表「申请转正」；管理后台「转正」tab；房间设置「允许访客直接注册」 | 房主申请→管理员批准→访客升级免填码；`cargo test --test e2e guest_upgrade`；管理员用户列表显示邀请人 |

范围与产品描述见 `README.md`「当前范围」；API/WS 契约见 `docs/api.md`。

## 模板（新增能力时复制）

```markdown
- [ ] <能力一句话>（<实现位置 file:line>）— 验证：`<命令>` / <可复现步骤> — 归属会话：<日期>
```

## 已废弃 / 移除

- （空）
