# CloudRoom API / WebSocket 契约

所有接口位于 `/api/v1`，错误统一为 `{"error": {"code": "...", "message": "..."}}`。

## 通用前置条件

- Cookie 会话：`__Host-cloudroom_session`（`Secure; HttpOnly; SameSite=Lax`）。测试环境可用 `server.insecure_cookies=true` 放宽。
- CSRF：所有 cookie 认证的变更请求必须带 `X-CSRF-Token`（从 `GET /api/v1/me` 获取）；GET 不改变状态。
- 房间授权：进入房间需要成员关系；媒体端点额外要求 `join_voice` 能力。

## 房间申请（建房与媒体模式变更共用）

`POST /api/v1/room-requests`，请求体（`deny_unknown_fields`，未知字段返回 422）：

```jsonc
// 创建房间
{
  "kind": "create_room",
  "note": "团队例会",
  "desired_code": "666",          // 仅超级管理员可指定
  "media_mode": "p2p"             // 必填："p2p" | "sfu"
}

// 变更媒体模式（房主，普通标准房间）
{
  "kind": "change_media_mode",
  "room_code": "666",             // 必填：目标标准房间
  "media_mode": "sfu",            // 必填，且必须与当前模式不同
  "note": "换到 SFU"
}
```

校验规则：

- guest 不能提交；`note` ≤ 500 字符；缺字段、非法枚举或字段组合错误一律 `422 validation_error`，不猜测默认模式。
- `change_media_mode`：申请人必须是目标标准房间的 owner；direct 房间返回 422（继承父房间模式）；目标模式与当前相同返回 422；同房间已有 pending 变更返回 `409 conflict`。
- 超级管理员提交即自动批准（响应 `auto_approved: true` 并携带 `room`）；其他用户进入 pending，由管理员审批。

响应中的申请对象：

```jsonc
{
  "request_id": "...",
  "requester_id": "...",
  "kind": "create_room" | "change_media_mode",
  "note": "...",
  "desired_code": "666" | null,          // 仅 create_room
  "target_room_code": "666" | null,      // 仅 change_media_mode
  "media_mode": "p2p" | "sfu",
  "status": "pending" | "approved" | "rejected" | "cancelled",
  "created_at": "...", "decided_at": null, "decided_by": null,
  "decision_reason": null
}
```

管理端：

- `GET /api/v1/admin/room-requests?status=pending`（admin/super_admin）
- `POST /api/v1/admin/room-requests/{id}/approve`：按 `kind` 分派——create 建房，change 应用模式变更；申请人不能批准自己的任何申请（403）；响应含 `request` 与 `room`（单条响应中的 `target_room_code` 为 null，列表接口会补全）。
- `POST /api/v1/admin/room-requests/{id}/reject`：`{"reason": "..."}` 必填并写审计。
- 用户端：`GET /api/v1/room-requests`、`POST /api/v1/room-requests/{id}/cancel`。

### 访客转正

- `POST /api/v1/rooms/{code}/guest-upgrade-requests`（房主）：`{"user_id":"<访客>"}` 发起转正申请；重复 pending 返回 409。
- `GET /api/v1/admin/guest-upgrade-requests?status=pending`（admin）：待审批列表。
- `POST /api/v1/admin/guest-upgrade-requests/{id}/approve`：批准后生成**绑定该访客**的 registration 邀请（不返回明文），访客升级时自动使用。
- `POST /api/v1/admin/guest-upgrade-requests/{id}/reject`：拒绝。
- `POST /api/v1/auth/upgrade-guest`：`invite_token` 可选；仅接受 registration 邀请（房间邀请码不可用于转正）。无码时依次匹配“已批准的绑定邀请”或“房间开关（房主为管理员且 `guest_upgrade_open=true`）”。
- 房间设置新增 `guest_upgrade_open`：仅房主且账号为 admin/super_admin 可开启；房主失去 admin 时其名下所有房间自动置 0。
- 邀请人记录在 `users.invited_by`：逐客审批为发起申请的房主，通用注册码为发码管理员，房间开关为房主；仅管理端用户列表展示。

## 房间与媒体模式

