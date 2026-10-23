# CloudRoom 公网部署安全审查（2026-10-22）

- 审查对象：`/home/hui/agents/omp/cloudroom`，工作区 HEAD `a8b6afa`；同时检查当前工作区的部署包 `../cloudroom-compose`（非 Git）。
- 审查方式：独立只读安全审查会话；未修改代码、配置、数据库，未提交，未向外部目标发请求。
- 证据规则：结论追到 `文件:行`；代码未证明的行为标「疑似」；未实际启动生产服务、未执行破坏性 payload。
- 适用范围：单机 SQLite + Nginx（或可选 `cloudroom-frontend-server`）+ 同机/外部 LiveKit；公网暴露 HTTP API、WSS、LiveKit 信令/媒体、TURN。

## 1. 威胁模型摘要

### 1.1 资产

| 资产 | 位置 | 影响 |
|---|---|---|
| 账号/密码哈希 | SQLite `users` | 账号接管、撞库 |
| 会话/CSRF token | SQLite `sessions`、浏览器 Cookie | 冒充登录、读写房间 |
| 房间/消息/公告/昵称 | SQLite `rooms`/`messages`/`room_members` | 隐私泄露、篡改、骚扰 |
| 邀请（房间/注册/bootstrap） | SQLite `invites`/`bootstrap_tokens` | 未授权进房、首个 super_admin |
| 头像文件 | `data/avatars/` | 隐私、存储耗尽、解码 DoS |
| LiveKit API key/secret 与 JWT/TURN 凭证 | `/etc/cloudroom/config.toml`、`/etc/livekit.yaml` | 任意加入 SFU 房间、媒体窃听、TURN 滥用 |
| 管理权限/审计 | `users.role`、`audit_log` | 全站管理、审计伪造 |
| SQLite 数据库/备份 | `data/cloudroom.db`、`backups/` | 全量数据泄露 |

### 1.2 入口与信任边界

```
匿名 ──HTTPS/WSS──> Nginx/前端服务器 ──HTTP/WS(loopback)──> cloudroom-server ──> SQLite
  │                                                          └──> LiveKit API/媒体（JWT/TURN）
  ├── 公开：/health、/config/public、/auth/register、/auth/login、/auth/upgrade-guest
  ├── 邀请：/rooms/{code}/join（匿名可创建 guest 会话）
  ├── 成员：/rooms/{code}、messages、mutes、settings、invites、ws、media-token、ice-servers
  └── 管理：/admin/*（admin/super_admin；部分 super_admin-only）
```

- 未登录用户只能读公开配置、注册、登录、凭邀请加入、读头像（仍需有效会话）。
- 普通成员只能操作自己所在房间；房主有房间管理权；admin 有全局元数据/管理权；super_admin 有角色/转让权。
- Nginx/前端服务器是唯一公网入口；后端应只监听 loopback/Unix socket。
- LiveKit 是独立信任域：后端签发的 JWT/TURN 凭证在有效期内是 bearer 凭证。
- 反向代理、LiveKit、数据库主机、日志系统均视为需要加固的基础设施，不是应用内信任边界。

### 1.3 严重度口径

按「公网可利用性 × 影响」评级，而不是代码风格：

- **Critical**：公网默认/低成本即可拿下管理面、媒体面或全量数据。
- **High**：可导致全站不可用、权限/会话绕过、未授权媒体访问，或大规模账号风险。
- **Medium**：需要成员/房主/管理员等前置权限，或需要特定条件，但影响真实。
- **Low**：信息泄露、防御纵深、配置/文档不一致或利用条件苛刻。

---

## 2. 发现表

统计：**Critical 1（条件性） / High 7 / Medium 9 / Low 13**。`Critical` 仅指当前测试 Compose 包被直接暴露到公网的情形；按 release 模板正确配置后，C-01 不适用。

> `V*` 对应第 5 节的验证命令。带「条件性」的条目取决于实际部署方式。

