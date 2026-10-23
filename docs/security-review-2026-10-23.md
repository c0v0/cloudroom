# CloudRoom 公网安全审查（2026-10-23，修复后复验）

- 审查对象：`/home/hui/agents/omp/cloudroom`，分支 `fix/security-wave2`。
- 代码审查基线：`25be297`；随后两个收尾 guard `75aa38f`（heartbeat > 0、DM 不接受邀请）未引入新的安全面。
- 方式：独立只读安全审查会话（最终 auth/WS、data/deploy、confirmation 三轮）；未修改代码/配置/数据库，未对外部目标发请求，未执行破坏性 payload。
- 前置报告：`docs/security-review-2026-10-22.md`、`docs/plans/2026-10-23-security-wave1.md`、`docs/plans/2026-10-23-security-wave2.md`。
- 部署包 `../cloudroom-compose` 不在此 Git 仓库；已同步修改，但未进入提交。

> **2026-10-23 后续更新：** 本报告中的 LiveKit 媒体撤销 High residual 已实现服务端
> best-effort 撤销（`RemoveParticipant`/`DeleteRoom`，60s admin JWT，内部 `/twirp`）。
> 真实 LiveKit 已验证踢人后 `ListParticipants` 为空、删房成功；仍保留“无持久队列，
> 故障时回退到 TTL”的 best-effort 限制。下文保留审查时点原文。

## 结论

**未发现新的 Critical，也未发现未修复的 Critical/High（除已明确接受的 LiveKit 媒体撤销 residual）。**

- Wave1/Wave2 修复后的主要认证、授权、CSRF、SQL、XSS、头像、XFF、WS 资源边界、清理路径均已验证。
- **LiveKit 媒体撤销仍是 High 类风险，按“接受”处理**：TTL 已降到 10 分钟、生产拒绝 >15 分钟，但没有 `RemoveParticipant`，被踢/禁用用户已建立的 LiveKit 连接仍可继续到过期（现有连接甚至可能更久，取决于 LiveKit 对 exp 的行为）。
- 因此：**邀请制、可信成员的自托管部署可上线**；**高对抗公网 SFU 场景应在补 LiveKit 撤销 API、TURN 配额和边缘限流后再上线**。

## 本轮验证通过的修复

| 修复 | 证据 |
|---|---|
| 生产默认拒绝不安全配置 | `backend/src/config.rs`：`development=false` 默认；拒绝 insecure cookie、HTTP origin、ws LiveKit、空/占位 secret、>15m media TTL |
| 密码/会话 | 登录 key trim、dummy Argon2、Argon2 全部 `spawn_blocking`；密码修改 5/min；账号 bucket 耗尽仍允许正确密码 |
| WS 资源与撤销 | `ws.rs` 消息/帧大小、有界队列、事件预算、升级限流、会话到期定时器；`hub.rs` 用户/全局连接上限、watch 关闭、慢消费者关闭；disable/reset/password/logout/kick/删房关闭 socket |
| XFF/限流 | 仅匹配 `trusted_proxy` 的 TCP peer 才使用 XFF；取最右跳；Nginx 覆盖 `$remote_addr`；frontend-server 用真实 peer 重写；限流 key 截断/桶淘汰 |
| guest/邀请 | 匿名 join 失败不落孤儿；upgrade guest 先验证邀请再 Argon2、加 CSRF/限流；所有 guest FK 重映射；邀请 TTL 1s..365d checked |
| 头像 | header 尺寸预检、`image::Limits`、上传/删除 CAS、启动 orphan 清理、读取异步 + 600/min |
| bootstrap/日志 | bootstrap URL 0600 文件、不打印 raw token；Nginx access log off；Compose `/rtc` error log 关闭 |
| 部署/运维 | 4MiB body / auth 64KiB、5xx generic、bidi 过滤、UMask=0077、systemd/LiveKit 占位符 guard、Compose 去测试 secret/自签、7880 不再发布 |
| sweeper | `purge_room` 不再嵌套事务；处理 `room_requests.created_room_id`、`direct_pairs`、递归子房；grace purge 回归测试通过 |
| presence | 每连接 heartbeat 刷新 user/member online，空闲但连接的 DM 不再被误删 |

## 剩余风险