- `Room.media_mode`：`"p2p" | "sfu"`（不再有 `auto`）；出现在 `GET /api/v1/rooms/{code}`、`/api/v1/me/rooms`、admin 房间详情中。
- `POST /api/v1/rooms/{code}/settings` 接受 `text_enabled` / `allow_direct_messages` / `expires_at` / `audio_bitrate`（kbps，0=自动）/ `announcement` / `max_members` / `e2ee` / `guest_upgrade_open`；携带 `media_mode` 返回 `422 validation_error`（模式只能走申请通道）。`audio_bitrate` 按当前房主账号角色校验 `voice.bitrate_tiers` 门槛（P2P 房间可保存但仅 SFU 生效）。
- direct 房间的 `media_mode` 在创建时继承父标准房间。
- `POST /api/v1/room-requests`（create_room 与 change_media_mode）可携带 `audio_bitrate`，校验同上；变更申请仅在目标为 sfu 时生效。
- `GET /api/v1/voice/options`（登录）：`{"default_kbps": 0, "bitrate_tiers": [{"kbps": 192, "min_owner_role": "admin"}, ...]}`——前端按当前用户角色过滤可选项。
- 房间级全员禁言与发言权随房间快照返回：`GET /api/v1/rooms/{code}` 响应新增 `voice` 字段：`{can_speak, granted, pending}`；房主（标准房间唯一 owner / 私聊双方 owner）额外可见 `grants: [{user_id, nickname, granted_at}]` 与 `requests: [{user_id, nickname, created_at}]`。

## 消息历史
`GET /api/v1/rooms/{code}/messages`（成员）：返回 `{"messages": [...], "last_seq": n}`，消息恒按 `seq` 升序。
- 无参数 / `after=0`：**最新一页**（服务端按 seq 倒序取 `limit` 条后翻回升序），首屏用它，避免长历史房间只看到最旧的 50 条。
- `before=<seq>`：取 seq 更早的一页（历史翻页，前端「加载更早的消息」）。
- `after=<seq>`：取 seq 之后的新消息（断线重连补齐缺口）。
- `limit`：1..200，默认 50。
## 消息编辑 / 删除 / 搜索
- `POST /api/v1/rooms/{code}/messages/{message_id}/edit`：`{"body":"..."}`。仅作者本人；已删除消息返回 409；成功后广播 `message_updated`。
- `POST /api/v1/rooms/{code}/messages/{message_id}/delete`：软删除（作者、房主或 admin/super_admin）。正文不再下发，广播 `message_deleted`。
- `GET /api/v1/rooms/{code}/messages/search?q=<关键词>&limit=30`：房间内子串搜索（LIKE，跨语言），只返回未删除消息，按时间升序；`limit` 默认 20，范围 1–50。
- 消息 JSON 新增：`edited_at`（null=未编辑）、`deleted`（true 时 body 恒为空串）、`attachment_id`、`attachment`。

## 消息线程

- 发送线程回复：`POST /rooms/{code}/messages` 或 WS `send_message` 增加可选 `thread_root_id`；根消息必须属于同房间、未删除且自身不是回复。
- `GET /api/v1/rooms/{code}/messages/{message_id}/thread?before=&limit=50`（成员）：返回 `{"root": message, "replies": [...]}`，按 `seq` 升序；`limit` 1–100。
- 频道主列表只返回 `thread_root_id IS NULL` 的消息；根消息 JSON 带 `thread_reply_count` 与 `thread_unread`（排除自己的回复），线程回复带 `thread_root_id`。
- `POST /api/v1/rooms/{code}/messages/{message_id}/thread/read`（CSRF + 成员）：把该线程已读游标推进到当前最新回复，返回 `{"ok":true,"last_seq":n}`；打开线程面板时自动调用。
- 线程回复推送标题带「· 线程」，URL 为 `/{code}?thread=<root_id>`，打开房间后自动展开对应线程。
- 删除根消息后线程入口返回 404；线程回复不会出现在主频道时间线。