| ID | 严重度 | 位置（文件:行） | 问题 | 攻击场景 | 修复建议 | 验证 |
|---|---|---|---|---|---|---|
| C-01 | **Critical（条件性）** | `../cloudroom-compose/.env:10-12`（LiveKit 固定测试 key/secret）；`../cloudroom-compose/cloudroom/config.template.toml:7-9`（`insecure_cookies=true`）；`../cloudroom-compose/nginx/cloudroom.conf:7-46`（无 HSTS/CSP/XFO/nosniff；LiveKit 7880 直接发布） | 当前 Docker Compose 包是测试包，却具备可公网启动的端口/配置：固定 LiveKit secret、无 Secure 的会话 Cookie、自签证书、无安全响应头。 | 若直接暴露公网：知道固定 secret 的人可伪造任意房间的 LiveKit JWT，加入/收听 SFU 音频；会话 Cookie 可能在明文 HTTP 降级时泄露；页面可被点击劫持/缺少 CSP 防线。 | 不要把该包直接上公网；换随机 LiveKit key/secret、受信证书、`insecure_cookies=false`、补齐安全头、只暴露必要端口；或使用 release 模板并完成第 4 节清单。 | V1 |
| H-01 | **High** | `backend/src/api/rooms.rs:741-743`；`backend/src/app/rooms.rs:568-600`；`backend/Cargo.toml:42-46` | 房主可控的 `duration_seconds` 直接传给 `chrono::Duration::seconds()` 和 `DateTime + TimeDelta`；超大 i64 会 panic，release 为 `panic="abort"`。 | 任意房主（包括任意成员可创建、双方均为 owner 的私聊房）发送 `duration_seconds=9223372036854775807`，进程 abort，所有房间/WS/登录中断。 | 用 `Duration::try_seconds()` + `checked_add_signed()`；限制 `0..=一年`；非法值 422；为 handler 加 catch-panic 或改 `panic="unwind"`。 | V2（仅隔离实例） |
| H-02 | **High** | `backend/src/api/security.rs:12-25`；`release/nginx/cloudroom.conf.example:59`；`frontend-server/src/main.rs:473-486` | `client_ip()` 只判断 `trusted_proxy.is_some()`，取 `X-Forwarded-For` 第一个值；Nginx `$proxy_add_x_forwarded_for` 会保留客户端自带值，前端服务器原样转发。 | 匿名攻击者每条请求更换 `X-Forwarded-For`，绕过登录/注册/加入的每 IP 限流，进行密码喷洒、无限注册/guest 建号、邀请探测；审计中的 IP 也可伪造。 | 代理改为 `X-Forwarded-For $remote_addr`（或 `X-Real-IP`）；后端校验 TCP peer 是否匹配 `trusted_proxy`，只信任代理追加的最后一跳；前端服务器自行写入 peer IP。 | V3 |
| H-03 | **High** | `backend/src/api/ws.rs:20-31`（未设 `max_message_size`）、`:58`（`unbounded_channel`）、`:171-186`、`:620-652`；`backend/src/infra/hub.rs:14,71-77` | WS 无入站大小/帧限制、无写队列上限；除文字消息外无事件级限流；广播把 payload 克隆进每个无界队列。 | 任意成员/guest 发送 64 MiB 的 `signal`/`send_message`，或不读自己的 socket 同时刷 `media_state`/`reaction`，造成内存暴涨、广播放大、SQLite 写锁争用，最终 OOM/abort。 | 设置 `max_message_size`/`max_frame_size`；改为有界 channel；每类事件 token bucket；限制 `signal.data` 大小和形状；慢消费者断开。 | V4 |
| H-04 | **High** | `backend/src/api/ws.rs:58-91`；`backend/src/infra/hub.rs:29-36`；`release/systemd/cloudroom-server.service:58` | 无每用户/每房间/全局连接上限；同一用户新连接只替换 Hub sender，旧 socket 不会立即关闭；无 idle/ping timeout。 | 一个有效会话打开大量 WS，每连接占 fd/task/channel；Nginx 无 `limit_conn` 时进一步放大，最终耗尽 fd/内存。 | 限制并发连接；替换连接时主动关闭旧任务；设置 WS idle timeout；Nginx `limit_conn`。 | V5 |
| H-05 | **High** | `backend/src/config.rs:261-273`；`backend/src/infra/livekit.rs:34-57`；`backend/src/api/rooms.rs:990-1013`；`backend/src/app/rooms.rs:503-529`；`backend/Cargo.toml:12-40` | LiveKit JWT TTL 默认 2h，且服务端没有 LiveKit HTTP 客户端/`RemoveParticipant`；被踢、主动离开、登出、禁用账号都不会撤销已签发 JWT。 | 被踢/禁用用户用仍有效的 token 直连 LiveKit，继续收听（并可用麦克风发布）SFU 房间音频直到 `exp`；房间码通常可分享。 | 将 `media_token_ttl` 降到 5–10 分钟；在踢人/禁用/删房时调用 LiveKit `RemoveParticipant`，或使用每次成员关系变化的临时 room name；WS 侧同步关闭。 | V6 |
| H-06 | **High** | `backend/src/api/auth_routes.rs:267-305`（`hash_password` 在 `:291`，邀请校验在 `:298` 之后）；无 `rl.allow` | `upgrade-guest` 在验证邀请前执行 Argon2id，且完全没有限流；Argon2 在 async worker 线程同步执行。 | 任意 guest（凭一个房间邀请即可获得）循环发无效 `invite_token`，每次消耗约 19 MiB/CPU，导致 worker 阻塞、内存/CPU 耗尽。 | 先校验邀请和 guest 成员关系，再 `spawn_blocking` 哈希；应用 `invite_redeem_per_hour_per_ip` 或按会话限流。 | V7 |
| H-07 | **High** | `backend/src/infra/rate_limit.rs:24-53`；`backend/src/main.rs:205-211`；`backend/src/api/auth_routes.rs:180-186` | 限流 key 来自攻击者可控的 XFF 和原始 username；key 长度无上限，bucket 无数量上限，保留约 1 小时，10 分钟才 prune。 | 用 8 KB XFF 或 1 MB username 反复请求，每个不同 key 留存；一个 IP 即可在 1 小时内制造数百 MB 到 GB 级 RSS；与 H-02 组合可无限放大。 | 校验/截断 key（IP 45 字节、username 先验证）；限制 bucket 总数/LRU；更频繁 prune；对无效输入用共享 bucket。 | V8 |
| M-01 | Medium | `backend/src/api/ws.rs:33-75,89`；`backend/src/app/admin.rs:109-113`；`backend/src/api/me.rs:48-55` | HTTP 会话删除/密码修改/禁用不会关闭已建立的 WS；WS 只在升级时读一次 `AuthUser`，之后仅检查 Hub 连接。 | 攻击者持有一个已打开的 WS：受害者改密、管理员禁用/重置后，攻击者仍可继续收消息、发消息、发 reaction/signal，直到断开。 | Hub 记录 session id；删除 session/禁用时 `unregister_all`；每个事件或定时重新校验 session/disabled。 | V9 |
| M-02 | Medium | `backend/src/api/admin.rs:313-326`；`backend/src/app/admin.rs:130-163` | 普通 admin 可以重置另一个 admin 的密码；临时密码直接返回且不强制首次改密。 | 低信任 admin 重置 peer admin，登录后以对方身份操作/读取房间，审计只显示 `user.password_reset`；文档称 admin 不看房间内容、只有 super 授予 admin。 | 重置 admin 目标也要求 super；强制首登改密；临时密码一次性/短 TTL；补充审计与文档。 | V10 |
| M-03 | Medium | `backend/src/api/rooms.rs:263-326`；`backend/src/app/sweeper.rs:24-76` | 匿名 `join` 在验证房间/邀请之前就写入 guest `users` + `sessions`；失败不回滚，sweeper 只删过期 session，不删 guest 用户。 | 无邀请或房间不存在也能无限制造 guest/session 行；配合 H-02 可无界增长数据库/WAL，造成磁盘/写锁压力。 | 先解析并验证邀请/房间，再创建 guest；失败时同事务回滚；清理无 session 的 guest 账号。 | V11 |
| M-04 | Medium | `backend/src/infra/avatars.rs:28-64`；`backend/src/api/me.rs:69-101` | `max_avatar_bytes` 只限制压缩后大小；先完整解码再检查 1024×1024，随后 `to_rgb8()` 再分配一份；上传接口无频率限制。 | 任意非 guest 用户上传小体积高像素 PNG（如 8000×8000），单请求造成数百 MB 瞬时分配；并发数次可 OOM；`panic="abort"` 使解码 panic 变成全站中断。 | 先用 `ImageReader::into_dimensions()` 检查尺寸再解码；设置 `image::Limits`；限制上传频率/并发；解码放 `spawn_blocking`。 | V12 |
| M-05 | Medium | `backend/src/api/auth_routes.rs:180-207` | 账号限流 key 用原始 `req.username`，但查询用 `trim()`；同一账号可用空格变体获得新 bucket；未知用户不做 dummy Argon2；登录密码无最大长度校验。 | 对已知账号进行远超 30 次/小时的密码尝试；用超长 username 占用限流内存；通过响应时间差枚举账号。 | key 先 `trim`/规范化；未知用户执行 dummy verify；限制 username/password 长度；错误统一。 | V13 |
| M-06 | Medium（疑似部分） | `backend/src/infra/livekit.rs:121-152`；`backend/src/api/rooms.rs:973-988`；`release/livekit/livekit.yaml.example:27-32` | 每个成员可无限领取 300s TURN 凭证；凭证不绑定房间；relay 端口仅 51 个（50200–50250），`ice-servers` 无限流。 | 成员循环领取并建立 TURN allocation，耗尽 relay 端口；受限 NAT 用户无法加入语音；TURN 还可能被当作短时开放中继（需 LiveKit 侧确认）。 | `ice-servers` 按用户/房间限流；扩大 relay range；限制并发 allocation；评估 TURN 的 peer 白名单/配额。 | V14 |
| M-07 | Medium | `backend/src/main.rs:160-186`；`release/deployment-zh.md:145-155`；`release/systemd/cloudroom-server.service:16-22` | 首次 bootstrap URL（含 raw token，24h 有效）打印到 journald；文档要求从 journal 取。 | 有 `adm`/`systemd-journal` 读权限的本地用户、日志采集/备份系统可在 24h 内取到 token，注册首个 super_admin。 | 输出到仅 root 可读的 0600 文件或 TTY；缩短 bootstrap TTL；把 journal 读取权限视为 root 等价。 | V15 |
| M-08 | Medium | `backend/src/config.rs:344-360`；`release/config.toml.example:46-51`；`release/livekit/livekit.yaml.example:34-36` | 配置校验只检查 LiveKit 三键非空且协议正确；`CHANGE_ME` 可通过；安装脚本不会自动启动，但操作者手动启动即使用已知凭据。 | 用示例配置启动后，知道 `CHANGE_ME` 的人可伪造 LiveKit JWT；TURN username 还会暴露 API key。 | 校验拒绝 `CHANGE_ME`/过短/已知默认 secret；安装脚本硬失败；生成随机 key/secret。 | V16 |
| M-09 | Medium | `backend/src/infra/db.rs:281-360`；`backend/src/api/extract.rs:33-37`；`backend/src/api/ws.rs` 多处 `with_sync` | 单个 `Mutex<Connection>` 串行化所有 SQLite 访问；部分 async handler 在 worker 线程上 `block_in_place`/`with_sync`；无全局请求超时。 | WS 事件风暴或 guest 写入持续占用 DB 锁，其他请求排队，直至 Nginx 3600s 超时；连接/任务堆积。 | 所有 DB 访问进 `spawn_blocking`；读写分离/连接池；加 `tower::timeout` 和每事件写上限。 | V17 |
| L-01 | Low | `backend/src/error.rs:99-110`；`backend/src/api/me.rs:143-151` | 内部错误把 SQLite/IO 错误原文返回客户端；头像读取错误包含路径。 | 触发 500 时可获知表名/列名/服务器路径；当前没有构造出稳定利用链。 | 500 返回通用 `internal_error`，详细日志只写服务端。 | V18 |
| L-02 | Low | `release/nginx/cloudroom.conf.example:37-41,53-69` | `/api/v1/` 内声明了 `add_header Cache-Control`，按 Nginx 规则会丢弃 server 级 HSTS/CSP/XFO/nosniff/Referrer-Policy；模板也无 `limit_req`/`limit_conn`/dotfile deny。 | API 响应缺少安全头；边缘限流缺失使 H-02/H-03 更容易放大；若 webroot 出现 dotfile 会被 try_files 提供。 | 在 API location 重复安全头；加 `limit_req`/`limit_conn`；`location ~ /\. { deny all; }`。 | V19 |
| L-03 | Low | `frontend-server/src/main.rs:264-277,387-435,461-490` | 自带 TLS 的入口只加 `nosniff`，无 CSP/HSTS/XFO/Referrer-Policy；握手 header 读取无超时；每连接无上限；客户端 XFF 原样转发。 | 作为公网入口时存在点击劫持/降级风险，慢速 header/连接耗尽；IP 伪造见 H-02。 | 仅作测试/内网；若公网使用，补安全头、超时、连接上限，并重写 XFF。 | V20 |
| L-04 | Low | `backend/src/util.rs:68-86`；`backend/src/api/rooms.rs:448-454`；`frontend/src/components/MessageList.tsx:90-96` | `is_control()` 只覆盖 Unicode Cc，不覆盖 bidi/零宽 Cf；昵称不去首尾空白且不要求唯一；公告先截断到 500 再检查，`>500` 永不触发。 | 成员把昵称伪装成房主（消息列表只显示 nickname），或用 bidi 控制字符改变显示顺序；公告可含不可见字符。 | 拒绝 bidi/Cf；昵称 trim；公告在截断前校验；展示角色/用户 ID 辅助识别。 | V21 |
| L-05 | Low | `backend/src/config.rs:45-58`；`backend/src/api/routes.rs:121-125`；`docs/configuration.md:52` | `server.max_body_bytes` 声明但未接入 `DefaultBodyLimit`；实际只有 Axum 默认约 2 MiB，文档的 4 MiB 不生效。 | 无直接利用；限制与文档不一致，接近 2 MiB 的头像 multipart 可能先被框架拒绝。 | 在 router 加 `DefaultBodyLimit::max(cfg.server.max_body_bytes)`；头像单独流式限制；更新文档。 | V22 |
| L-06 | Low | `backend/src/config.rs:285,300`；`backend/src/api/auth_routes.rs`；`docs/security.md:36` | `invite_redeem_per_hour_per_ip` 从未使用；文档声称邀请兑换受限。 | 邀请兑换仅受 `room_join`（可绕过）或完全无限（`upgrade-guest`）约束。 | 实现该 bucket 或删除配置/文档声明；与 H-06 一并修。 | V23 |
| L-07 | Low | `frontend/package-lock.json`（react-router 6.30.4）；`npm audit` | `npm audit --omit=dev` 报 2 个 moderate：React Router open redirect（含 backslash bypass）和 SSR `deserializeErrors` 注入；当前应用未使用 SSR，`Link`/`navigate` 目标均来自校验后的房间码或固定路径。 | 若未来把用户输入传给 `Link`/`navigate`，可能开放重定向；SSR 未使用。 | 升级 react-router-dom 到修复版本；保持 `to`/`navigate` 只接受内部、已校验路径。 | V24 |
| L-08 | Low | `release/systemd/cloudroom-server.service:16-65` | 无 `UMask=`；SQLite/备份文件可能按 0644 创建（受 0750 数据目录保护）；`LimitNPROC=256` 与 Tokio blocking 线程需求不匹配。 | 同组/同机用户读取数据文件的风险；极端并发下线程创建失败/abort。 | 增加 `UMask=0077`；提高/配置 blocking 线程；评估 `LimitNPROC`。 | V25 |
| L-09 | Low | `release/deployment-zh.md:210-233` | 备份是手工 `.backup`，默认未加密；无自动备份/恢复演练。 | 数据库/备份泄露或主机故障导致数据丢失。 | 使用 age/restic/borg 加密；定期恢复演练；备份权限 0600。 | V26 |
| L-10 | Low | `release/nginx/cloudroom.conf.example:41`；`frontend/index.html:6` | 无 `Permissions-Policy`；CSP `connect-src 'self' wss: https:` 过宽，缺 `frame-ancestors`（由 XFO 补偿）。 | XSS 后可向任意 https/wss 外联；浏览器权限策略未收紧。 | 收窄 `connect-src` 到 LiveKit 域名；加 `frame-ancestors 'none'`、`Permissions-Policy`。 | V27 |
| L-11 | Low | `backend/src/api/auth_routes.rs:267-275` | cookie 认证的 `upgrade-guest` 没有 `require_csrf`/Origin 检查。 | 目前浏览器跨站 JSON 预检 + SameSite=Lax 阻止；属防御纵深缺口，未来改 content-type/代理时可能变成 CSRF。 | 改为 `AuthUser` 并调用 `require_csrf`。 | V28 |
| L-12 | Low | `backend/src/config.rs:39-40,431-441`；`backend/src/api/routes.rs` | 文档把 `allowed_origins` 描述为 CORS，后端没有 CORS layer，跨域前端无法实际调用 API；这同时使 CSRF 面更小。 | 功能/文档不一致；不是漏洞。 | 明确只支持同源，或补严格 CORS 并保持凭据模式限制。 | V29 |
| L-13 | Low | `backend/src/api/auth_routes.rs:59-94` | bootstrap token 在创建用户前被消费；用户名冲突/创建失败会烧掉 24h bootstrap。 | 持有 bootstrap link 的人可被已知用户名冲突阻断首次注册（需先获得 link）。 | 先验证用户名/哈希，成功创建用户后再原子消费；或失败时恢复 token。 | V30 |

