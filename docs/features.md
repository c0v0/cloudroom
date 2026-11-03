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
| 邀请撤销终态（已用/过期 409） | 管理端邀请撤销 | `cd backend && cargo test --test e2e room_mutes_and_invite_revoke_paths` |
| 房主转让后单一 owner | 房间成员管理 | `cd backend && cargo test --test e2e admin_reject_force_delete_and_transfer` |
| 踢人/离开/禁用/重置密码/自助改密/删除房间/过期时撤销 LiveKit 媒体（best-effort） | 房间成员管理与 SFU 媒体面 | `cd backend && cargo test --test e2e livekit_`；真实 LiveKit 按 `docs/plans/2026-10-23-livekit-revocation.md` 手动复核 |
| 消息列表展示与交互 | Room 消息列表 | `cd frontend && npm test -- src/components/MessageList.test.tsx` |
| 消息编辑/删除/搜索 | Room 消息项「编辑/删除」、房间头部「搜索」 | 编辑自己的消息后显示「已编辑」；删除后显示占位；搜索面板可命中 |
| 房间附件（申请制） | 建房申请「允许附件」+ 输入框「附件」 | 未开启的房间上传返回 403；开启后截图可发送并渲染；超配额 409 |
| 屏幕共享（仅 SFU） | 语音栏「共享屏幕」 | SFU 房间加入语音后可共享，远端显示画面 |
| 本地录音 | 语音栏「录音/停止录音」 | 录音结束自动下载 webm（远端混音 + 本地麦克风） |
| 房间公告 | 房间设置「房间公告」+ 顶部横幅 | 支持换行与链接，可关闭，公告更新后重新显示 |
| 媒体端到端加密（房间可选） | 房间设置「端到端加密」 | 开启后 media-token 返回密钥，客户端以 E2EE 连接 |
| 消息历史分页与重连补齐 | Room 消息列表「加载更早的消息」 | 刷新长历史房间默认显示最新 50 条；点「加载更早的消息」按 `before` 游标取更早一页；断线重连循环按 `after` 补齐到 `hello.last_seq`，消息按 `seq` 排序 |
| 提及通知 | 消息正文 `@昵称` | 被 @ 时桌面通知标题为「提到了你」，即使房间静音也提醒；与回复通知不重复 |
| 设备选择 | 个人设置「声音」→ 麦克风 / 扬声器 | 选择后扬声器立即切换；麦克风在语音中自动重建；不支持 `setSinkId` 的浏览器会禁用扬声器下拉 |
| 键盘操作消息 | 消息项操作区 | Tab 聚焦消息内控件时显示「回复/复制/编辑/删除」，不再仅 hover 可见 |
| 正在输入 / 已读回执 | 输入框上方、自己消息下方 | 输入时节流广播 `typing`（4s 过期）；停在底部时上报 `read`，自己消息显示「已读 N」；重连后从 `hello.reads` 恢复；`CHROME_PATH=<chrome> node test-server/two-user-probe.mjs` |
| 举手 / 语音表情 | 语音栏「举手」「👍👏🎉😂❤️🔥」 | 举手状态随 `hello.hands` 下发、成员列表显示 ✋；表情在房间右下角飘屏 2.6s；`CHROME_PATH=<chrome> node test-server/two-user-probe.mjs` |
| Markdown | 消息输入框（支持 Shift+Enter 换行） | `**粗体**`、`*斜体*`、`` `行内代码` ``、```` ```代码块``` ````、`~~删除线~~`、链接；不解析 HTML |
| PWA / 离线壳 | 浏览器安装提示、断网后刷新 | `sw.js` 缓存应用壳与 `/assets/`；离线时导航回退到缓存的壳 |
| 回声消除实际状态提示 | Room 语音栏、声音设置 | `cd frontend && npm test -- src/hooks/audioCapture.test.ts`；真实设备按 `docs/deploy-checklist.md` 第 6 节手动验证 |

范围与产品描述见 `README.md`「当前范围」；API/WS 契约见 `docs/api.md`。

## 模板（新增能力时复制）

```markdown
- [ ] <能力一句话>（<实现位置 file:line>）— 验证：`<命令>` / <可复现步骤> — 归属会话：<日期>
```

## 已废弃 / 移除

- （空）