## 附件（房间级开关）
建房申请携带 `attachments: true` 才会开启；`Room.attachments_enabled` 随房间快照返回，未开启时上传一律 403 `attachments_disabled`。
- `POST /api/v1/rooms/{code}/attachments`（multipart，字段 `file`）：返回 `{"attachment":{id,filename,mime,size,url,thumbnail_url}}`；图片会异步生成 320px JPEG 缩略图，`thumbnail_url` 为 null 表示非图片。
  - 缩略图：`GET /api/v1/rooms/{code}/attachments/{attachment_id}/thumbnail`，成员鉴权，`image/jpeg`；生成失败或非图片返回 404，前端回退到原图。
  - 允许类型：PNG/JPEG/WebP/GIF、MP3/WAV/OGG/M4A、MP4/WebM、PDF（按魔数嗅探，不信 Content-Type）。
  - 普通成员可发图片（`max_user_attachment_bytes`，默认 2 MiB）与音频/视频（`max_media_attachment_bytes`，默认 20 MiB）；admin/super_admin 可发 PDF（`max_attachment_bytes`，默认 5 MiB），媒体同样 20 MiB。
  - 房间配额 `security.max_room_attachment_bytes`（默认 50 MiB），超限 409 `attachment_quota_exceeded`。
- `GET /api/v1/rooms/{code}/attachments/{attachment_id}`：成员鉴权后返回文件；图片/音频/视频 inline，PDF 为 attachment；支持单段 `Range` 请求（206），音视频可拖动进度。
- 发送消息时带 `attachment_id`（正文可为空），只能引用本房间且由本人上传的附件。

## 媒体加密（房间可选，服务器持钥）
- `POST /rooms/{code}/settings` 接受 `e2ee: bool`；仅 SFU 房间可开启，非 SFU 返回 422。
- 开启时服务端生成 32 字节共享密钥，随 `POST /rooms/{code}/media-token` 响应的 `e2ee_key` 下发给成员（LiveKit 与中间网络无法解密；服务端持有密钥，属"服务器可信"模型）。关闭后密钥清空。
- 成员切换开关后需重新加入语音生效。

## 媒体凭证端点

| 端点 | 权限 | 房间模式 | 成功响应 |
|---|---|---|---|
| `GET /api/v1/rooms/{code}/ice-servers` | 成员 + `join_voice` | 仅 `p2p` | `{"iceServers":[{"urls":["turn:<host>:3478?transport=udp"],"username":"...","credential":"..."}],"ttl":300}`（LiveKit 内嵌 TURN 短期凭证） |
| `POST /api/v1/rooms/{code}/media-token` | CSRF + 成员 + `join_voice` | 仅 `sfu` | `{"token":"<JWT>","url":"wss://<livekit>","room":"<code>"}` |

错误：

- 模式错配（p2p 调 media-token、sfu 调 ice-servers）：`409 media_mode_mismatch`。
- p2p 房间加入语音超过 `p2p_voice_max_participants`：`409 sfu_required`。
- 房间级禁言中（`room_muted=true`）且用户未获发言权（房主同样受此限制）：`403 room_muted`（ice-servers / media-token / `voice_join` 同规则）。
- 非成员/房间失效：沿用现有 403 / 410，不泄露房间模式。

LiveKit JWT claims：`iss` = api_key，`sub` = 用户 ID（participant identity），`exp` = `media_token_ttl`，`video` 包含 `room`、`roomJoin: true`、`participant`、`canPublish: true`、`canSubscribe: true`、`canPublishSources: ["microphone", "screen_share"]`。当前版本不能发布 camera / data；屏幕共享仅 SFU 房间可用。

## 控制面 WebSocket

`GET /api/v1/rooms/{code}/ws`（WSS，cookie 会话）。

客户端事件：

