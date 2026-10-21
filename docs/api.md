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
- `POST /api/v1/admin/room-requests/{id}/approve`：按 `kind` 分派——create 建房，change 应用模式变更；变更申请的申请人不能批准自己的申请（403）；响应含 `request` 与 `room`。
- `POST /api/v1/admin/room-requests/{id}/reject`：`{"reason": "..."}` 必填并写审计。
- 用户端：`GET /api/v1/room-requests`、`POST /api/v1/room-requests/{id}/cancel`。

## 房间与媒体模式

- `Room.media_mode`：`"p2p" | "sfu"`（不再有 `auto`）；出现在 `GET /api/v1/rooms/{code}`、`/api/v1/me/rooms`、admin 房间详情中。
- `POST /api/v1/rooms/{code}/settings` 只接受 `text_enabled` / `allow_direct_messages` / `expires_at` / `audio_bitrate`（kbps，0=自动）；携带 `media_mode` 返回 `422 validation_error`（模式只能走申请通道）。`audio_bitrate` 按当前房主账号角色校验 `voice.bitrate_tiers` 门槛（P2P 房间可保存但仅 SFU 生效）。
- direct 房间的 `media_mode` 在创建时继承父标准房间。
- `POST /api/v1/room-requests`（create_room）可携带 `audio_bitrate`，校验同上；变更申请恒为 0。
- `GET /api/v1/voice/options`（登录）：`{"default_kbps": 0, "bitrate_tiers": [{"kbps": 192, "min_owner_role": "admin"}, ...]}`——前端按当前用户角色过滤可选项。
- 房间级全员禁言与发言权随房间快照返回：`GET /api/v1/rooms/{code}` 响应新增 `voice` 字段：`{can_speak, granted, pending}`；房主（标准房间唯一 owner / 私聊双方 owner）额外可见 `grants: [{user_id, nickname, granted_at}]` 与 `requests: [{user_id, nickname, created_at}]`。

## 媒体凭证端点

| 端点 | 权限 | 房间模式 | 成功响应 |
|---|---|---|---|
| `GET /api/v1/rooms/{code}/ice-servers` | 成员 + `join_voice` | 仅 `p2p` | `{"iceServers":[{"urls":["turn:<host>:3478?transport=udp"],"username":"...","credential":"..."}],"ttl":300}`（LiveKit 内嵌 TURN 短期凭证） |
| `POST /api/v1/rooms/{code}/media-token` | CSRF + 成员 + `join_voice` | 仅 `sfu` | `{"token":"<JWT>","url":"wss://<livekit>","room":"<code>"}` |

错误：

- 模式错配（p2p 调 media-token、sfu 调 ice-servers）：`409 media_mode_mismatch`。
- p2p 房间加入语音超过 `p2p_voice_max_participants`：`409 sfu_required`。
- 房间级禁言中（`room_muted=true`）且用户未获发言权、不是房主：`403 room_muted`（ice-servers / media-token / `voice_join` 同规则）。
- 非成员/房间失效：沿用现有 403 / 410，不泄露房间模式。

LiveKit JWT claims：`iss` = api_key，`sub` = 用户 ID（participant identity），`exp` = `media_token_ttl`，`video` 包含 `room`、`roomJoin: true`、`participant`、`canPublish: true`、`canSubscribe: true`、`canPublishSources: ["microphone"]`。当前版本不能发布 camera / screen-share / data。

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

服务端事件：`hello`（房间/成员/voice_members/序列号）、`message`、`reaction`（`message_id`/`user_id`/`added`/`reactions` 聚合）、`member_joined`/`member_left`/`member_updated`、`member_offline`（断连≠离开房间，成员关系保留）、`room_settings`、`voice_state`（p2p）、`signal`（p2p）、`media_state`、`room_muted`、`speak_grant`/`speak_revoked`/`speak_requested`/`speak_rejected`、`speak_request_notify`（通知房主）、`muted`/`unmuted`、`owner_transferred`（`owner_user_id`）、`direct_room_created`（父房间通知，含 `room`/`peer_user_id`）、`room_deleted`、`error`。

断线后按 1/2/4/8/16/30 秒退避重连，`hello` 重新同步；不整页刷新。关闭标签页断连经 8 秒延迟窗口确认后广播 `member_offline` 并插入"xxx 离开了房间"系统消息（主动离开走 `member_left`，不重复播报）。

## 公共配置与健康

- `GET /api/v1/config/public`：`features` 固定为 `{"voice": true, "screen_share": false, "media_playback": false, "voice_modes": ["p2p","sfu"], "p2p_voice_max_participants": <配置值>}`。
- `GET /api/v1/health`：`checks.sqlite` 与 `checks.livekit`（配置通过启动校验后恒为 `configured`）。

## 未来能力

`screen_share` / `media_playback` 为 `false` 表示尚未交付，不是客户端可绕过的权限；未来实现时新增显式 capability 与对应错误码，不恢复 P2P DataChannel 大文件传输。