---

## 3. 公网部署前必须修（Critical/High）

**结论：当前代码不能直接作为公网高信任服务上线。** 在完成以下项前，不应把服务暴露到公网：

1. **C-01 部署包去测试化**：换随机 LiveKit key/secret；受信证书；`insecure_cookies=false`；补齐 HSTS/CSP/XFO/nosniff/Referrer-Policy；只暴露 80/443（以及实际需要的 LiveKit 端口），不要把测试 `.env`/自签证书带上公网。
2. **H-01 修 panic**：mute 时长使用 checked API 并限幅；release 不应让单个请求 abort 整个进程。
3. **H-02/H-07 修真实客户端 IP 与限流内存**：代理覆盖 XFF；后端验证 peer；限 key 长度/bucket 数量；否则所有 IP 限流等于没有。
4. **H-03/H-04 修 WS 资源边界**：入站大小/帧限制、有界发送队列、事件限流、连接上限、替换连接关闭旧 socket。
5. **H-05 LiveKit 撤销**：缩短 TTL，并在踢人/禁用/删房时从 LiveKit 移除参与者，或使用不可长期复用的房间名。
6. **H-06 upgrade-guest**：先校验邀请再哈希、加限流、哈希移出 async worker。
7. **M-01/M-02**：至少保证禁用/改密/重置后已建立 WS 关闭；普通 admin 不应能接管 peer admin。
8. **M-03/M-04**：匿名 join 不落孤儿数据；头像先验尺寸再解码。
9. **M-09**：为 DB 访问加超时/并发边界，避免一个事件流拖垮全站。