| type | 说明 |
|---|---|
| `ping` | 心跳，服务端回 `pong` |
| `send_message` | `{"type":"send_message","body":"...","reply_to":"<message_id>"}` 文字消息；`reply_to` 可省略 |
| `voice_join` / `voice_leave` | 仅 p2p 房间；sfu 房间返回 `media_mode_mismatch`；禁言中未获权返回 `403 room_muted` |
| `signal` | `{"type":"signal","to":"<uid>","data":{...}}` P2P SDP/ICE 中继，仅 p2p 房间、仅当前成员之间 |
| `media_state` | `{"type":"media_state","mic_muted":bool,"speaker_muted":bool}` 麦克风/扬声器静音状态广播（供成员列表展示） |
| `room_mute_toggle` | `{"type":"room_mute_toggle","muted":bool}` 仅房主；开启/关闭房间级全员禁言 |
| `speak_request` | 禁言中申请发言（房主申请自动通过并广播 `speak_grant`） |
| `speak_reply` | `{"type":"speak_reply","user_id":"...","approve":bool}` 仅房主；批准/拒绝申请 |
| `speak_revoke` | `{"type":"speak_revoke","user_id":"..."}` 仅房主；取消某用户发言权限 |
| `speak_relinquish` | 获权用户主动放弃自己的发言权；广播 `speak_revoked`，含 `relinquished:true` |
| `reaction` | `{"type":"reaction","message_id":"...","emoji":"..."}` 切换回应；emoji 白名单，服务端广播最新聚合 |
| `typing` | 正在输入（4s 过期，服务端节流广播） |
| `read` | `{"type":"read","seq":N}` 上报已读游标；广播 `read` 聚合 |
| `hand` | `{"type":"hand","raised":bool}` 举手/放下；随 `hello.hands` 下发 |
| `voice_reaction` | `{"type":"voice_reaction","emoji":"..."}` 语音表情飘屏（白名单） |

服务端事件：`hello`（房间/成员/voice_members/序列号/hands/reads）、`message`、`message_updated`、`message_deleted`、`reaction`（`message_id`/`user_id`/`added`/`reactions` 聚合）、`typing`、`read`、`hand`、`voice_reaction`、`member_joined`/`member_left`/`member_updated`、`member_offline`（断连≠离开房间，成员关系保留）、`room_settings`、`voice_state`（p2p）、`signal`（p2p）、`media_state`、`room_muted`、`speak_grant`/`speak_revoked`/`speak_requested`/`speak_rejected`、`speak_request_notify`（通知房主）、`muted`/`unmuted`、`owner_transferred`（`owner_user_id`）、`direct_room_created`（父房间通知，含 `room`/`peer_user_id`）、`room_deleted`、`error`。

断线后按 1/2/4/8/16/30 秒退避重连，`hello` 重新同步；不整页刷新。关闭标签页断连经 8 秒延迟窗口确认后广播 `member_offline` 并插入"xxx 离开了房间"系统消息（主动离开走 `member_left`，不重复播报）。

## 暂无前端入口的端点

- `GET /api/v1/rooms/{code}/invites`、`POST /api/v1/rooms/{code}/invites/{id}/revoke`：邀请列表/撤销；当前仅测试使用。
- `GET /api/v1/rooms/{code}/mutes`：禁言列表；当前仅测试使用，前端读取房间快照中的 `mutes`。
- `POST /api/v1/rooms/{code}/mutes` 仅支持 `scope:"member"`；全员禁言使用 WebSocket `room_mute_toggle`（写 `rooms.room_muted`），避免两套房间级状态不一致。
- `POST /api/v1/rooms/{code}/messages`：HTTP 发消息；前端统一走 WebSocket，保留给脚本/测试。
- `POST /api/v1/rooms/{code}/members/{uid}/role`：仅支持把 manager 降为 member；无前端入口，当前仅测试使用。

## 跨房间搜索

- `GET /api/v1/search?q=<关键词>&limit=20`（登录）：仅搜索调用者当前加入且 `state=active` 的房间，排除已删除消息；`limit` 默认 20，范围 1–50；`q` 1–100 字符。
- 3 个字符及以上走 SQLite FTS5 trigram（子串、中文可用）；1–2 个字符回退到转义后的 `LIKE`。
- 响应：`{"results":[{"message_id","room_code","seq","body","created_at","sender_user_id","sender_nickname"}],"query":"..."}`，最新在前。

## 房间 Webhook

