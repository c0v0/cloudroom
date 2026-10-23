# 安全修复第二波（2026-10-23）

- 分支：`fix/security-wave2`，从 `fix/security-wave1` 继续（wave1 基于 `main`）。
- 范围：修复第一波安全审查中未处理的高/中风险，以及修复后独立审查发现的新问题。
- 未推送、未合并 main；Compose 部署包（`../cloudroom-compose`，非 Git）同步修改但不在提交中。

## 提交

| 提交 | 内容 |
|---|---|
| `352079f` | `fix(security): harden remaining auth, media and HTTP surfaces` |
| `ee16573` | `fix(security): close post-review gaps` |
| `353cdcb` | `fix(security): close final audit gaps` |
| `25be297` | `fix(rooms): purge dependents atomically and refresh presence` |
| `75aa38f` | `fix(config): validate heartbeat and reject direct-room invites` |

## 主要修复

### 认证 / 会话 / 权限

- 登录账号限流使用 trim 后 key；未知/非法用户名执行 dummy Argon2；Argon2 校验/哈希移入 `spawn_blocking`。
- 账号 bucket 耗尽时仍验证密码：正确密码可登录，错误密码返回 429；避免永久锁死。
- 限流器 key 截断 128 字符、最多 100k 桶；满时淘汰最旧桶，不再全局 fail-closed。
- `/me/password` 加 5/min 限制；密码修改/重置/禁用/登出使用 `Hub::unregister_user` 关闭所有 WS。
- 普通 admin 不能重置 admin/super_admin；`upgrade-guest` 增加 CSRF。
- 匿名 join 先验证房间/邀请，再在事务内创建 guest；`upgrade-guest` 先验证邀请再哈希，并加邀请兑换限流。
- guest 升级前重新映射所有用户 FK（DM、mute、invite、speak grant/request、reaction、message、audit）。
- 邀请 TTL 限制 1s..365d，并使用 `checked_add_signed`。

### WebSocket

- 入站消息/帧大小、每连接有界队列、事件 token bucket、每用户/全局连接上限。
- 用 `watch` 关闭通道同时唤醒 read/write 两侧；logout/禁用/重置/改密/踢人/离房/升级 guest/删房会立即关闭 socket。
- WS 会话绝对过期定时器；连接期间周期刷新 presence，避免空闲用户被误判离线。
- WS 升级按用户限流。

### 媒体 / 头像 / 运维

- LiveKit TTL 默认 10m，生产拒绝 >15m（仍无 `RemoveParticipant`，见剩余风险）。
- ICE/media token/direct room/avatar 领取/读取限流；TURN 仍 300s。
- 头像先用 header 读尺寸，再设置 `image::Limits` 解码；上传/删除 CAS，启动时清理未引用文件；读取改异步 + 600/min。
- 请求体全局 4 MiB、公开 auth 路由 64 KiB；5xx 统一 generic message。
- bootstrap URL 写 0600 文件，不再打印 raw token；集成/UI 脚本改读文件。
- Nginx 关闭 access log（查询串有邀请/bootstrap token），Compose `/rtc` error log 关闭；补齐 API 安全头、XFF 覆盖、dotfile deny。
- Compose `.env` 去固定 secret、`insecure_cookies=false`、真实证书必需、`CHANGE_ME` 拒绝；systemd 增加 `UMask=0077`。
- 修复 sweeper `purge_room` 嵌套事务/FK 阻塞；递归清理 direct 子房，null `room_requests.created_room_id`，增加回归测试。

## 验证

```text
cargo test --locked --all-targets
39 unit tests + 27 e2e tests passed

scripts/check.sh
18 PASS / 0 FAIL / 0 SKIP
```

关键定向测试：

- `grace_purge_handles_room_request_fk_and_continues_sweeping`
- `disabling_an_account_closes_its_open_websocket`
- `forged_xff_from_untrusted_peer_cannot_bypass_login_limit`
- `invalid_anonymous_join_does_not_create_guest_rows`
- `oversized_json_body_is_rejected_before_extraction`
- `ws_rejects_oversized_messages_and_event_bursts`
- `avatar_upload_and_delete_paths`、`avatar_db_failure_cleans_new_file`
- `infra::avatars::tests::cleanup_removes_unreferenced_files`

## 剩余风险（未在本波完全消除）

- **LiveKit 媒体撤销（High，接受）**：TTL 已降到 10m/生产上限 15m，但未实现 LiveKit `RemoveParticipant`/`DeleteRoom`；已连接/已签发 token 在 TTL 内仍可听/说。高敏感 SFU 场景需补服务端 API 或更短 TTL。
- **TURN relay 池**：300s 凭证不绑定房间/用户，relay 端口 51 个；已限流但可被成员耗尽。
- **读接口/DB**：`/health`、`/config/public` 及房间/消息读仍无边缘限流；单 SQLite mutex、无全局请求超时。
- **WS 每房间连接上限/空闲超时**：仅每用户/全局上限和会话绝对过期。
- **账号锁定**：账号 bucket 不再阻止正确密码，但分布式猜测仍只受每 IP + Argon2 成本限制；错误密码达到上限返回 429 仍可区分已存在账号。
- **bootstrap**：token 在创建用户前消费；用户名冲突会烧掉链接（低）。
- **子房清理**：递归 purge 的子房没有插入 tombstone；活跃子房可能随父房清理（低）。
- **frontend-server**：TLS 握手/总请求/上游无超时、无连接上限；仅建议内网/测试使用。
- **管理员临时密码**：普通用户重置仍返回临时密码且不强制首登改密（设计级）。
- **依赖/供应链**：react-router 6.30.4 仍有 moderate advisory；LiveKit 二进制无 checksum；镜像标签未 pin digest。
- **Nginx 边缘限流**：模板仍无 `limit_req`/`limit_conn`。
- **备份/保留**：手工备份默认未加密；audit/messages 无保留策略。
- **无 E2EE / SQLite 未加密 / P2P 语音禁言合作式**：产品边界。

## 结论

本波修复了所有可代码修复的 Critical/High 及主要 Medium；最终独立审查未发现新的 Critical/High（LiveKit 媒体撤销为已知接受项）。公网高对抗场景仍需补 LiveKit 撤销、边缘限流、TURN 配额和部署实测。