## 4. 可接受风险 / 已补偿

以下不是「无风险」，而是当前产品边界；公网部署前应由部署方书面接受：

- **无 E2EE**：服务器/运维者可读文字历史；`docs/security.md` 已声明。高敏感内容不应使用本系统。
- **SQLite 不加密**：依赖磁盘加密、目录 0750、加密备份；否则数据库/备份泄露。
- **P2P 语音禁言是合作式的**：被禁言客户端可不遵守；SFU 路径也只控制诚实客户端。恶意成员仍可通过自己的麦克风/客户端绕过本地静音。
- **房间码不是凭证**：依赖成员/邀请校验；不要仅凭房间码分享敏感内容。
- **30 天会话**：被盗 Cookie 可长期使用；建议缩短 `session_ttl`，至少对管理员缩短。
- **npm moderate 依赖**：当前路由输入受校验，未发现可达开放重定向；仍应升级。
- **无自动备份**：手工加密备份可接受，但必须有恢复演练。

---

## 5. 逐条验证命令

> 所有命令只对本地/隔离测试实例执行；不要在生产执行会崩溃或压力测试的命令。`<host>` 替换为测试域名，`<cookie>`/`<csrf>` 用测试账号。

### V1 测试部署包默认值/头

```bash
# 不打印 secret 值，只确认是否仍是默认/已设置
awk -F= '/^LIVEKIT_API_KEY=/{print "key=" $2} /^LIVEKIT_API_SECRET=/{print "secret_len=" length($2)}' \
  ../cloudroom-compose/.env
grep -n 'insecure_cookies' ../cloudroom-compose/cloudroom/config.template.toml
curl -sI https://<host>/ | grep -Ei 'strict-transport|content-security|x-frame|x-content-type|referrer-policy'
curl -sI https://<host>/api/v1/health | grep -Ei 'strict-transport|content-security|x-frame|x-content-type|referrer-policy'
```

期望：secret 不是测试默认值；`insecure_cookies=false`；两处响应都有完整安全头。

### V2 验证 mute 时长 panic（危险，仅隔离实例）

```bash
# 先在隔离实例获取 owner 会话 cookie 与 csrf token；此请求会 abort 进程
curl -si -X POST "https://<host>/api/v1/rooms/<room>/mutes" \
  -b "cloudroom_session=<cookie>" \
  -H "X-CSRF-Token: <csrf>" -H 'Content-Type: application/json' \
  -d '{"scope":"room","duration_seconds":9223372036854775807}'
```

期望：修复后返回 422，服务仍存活；未修复时连接重置/日志出现 abort。

### V3 验证 XFF 绕过

```bash
for i in $(seq 1 12); do
  curl -s -o /dev/null -w '%{http_code} ' \
    -X POST "https://<host>/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -H "X-Forwarded-For: 198.51.100.$i" \
    -d '{"username":"probe","password":"wrong"}'
done; echo
# 对照：不带 XFF 的同一循环，应在 10 次后出现 429
```

期望：修复后 12 次请求仍在 IP 限流内/出现 429；未修复时全部 401。

### V4 验证 WS 大小/事件/队列