- 房主管理：`POST /api/v1/rooms/{code}/webhooks` `{"name":"CI"}` → 返回 `{webhook:{id,name,...},token,url}`；令牌只返回一次，数据库仅存 SHA-256。
- `GET /api/v1/rooms/{code}/webhooks` 列出（不含令牌）；`POST .../webhooks/{id}/revoke` 撤销。
- 公开投递：`POST /api/v1/webhooks/{token}` `{"body":"..."}`，无需 Cookie/CSRF；仅普通房间、文字频道开启时可用；消息以当前房主身份写入并广播 `message`；按 token 限流。
- 访问日志把 `/api/v1/webhooks/<token>` 重写为 `/api/v1/webhooks/***`，不记录令牌。

## OIDC 单点登录

- `GET /api/v1/auth/oidc/login?next=/`：302/303 跳转到 IdP；使用 state、nonce、PKCE S256，state 10 分钟一次性有效。
- `GET /api/v1/auth/oidc/callback?code=&state=`：校验 state，交换 code，验证 ID token（RS/ES 签名、iss、aud、exp、nonce），成功后建立会话并跳回 `next`（仅允许站内相对路径）。
- 账号匹配顺序：`(issuer, sub)` 已绑定 → 已验证邮箱匹配本地账号并绑定 → `auto_create=true` 时创建普通用户；否则 403。
- 失败统一跳转 `/login?sso_error=1`，不向浏览器泄露 IdP 错误细节。
- 公共配置 `GET /api/v1/config/public` 的 `oidc` 字段只暴露 `{enabled,label}`。

## 管理端审计

- `GET /api/v1/admin/audit?offset=0&limit=50`（admin/super_admin）：分页返回审计条目。
- `GET /api/v1/admin/audit/export?limit=5000`：返回 `text/csv` 附件（最新在前，`limit` 1–10000）；字段为 `entry_id,created_at,actor_user_id,action,target_type,target_id,detail`，`detail` 为 JSON 字符串并做 CSV 转义。
- 保留期由 `security.audit_retention_days` 控制（默认 0=永久），sweeper 每分钟清理过期条目。

## 公共配置与健康

- `GET /api/v1/config/public`：`features` 固定为 `{"voice": true, "screen_share": true, "media_playback": false, "voice_modes": ["p2p","sfu"], "p2p_voice_max_participants": <配置值>}`（屏幕共享仅 SFU 房间可用）。
- `GET /api/v1/health`：`checks.sqlite` 与 `checks.livekit`（配置通过启动校验后恒为 `configured`）。

## 未来能力

`media_playback` 为 `false` 表示尚未交付，不是客户端可绕过的权限；未来实现时新增显式 capability 与对应错误码，不恢复 P2P DataChannel 大文件传输。

## 两步验证（TOTP）

- `POST /api/v1/me/totp/setup`（CSRF）：生成新密钥，返回 `{secret, otpauth_url}`；此时尚未启用。
- `POST /api/v1/me/totp/enable` `{code}`：验证 6 位动态码后启用，返回一次性 `recovery_codes`（10 条）。
- `POST /api/v1/me/totp/disable` `{password?, code}`：有密码的账号必须提供密码；`code` 可为动态码或未使用的恢复码。
- 登录：密码正确且已启用 TOTP 时返回 `{"totp_required":true,"challenge_token":"..."}`（5 分钟一次性）；再调用 `POST /api/v1/auth/login/totp` `{challenge_token, code}` 建立会话。恢复码同样可用于第二步。
- `GET /api/v1/me` 新增 `totp_enabled`。管理员重置密码会同时清除 TOTP，作为丢失验证器的恢复路径。

## 消息固定与转发

- `POST /api/v1/rooms/{code}/messages/{message_id}/pin`（房主 + CSRF）：固定未删除消息；幂等。
- `POST /api/v1/rooms/{code}/messages/{message_id}/unpin`（房主 + CSRF）：取消固定；幂等。
- `GET /api/v1/rooms/{code}/pins`（成员）：按固定时间倒序返回未删除消息；消息 JSON 带 `pinned=true`。
- 广播事件：`message_pinned` / `message_unpinned`（`message_id`）。
- `POST /api/v1/rooms/{code}/messages/{message_id}/forward`（CSRF）：`{"target_room_code":"..."}`；目标房间需为当前用户已加入的正常房间且文字频道开启；复制正文，不带附件；消息 JSON 的 `forwarded_from` 记录来源 `{room_code,sender,created_at,message_id}`。