| 严重度 | 位置 | 问题 | 建议 |
|---|---|---|---|
| **High（接受）** | `backend/src/infra/livekit.rs:34-57`；`backend/src/api/rooms.rs` remove/leave；`backend/src/app/rooms.rs` terminate | LiveKit 媒体凭证只靠 TTL；kick/disable/room delete 不调用 `RemoveParticipant`/`DeleteRoom`。 | 补 LiveKit RoomService 客户端并调用；或把 TTL 降到 2–5 分钟；高敏感房间启用服务端移除。 |
| Medium | `backend/src/infra/livekit.rs:121-153`；`release/livekit/livekit.yaml.example:27-32` | TURN 凭证不绑定 room/user，300s 可复用；relay 仅 51 端口，成员可耗尽。 | 扩大 relay range、按 IP/凭证配额、限制 peer/egress、监控 allocation。 |
| Medium | `backend/src/api/routes.rs:133-160`；`backend/src/infra/db.rs` | `/health`、`/config/public` 及房间/消息读无边缘限流，单 SQLite mutex 可被读洪泛阻塞。 | Nginx `limit_req`/`limit_conn`；缓存 health/config；读接口限流；`tower::timeout`/并发限制。 |
| Medium | `release/nginx/cloudroom.conf.example` | 无 `limit_req`/`limit_conn`；`proxy_read_timeout` 3600s 对非 WS 也生效。 | 拆分 `/ws` 与普通 API，分别配置超时/连接限制。 |
| Medium | `backend/src/api/auth_routes.rs` | 账号 bucket 不再阻止正确密码，但分布式错误猜测仍主要受每 IP + Argon2 成本约束；错误密码达到上限返回 429 可暴露账号存在。 | 统一错误响应 + 全局 Argon2 信号量；或按 (account, IP) 指数退避并返回 `Retry-After`。 |
| Low | `backend/src/infra/repo.rs` purge recursion | 递归清理的子房不写 tombstone；活跃子房可能随父房清理。 | 递归时插入 tombstone；父房删除时通知/单独 grace 子房。 |
| Low | `backend/src/api/auth_routes.rs:75-99` | bootstrap token 在创建用户前消费；用户名冲突会烧掉链接。 | 先验证用户名/哈希，再原子消费；失败恢复 token。 |
| Low | `frontend-server/src/main.rs:178-196` | 可选 TLS 入口无握手/总请求/上游超时、无连接上限。 | 只内网使用；补 timeout/ConcurrencyLimit/body limit。 |
| Low | `backend/src/api/admin.rs:313-326`；`backend/src/app/admin.rs:145-163` | 普通用户密码重置返回临时密码且无强制首登改密。 | 一次性链接/强制改密；缩小 admin 权限。 |
| Low | `release/nginx/cloudroom.conf.example` | 无 `/rtc` 块时 LiveKit JWT 查询串可能进入 error log（Compose 已 `error_log /dev/null crit`）。 | release 模板补 `/rtc` 时同步 error_log 配置。 |
| Low | `backend/src/util.rs`；`frontend/.../MessageList.tsx` | 昵称可自我设置且不唯一；除 bidi 外的不可见 Cf 未全部拒绝，仍可冒充房主。 | 拒绝全部 Cf（emoji ZWJ 例外）、昵称唯一、渲染角色/用户名。 |
| Low | `frontend/package-lock.json` | react-router 6.30.4 有 moderate advisory；当前无 SSR，路由目标受校验，未构造可达利用。 | 升级依赖；保持 Link/navigate 目标白名单。 |
| Low | Compose/Docker | nginx/alpine 未 pin digest；LiveKit 二进制无 checksum；备份默认未加密；audit/messages 无保留策略。 | 固定 digest/校验和；加密备份与保留/监控。 |

## 公网部署检查清单（补充）

- [ ] 使用受信证书、`insecure_cookies=false`、完整安全头；不要使用 Compose 测试/自签默认值。
- [ ] Nginx 覆盖 XFF、关闭/脱敏访问日志与 `/rtc` error log，添加 `limit_req`/`limit_conn`。
- [ ] systemd `UMask=0077`、数据/配置权限、journal 不含 token。
- [ ] LiveKit 使用随机 key/secret、WSS；高敏感场景补 `RemoveParticipant`/`DeleteRoom`。
- [ ] TURN relay range/配额与防火墙；监控 allocation。
- [ ] 备份加密 + 恢复演练；保留 audit/messages 配额。
- [ ] 依赖门禁 `cargo audit`/`npm audit`；LiveKit 二进制校验和；镜像 pin digest。
- [ ] 生产实测 TLS、Nginx、systemd、真实麦克风；不要用 fake device/自签结果代替。

## 验证命令

```bash
cd backend && cargo test --locked --all-targets
cd .. && scripts/check.sh
# 关键复现/回归（隔离实例）
cargo test --locked --test e2e grace_purge_handles_room_request_fk_and_continues_sweeping
cargo test --locked --test e2e disabling_an_account_closes_its_open_websocket
cargo test --locked --test e2e forged_xff_from_untrusted_peer_cannot_bypass_login_limit
cargo test --locked --test e2e invalid_anonymous_join_does_not_create_guest_rows
cargo test --locked --test e2e ws_rejects_oversized_messages_and_event_bursts
```

## 未覆盖/假设

- 未启动生产 Nginx/systemd/LiveKit，未做真实 TURN allocation、真实麦克风、证书链和压力测试。
- LiveKit 对已建立连接在 JWT 到期后的行为属于外部组件，标记疑似；需要在隔离 LiveKit 上验证。
- `cloudroom-compose` 不在 Git；已修改但未随分支提交，部署方需单独同步。
- 未审计 LiveKit 二进制/上游镜像的供应链签名；仅检查了 lockfile/配置。

## 最终判断

- **代码层面：无新的 Critical/High；主要 Medium 已修复或转为明确的部署项。**
- **公网对抗性场景：仍不建议直接暴露；先处理 LiveKit 撤销、TURN 配额、边缘限流和真实部署验证。**