```bash
# 需要 websocat；先获取有效房间 cookie 与 Origin
python3 - <<'PY' > /tmp/big.json
print('{"type":"send_message","body":"' + 'A'*20000000 + '"}')
PY
websocat -H="Cookie: cloudroom_session=<cookie>" -H="Origin: https://<host>" \
  "wss://<host>/api/v1/rooms/<room>/ws" < /tmp/big.json
# 另一个终端对同一连接快速发送 media_state/reaction，同时用 ps 观察 RSS
ps -o rss= -p "$(pgrep -f cloudroom-server)"
```

期望：修复后大帧被关闭/拒绝；RSS 不随事件数量无界增长。

### V5 验证 WS 连接上限/旧连接

```bash
# 用同一会话打开多个连接；记录 fd/线程/RSS
for i in $(seq 1 200); do
  websocat -H="Cookie: cloudroom_session=<cookie>" -H="Origin: https://<host>" \
    "wss://<host>/api/v1/rooms/<room>/ws" >/dev/null 2>&1 &
done
ps -o rss= -p "$(pgrep -f cloudroom-server)"
ls /proc/$(pgrep -f cloudroom-server)/fd | wc -l
```

期望：修复后有明确上限并拒绝超额连接；未修复时连接/fd 持续增长。

### V6 验证 LiveKit 旧 token

```bash
# 1) 加入 sfu 房间并获取 token
curl -s -X POST "https://<host>/api/v1/rooms/<room>/media-token" \
  -b "cloudroom_session=<cookie>" -H "X-CSRF-Token: <csrf>" \
  -H 'Content-Type: application/json' -d '{}' > /tmp/token.json

# 2) 解码 exp（只读）
python3 - <<'PY'
import base64, json
p=json.load(open('/tmp/token.json'))['token'].split('.')[1]
print(json.loads(base64.urlsafe_b64decode(p+'='*(-len(p)%4)))['exp'])
PY

# 3) 让管理员移除该成员（HTTP 404/403 或 200），然后用旧 token 直接连 LiveKit
# 使用 livekit-client/测试脚本 connect(wss://<livekit>, old_token)；观察是否仍能订阅音频
```

期望：移除后旧 token 应被 LiveKit 拒绝（或 TTL 已极短）；未修复时可继续连接。

### V7 验证 upgrade-guest 资源消耗

```bash
# 用 guest 会话循环发送无效邀请；观察延迟/CPU/RSS
for i in $(seq 1 50); do
  curl -s -o /dev/null -w '%{http_code} %{time_total}\n' \
    -X POST "https://<host>/api/v1/auth/upgrade-guest" \
    -b "cloudroom_session=<guest-cookie>" -H 'Content-Type: application/json' \
    -d '{"username":"aaaa","password":"bbbbbbbb","invite_token":"invalid"}'
done
```

期望：修复后出现 429/快速失败；未修复时全部 400 且每次都有 Argon2 延迟。

### V8 验证限流内存

```bash
# 生成不同长度的 username；只观察 RSS，不打印内容
for i in $(seq 1 100); do
  u=$(printf 'a%.0s' $(seq 1 100000))
  curl -s -o /dev/null -X POST "https://<host>/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    --data-binary "{\"username\":\"$u$i\",\"password\":\"x\"}"
done
ps -o rss= -p "$(pgrep -f cloudroom-server)"
```

期望：修复后 username 在限流前被长度限制；RSS 不随请求线性增长。

### V9 验证禁用/改密后 WS

```bash
# A 打开 WS；管理员禁用/改密 A；A 的旧 WS 继续发消息
websocat -H="Cookie: cloudroom_session=<A-cookie>" -H="Origin: https://<host>" \
  "wss://<host>/api/v1/rooms/<room>/ws"
# 在另一个 shell：
curl -s -X POST "https://<host>/api/v1/admin/users/<A-id>/disable" \
  -b "cloudroom_session=<admin-cookie>" -H "X-CSRF-Token: <admin-csrf>" \
  -H 'Content-Type: application/json' -d '{"disabled":true}'
# 在旧 WS 输入：
{"type":"send_message","body":"still here"}
```

期望：修复后旧 WS 关闭或发消息被拒；未修复时消息仍写入。

### V10 验证 admin 重置 peer admin

```bash
# admin A 重置 admin B（应 403；当前会 200 + temporary_password）
curl -si -X POST "https://<host>/api/v1/admin/users/<B-id>/reset-password" \
  -b "cloudroom_session=<A-cookie>" -H "X-CSRF-Token: <A-csrf>" \
  -H 'Content-Type: application/json' -d '{}'
```

### V11 验证匿名 join 孤儿行

```bash
sqlite3 /var/lib/cloudroom/cloudroom.db 'select count(*) from users; select count(*) from sessions;'
for i in $(seq 1 30); do
  curl -s -o /dev/null -X POST "https://<host>/api/v1/rooms/nosuchcode/join" \
    -H 'Content-Type: application/json' -d '{}'
done
sqlite3 /var/lib/cloudroom/cloudroom.db 'select count(*) from users; select count(*) from sessions;'
```

期望：修复后计数不变；未修复时增加。

### V12 验证头像解码

```bash
python3 - <<'PY'
from PIL import Image
Image.new('RGB',(8000,8000)).save('/tmp/bomb.png')
PY
ls -lh /tmp/bomb.png
# 用有效会话/CSRF 上传；同时观察 RSS
curl -s -X POST "https://<host>/api/v1/me/avatar" \
  -b "cloudroom_session=<cookie>" -H "X-CSRF-Token: <csrf>" \
  -F "avatar=@/tmp/bomb.png" >/dev/null
ps -o rss= -p "$(pgrep -f cloudroom-server)"
```

期望：修复后在上传早期返回 400/422；RSS 不应出现数百 MB 峰值。

### V13 验证登录限流 key

```bash
for u in 'alice' 'alice ' 'alice  '; do
  for i in $(seq 1 31); do
    curl -s -o /dev/null -w '%{http_code} ' -X POST "https://<host>/api/v1/auth/login" \
      -H 'Content-Type: application/json' \
      -d "{\"username\":\"$u\",\"password\":\"wrong\"}"
  done; echo
done
```

期望：修复后空格变体共用同一 bucket，30 次后 429；未修复时每组仍可继续。

### V14 验证 TURN 分配

```bash
# 仅测试实例；重复获取凭证，观察 relay 端口/allocation
for i in $(seq 1 100); do
  curl -s "https://<host>/api/v1/rooms/<p2p-room>/ice-servers" \
    -b "cloudroom_session=<member-cookie>" >/dev/null
done
# 如安装了 turnutils_uclient/turnutils 或 LiveKit 管理工具，检查 allocation 数量
```

期望：修复后出现 429 或 allocation 上限；未修复时可持续领取。