## 全局收件箱

- `GET /api/v1/inbox?limit=20`（登录）：聚合当前用户所有 `active` 房间：
  - `rooms[]`：`{room_code,kind,unread,mentions,thread_unread}`；`unread` 为主频道未读（排除自己），`thread_unread` 为线程未读，`mentions` 为未读 @提及。
  - `mentions[]`：最近提及（默认 20，最大 50），含 `message_id/room_code/body/created_at/sender_nickname/unread/thread_root_id`。
  - `total_unread` = 主频道未读 + 线程未读；`total_mentions` = 未读提及。
- 发送消息时按房间昵称记录 `message_mentions`；历史消息不回溯索引（迁移后新消息生效）。
- 已读沿用 `room_members.last_read_seq`（主频道）与 `thread_reads.last_seq`（线程）；打开房间/线程时由客户端上报。

## 定时消息与草稿

- `POST /api/v1/rooms/{code}/messages/schedule`（CSRF + 成员）：`{body,send_at,thread_root_id?}`；`send_at` 为 RFC3339，必须晚于当前时间且不超过 30 天；每房间/用户最多 20 条待发送。
- `GET /api/v1/rooms/{code}/scheduled`：返回当前用户在该房间的待发送消息。
- `POST /api/v1/rooms/{code}/scheduled/{id}/cancel`：取消自己的待发送消息。
- 后台每 30 秒投递到期消息，复用正常发送流程（成员/文字频道/禁言/限流校验）；失败记录 `status=failed` 与 `error`。
- 草稿仅保存在浏览器 `localStorage`（`cloudroom.draft.<room_code>`），发送成功/取消后清除，不经过服务器。

## 自定义 Emoji

- `GET /api/v1/emojis`（登录）：返回 `{emojis:[{id,name,url}]}`；`url` 为成员鉴权图片地址。
- `GET /api/v1/emojis/{id}/image`（登录）：图片 inline，私有缓存。
- `POST /api/v1/admin/emojis`（admin，multipart `name` + `file`）：名称 2–32 位小写字母/数字/_-+；PNG/JPEG/WebP/GIF，≤256 KiB、≤128×128。
- `DELETE /api/v1/admin/emojis/{id}`（admin）：删除记录与文件。
- 消息正文中的 `:name:` 会渲染为对应图片；代码块/行内代码内不替换。

## 链接预览

- `GET /api/v1/link-preview?url=<http(s) URL>`（登录）：服务端抓取 HTML，解析 `og:title`/`og:description`/`og:site_name`（回退 `<title>`），返回 `{preview:{url,title,description,site_name}}`。
- 安全：仅 http/https、仅 80/443、拒绝 URL 凭据；DNS 解析后所有地址必须为公网 IP 并固定连接；不自动跟随到内网（手动重定向最多 2 次且每跳重新校验）；8 秒超时、512 KiB HTML 上限、仅 text/html；失败统一 422 并缓存 1 小时，成功缓存 24 小时。
- 隐私：抓取由服务器发起，会暴露服务器 IP；前端只展示文本卡片，不加载远程图片。

## 消息保留与房间导出

- `rooms.message_retention_days`（默认 0=永久）：sweeper 每分钟把早于该天数的消息软删除（正文清空、FTS 移除），同时删除其附件行与文件并回收房间配额；已删除消息仍保留占位。
- `GET /api/v1/rooms/{code}/export`（房主或 admin）：返回 `application/json` 附件，含房间信息、成员与全部未删除消息；超过 20000 条返回 413。

## 自定义 Emoji 反应

- WS `reaction` 事件除固定 unicode 白名单外，接受已注册的 `:name:`（名称需存在且未禁用）；`reaction` 聚合中 `emoji` 原样返回 `:name:`，前端用自定义 emoji 图片渲染。
- 固定 unicode 白名单：`👍 ❤️ 😂 😮 😢 😡 🎉`。