### V15 验证 bootstrap token 日志

```bash
sudo journalctl -u cloudroom-server -n 100 --no-pager | grep -i bootstrap
```

期望：修复后只输出不含 raw token 的提示，或 token 只写 root-only 文件。

### V16 验证默认 LiveKit 占位符

```bash
grep -n 'CHANGE_ME' release/config.toml.example release/livekit/livekit.yaml.example
# 用示例配置启动隔离实例；应因占位符拒绝启动
```

期望：启动失败；当前会启动。

### V17 验证 DB 串行/超时

```bash
# 测试实例：一个循环发 reaction/消息，另一个测 /health 延迟
time curl -s https://<host>/api/v1/health >/dev/null
# 可用 e2e/websocat 压 reaction，再重复 health 延迟
```

期望：修复后 P99 有上界；未修复时健康检查可能被拖慢。

### V18 验证错误泄露

```bash
# 找一个会触发 500 的测试输入（勿在生产）
curl -si https://<host>/api/v1/me/avatar -b "cloudroom_session=<cookie>"
# 观察 500 body 是否含 sqlite/path
```

### V19 验证 Nginx 头/边缘限制

```bash
sudo nginx -t
curl -sI https://<host>/api/v1/health | grep -Ei 'strict-transport|content-security|x-frame|x-content-type|referrer-policy'
grep -nE 'limit_req|limit_conn|proxy_add_x_forwarded_for' /etc/nginx/sites-available/cloudroom.conf
```

### V20 验证 frontend-server 头

```bash
curl -sI https://<host>:8081/ | grep -Ei 'strict-transport|content-security|x-frame|referrer-policy'
```

### V21 验证 bidi/昵称

```bash
curl -s -X POST "https://<host>/api/v1/rooms/<room>/members/<uid>/nickname" \
  -b "cloudroom_session=<member-cookie>" -H "X-CSRF-Token: <csrf>" \
  -H 'Content-Type: application/json' \
  -d '{"nickname":"\u202eowner"}'
```

期望：修复后 422；未修复时 200。

### V22 验证 body limit

```bash
python3 -c 'print("{\"username\":\"a\",\"password\":\""+"x"*5000000+"\"}")' > /tmp/body.json
curl -si -X POST https://<host>/api/v1/auth/login \
  -H 'Content-Type: application/json' --data-binary @/tmp/body.json
```

期望：修复后按配置返回 413；当前由 Axum 默认约 2 MiB 拒绝，错误不体现 4 MiB 配置。

### V23 验证 invite_redeem 配置

```bash
grep -rn 'invite_redeem_per_hour_per_ip' backend/src
# 只有 config.rs 定义 → 没有实现
```

### V24 验证 npm 依赖

```bash
cd frontend
npm audit --omit=dev
```

期望：0 漏洞；当前 2 个 moderate（react-router）。

### V25 验证 systemd 权限/UMask

```bash
stat -c '%a %U:%G %n' /var/lib/cloudroom /var/lib/cloudroom/cloudroom.db
systemd-analyze security cloudroom-server.service --no-pager
grep -nE 'UMask|LimitNPROC' /etc/systemd/system/cloudroom-server.service
```

### V26 验证备份

```bash
sudo -u cloudroom sqlite3 /var/lib/cloudroom/cloudroom.db \
  ".backup '/var/lib/cloudroom/backups/test.db'"
stat -c '%a %U:%G %s %n' /var/lib/cloudroom/backups/test.db
# 恢复演练必须在隔离环境进行
```

### V27 验证 CSP/Permissions

```bash
curl -sI https://<host>/ | grep -Ei 'content-security|permissions-policy|frame-ancestors'
```

### V28 验证 upgrade-guest CSRF

```bash
grep -n 'upgrade_guest' -A8 backend/src/api/auth_routes.rs
# 代码中无 require_csrf；跨站表单应 415，JSON 预检应失败
```

### V29 验证 CORS 行为

```bash
curl -si -X OPTIONS https://<host>/api/v1/me \
  -H 'Origin: https://evil.example' \
  -H 'Access-Control-Request-Method: GET'
# 当前无 Access-Control-Allow-Origin；跨域前端不可用
```

### V30 验证 bootstrap 冲突消耗

```bash
# 测试实例：用 bootstrap link 注册一个已存在的用户名，再检查 bootstrap token 状态
sqlite3 /var/lib/cloudroom/cloudroom.db \
  'select consumed_at from bootstrap_tokens order by created_at desc limit 1;'
```

期望：修复后失败注册不应消费 token。

---

## 6. 公网部署检查清单

### TLS / Cookie

- [ ] 使用受信 CA 证书（`openssl verify`/`curl --cacert` 均通过），不使用自签证书公网暴露。
- [ ] `server.insecure_cookies=false`；生产 Cookie 为 `__Host-cloudroom_session; Secure; HttpOnly; SameSite=Lax`。
- [ ] 只开放 80（ACME/跳转）和 443；HSTS 已由主文档响应下发。
- [ ] 禁止 `http://` 明文 API；Nginx 80 只 301 到 HTTPS。

### Nginx

- [ ] `X-Forwarded-For` 由代理**覆盖**为 `$remote_addr`，或使用受信 `X-Real-IP`；禁止 `$proxy_add_x_forwarded_for` 直接给后端。
- [ ] `/api/v1/` 重复 HSTS/CSP/XFO/nosniff/Referrer-Policy；增加 `Permissions-Policy` 与 `frame-ancestors 'none'`。
- [ ] 增加 `limit_req`/`limit_conn`（登录、注册、WS、上传），与后端限流叠加。
- [ ] `location ~ /\. { deny all; }`；关闭 `server_tokens`；静态目录不包含 `.env`/source map。
- [ ] WSS `Upgrade`/`Connection`、`proxy_read_timeout`/`proxy_send_timeout` 配置正确；LiveKit 媒体 UDP 不经 Nginx。
- [ ] 只暴露必要的 LiveKit 端口；不要把 7880 明文信令端口作为公网入口（优先 Nginx WSS `/rtc`）。

### systemd / 主机

- [ ] 非 root `cloudroom`/`livekit` 用户；`ProtectSystem=strict`、`NoNewPrivileges`、空 `CapabilityBoundingSet`。
- [ ] `/etc/cloudroom/config.toml` 0640 root:cloudroom；`/etc/livekit.yaml` 0640 root:livekit；数据目录 0750。
- [ ] 增加 `UMask=0077`；数据库/备份文件不可被其他本地用户读取。
- [ ] `LimitNOFILE`/`LimitNPROC` 与 Tokio/WS 负载匹配；监控重启/OOM。
- [ ] bootstrap token 不进入可被低权限用户读取的日志；journal 访问视为 root 等价。

### 密钥/账号

- [ ] 替换所有 `CHANGE_ME`/`devkey`/`devsecret`；LiveKit API key/secret 随机且只存服务端。
- [ ] `registration_mode="invite_only"`；开放注册必须明确批准。
- [ ] 首次 super_admin 创建后检查 bootstrap 已关闭；删除测试账号。
- [ ] admin 仅授予必要人员；定期复核；普通 admin 不应能重置 peer admin（修 M-02 后）。
- [ ] 强制管理员/用户使用密码管理器；考虑更短 session TTL。

### 数据/运维

- [ ] 备份使用 `.backup` 并加密（age/restic/borg）；恢复演练通过；备份文件 0600。
- [ ] 日志不记录 Cookie/密码/token/请求体；访问日志只记录方法/路径/状态。
- [ ] 监控 5xx、RSS/OOM、WS 连接数、SQLite 写延迟、TURN allocation。
- [ ] 依赖门禁：`cargo audit` 两个锁文件 + `npm audit` 纳入 CI；LiveKit 二进制校验 sha256/签名。
- [ ] 保留 `docs/security.md` 的已知限制，不把本地自签/fake device 测试写成生产验证。

---

## 7. 未覆盖项与假设

- **未实际启动生产环境**：没有真实域名/CA、Nginx、systemd、LiveKit 公网、真实麦克风；因此 C-01、TLS、防火墙、LiveKit/TURN 行为均需部署方复验。
- **未执行破坏性测试**：未触发 H-01 panic、未做压力/DoS、未对外部目标请求；第 5 节中标注「危险」的命令只能在隔离实例执行。
- **LiveKit TURN 行为疑似**：代码显示 TURN 凭证不绑定房间，但 LiveKit 内嵌 TURN 是否允许任意 peer 中继（开放中继/SSRF）未在本地验证；M-06 的「任意 peer」部分需 LiveKit 侧测试确认。
- **LiveKit 二进制未审计**：`release/livekit-server` 是第三方二进制，未做源码/签名验证；供应链风险仍在。
- **npm 两个 moderate**：React Router 的开放重定向在当前路由输入受校验的前提下未构造出可达利用；SSR `deserializeErrors` 未使用。仍需升级。
- **没有 CORS**：当前仅支持同源；`allowed_origins` 只用于 Origin/WS 检查，不是 CORS 配置。跨域前端部署不可用，不是漏洞。
- **历史数据**：历史 owner 角色/`left_reason=NULL`、旧会话等数据问题不在本次代码审查范围；按 `docs/debt.md` 与迁移计划处理。
- **严重度**：基于当前代码与默认配置；若部署方关闭 invite_only、开放注册、使用测试密钥或把后端直接暴露公网，风险会显著升高。

---

## 附录 A–J 清单结论

### A 认证与会话

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| 密码哈希/参数 | **存在** | Argon2id，默认 m=19456/t=2/p=1；`backend/src/infra/auth.rs:30-55`，`config.rs:92-104` |
| 登录失败区分用户/密码 | **不存在（消息一致）**；**存在时间侧信道** | 统一「用户名或密码错误」`auth_routes.rs:189-200`；未知用户跳过 Argon2，存在时间差 |
| Cookie HttpOnly/Secure/SameSite/Path/过期 | **生产存在；测试包可关闭 Secure** | `infra/auth.rs:80-103`；`insecure_cookies` `config.rs:43-44` |
| 会话固定 | **不存在** | 每次登录/注册新 session token `auth_routes.rs:235-255` |
| 登出失效 | **存在（当前会话）** | `auth_routes.rs:218-233` |
| 改密撤销其他会话 | **HTTP 存在；WS 不存在** | `api/me.rs:48-55`；WS 不重校验 M-01 |
| 密码重置/临时密码 | **存在**；**无强制改密** | `api/admin.rs:313-326`；M-02 |
| 重置 token 熵/过期 | **存在（256-bit；无独立过期，靠密码策略）** | `util.rs:23-28`；`api/admin.rs:323-325` |
| 登录/注册限流 | **部分存在，可绕过** | `auth_routes.rs:35-41,171-187`；H-02/H-07/M-05 |
| 重置限流 | **不存在** | 无 `rl.allow` in reset |
| bootstrap/初始管理员 | **存在但需保护** | `main.rs:85-117,160-186`；M-07 |
| 默认凭据 | **无默认账号；示例密钥可启动** | `config.rs:344-360`；M-08 |

### B 授权

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| 对象级房间/消息授权 | **存在** | `app/access.rs:9-49`；`api/rooms.rs:872-920` |
| 成员/邀请/设置/禁言授权 | **存在** | `api/rooms.rs:521-815`；owner caps `access.rs:33-49` |
| 头像对象授权 | **仅登录可读，UUID 随机** | `api/me.rs:143-161`；`infra/avatars.rs:81-92` |
| 角色集中服务端 | **存在** | `domain/models.rs:40-47`；`api/admin.rs:15-29` |
| 前端隐藏是否被误当授权 | **否（后端复核）** | 所有 admin/房主写操作均有 `require_admin`/`require_cap` |
| 离开/被踢访问 | **已修复，无回归** | `access.rs:24-26`；`app/rooms.rs:242-260`；M-01 仅 WS |
| 房主转让/多 owner | **存在且集中** | `app/admin.rs:41-91`；`repo.rs:1004-1016` |
| guest 权限 | **受限** | `api/rooms.rs:52-54`；guest 不能建房/上传，但可建 DM（H-01 前置） |
| admin 自我提权 | **未发现** | `set_user_role` super-only/拒绝 self；`api/admin.rs:334-357` |
| admin 重置 peer admin | **存在权限边界问题** | M-02 |

### C CSRF / 跨域 / 浏览器边界

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| 状态变更 CSRF | **基本存在；`upgrade-guest` 例外** | `api/security.rs:29-54`；`auth_routes.rs:267`；L-11 |
| Origin/Sec-Fetch-Site | **存在** | `security.rs:32-45` |
| CORS | **未实现**（仅同源） | 无 CORS layer；`config.rs:431-441` 只用于检查；L-12 |
| WS Origin/鉴权 | **存在**；无 Origin 的非浏览器请求被放行 | `security.rs:57-67`；`ws.rs:20-31` |
| 点击劫持 | **Nginx 模板有 XFO；Compose/frontend-server 缺失** | `nginx/...:40`；C-01/L-03 |
| 安全响应头 | **Nginx 模板部分；API location 丢继承** | `nginx/...:37-41,69`；L-02 |

### D 输入与注入

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| SQL 参数化 | **存在** | `repo.rs` 全量查询；动态部分仅固定列/WHERE/占位符 |
| 动态 SQL 拼接 | **存在但受控** | `repo.rs:338-344,685-695,769-824`；值均绑定 |
| 路径穿越（头像/静态） | **未发现可利用** | `infra/avatars.rs:81-92`；`frontend-server/main.rs:387-407` |
| 命令注入 | **未发现** | 无 Rust 子进程；脚本路径带引号；systemd 固定 Exec |
| JSON 深度/大小 | **默认有深度上限；大小配置未接入** | L-05；Axum 默认约 2 MiB |
| multipart 大小 | **仅框架默认；config 未接入** | L-05；`api/me.rs:84-95` |
| 请求超时 | **不存在（应用层）** | M-09；依赖 Nginx/前端服务器 |

### E XSS 与内容

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| dangerouslySetInnerHTML/innerHTML | **不存在** | 前端 grep 0 命中；`MessageList.tsx` 纯 React 文本 |
| 消息/昵称/公告/邀请备注渲染 | **文本转义** | `MessageList.tsx:90-96`；`Room.tsx:767-770`；`InvitesTab.tsx:56-70` |
| 链接协议白名单 | **存在（仅 https?）** | `MessageList.tsx:48-60`；`rel="noopener noreferrer"` |
| @提及/reaction | **文本/表情白名单** | `MessageList.tsx:24-44`；`ws.rs:565` |
| SVG/上传头 | **拒绝 SVG，重编码 PNG，nosniff** | `infra/avatars.rs:13-64`；`api/me.rs:151-157` |
| CSP/HSTS/Referrer/Permissions | **Nginx 模板有部分；缺 Permissions/Compose 全缺** | L-02/L-03/L-10；C-01 |

### F WS 与实时面

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| 升级鉴权 | **存在** | `ws.rs:20-31`；`extract.rs:52-62` |
| 每事件授权 | **存在（房间/角色/目标复核）** | `ws.rs:186-653`；`access.rs` |
| 事件字段校验 | **部分**；`signal.data` 无形状校验 | `ws.rs:783-824`；L-11/Input |
| 消息大小/频率 | **仅 chat 有 60/min；WS 无大小/帧限制** | H-03；`messages.rs:36-43` |
| 连接数/资源释放 | **无上限；旧连接不立即关闭** | H-04；`hub.rs:29-36` |
| 断线重连 | **存在 8s 宽限，但 teardown 可被滥用** | `ws.rs:686-732`；M-03/H-03 |
| 广播成员范围 | **正确（按 room_id）** | `hub.rs:71-77`；`ws.rs` broadcast |

### G LiveKit / 媒体

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| JWT TTL/grants | **2h；仅 microphone publish + subscribe** | `config.rs:273`；`infra/livekit.rs:34-51` |
| room 名注入 | **不存在** | room code 校验；JSON 编码 `livekit.rs:37-51` |
| secret 存储 | **仅服务端配置；不返回浏览器** | `config.rs`/`livekit.rs`；`user_json`/room JSON 不含 |
| 被踢/离开后 token | **仍有效至 exp** | H-05；`remove_member` 不调用 LiveKit |
| public URL/TURN 暴露 | **由部署决定；Compose 直接发布 7880/媒体端口** | C-01；`livekit.yaml.example:14-32` |
| TURN 凭证范围 | **300s，不绑定房间** | M-06；`livekit.rs:121-152` |

### H 限流与 DoS

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| 登录/注册/加入 | **存在但可绕过** | H-02/H-07/M-05 |
| 邀请兑换 | **配置未实现；upgrade 无限流** | L-06/H-06 |
| 建房申请 | **存在（10/hour/user）** | `api/rooms.rs:55-62` |
| 发消息 | **存在（60/min/user）** | `messages.rs:36-43` |
| 上传 | **不存在限流** | M-04 |
| 直接房 | **无独立限流** | `api/rooms.rs:929-952` |
| WS 事件 | **仅 chat 有限流** | H-03 |
| SQLite 写锁/慢请求 | **单 mutex；无超时** | M-09 |
| 连接数 | **无应用上限** | H-04 |

### I 部署与运维

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| systemd 用户/权限 | **存在加固**；无 UMask | `release/systemd/...:16-65`；L-08 |
| 文件/数据库权限 | **目录 0750；文件依赖 umask** | `install.sh:25-28`；L-08 |
| Nginx TLS/代理/头/限流 | **部分；XFF/头继承/limit 有问题** | H-02/L-02 |
| 备份/恢复 | **手工、默认未加密** | L-09 |
| 日志敏感信息 | **访问日志脱敏；bootstrap token 入 journal** | M-07 |
| 错误泄露 | **500 可能含 SQL/路径** | L-01 |
| source map/调试端点 | **无 sourcemap/调试路由** | `vite.config.ts`；`routes.rs` |
| 默认账号/开关 | **无默认账号；示例密钥/测试包需处理** | C-01/M-08 |

### J 依赖与供应链

| 项 | 结论 | 证据 / 说明 |
|---|---|---|
| cargo audit | **通过（本次执行）** | backend 0、frontend-server 0；exit 0 |
| npm audit | **2 moderate（react-router）** | `npm audit --omit=dev`；L-07 |
| 未维护依赖 | **本次未发现**（仅静态/audit） | lockfiles |
| 构建脚本网络拉取 | **未发现未固定版本** | `build-musl.sh` 只检查工具链；`build-release.sh` 用 `--locked`/`npm ci` |
| lockfile 入库 | **存在** | `backend/Cargo.lock`、`frontend-server/Cargo.lock`、`frontend/package-lock.json` |
| LiveKit 二进制校验 | **不存在** | `install.sh:36-42` 直接安装，无 sha256/签名 |

---

## 8. 结论

- **公网高信任部署：不安全，需先修 Critical/High。** 最优先：部署包去测试化、H-01 panic、H-02/H-07 真实 IP/限流内存、H-03/H-04 WS 资源边界、H-05 LiveKit 撤销、H-06 upgrade-guest 限流。
- **代码层的认证/对象授权基础总体扎实**：Argon2id、`__Host-` Cookie、CSRF token、服务端房主/admin 检查、参数化 SQL、无 DOM XSS、SVG/头像路径处理均有明确实现。
- **主要风险集中在资源边界和媒体撤销**，不是传统 SQLi/XSS：一个已进房成员即可利用 WS/上传/私聊房触发全站 DoS；被踢/禁用用户的媒体 token 仍有效。
- **部署配置与代码同等重要**：测试 Compose 包、默认 LiveKit 密钥、自签证书、`insecure_cookies=true`、缺失安全头和可伪造 XFF 都会把「代码看起来还行」变成公网事故。

本报告只读产出；除写入本文件外，没有修改仓库代码、配置或数据库，也没有提交。
