# 安全修复第一波（2026-10-23）

- 分支：`fix/security-wave1`，从 `main`（`b2815ce`）创建。
- 范围：安全审查报告中的部署默认值、mute 时长 panic、X-Forwarded-For 信任、WebSocket 资源边界。
- 未推送、未发布、未合并 main。

## 提交

| 提交 | 内容 |
|---|---|
| `990c977` | `docs: add public deployment security review` |
| `4d07f48` | `fix(config): reject insecure defaults in production` |
| `9eeefcb` | `fix(rooms): validate duration before chrono arithmetic` |
| `1790da1` | `fix(security): trust forwarded headers only from configured proxies` |
| `e0b16ff` | `fix(ws): bound message size, queues and event rate` |

---

## 1. 部署默认值 / 生产启动校验

### 复现

默认配置此前可通过校验（空/占位 LiveKit 凭据、http origin、`insecure_cookies=true` 均可启动；Compose 包带固定测试 key/secret、自签证书、`insecure_cookies=true`）。

### 改动

- `backend/src/config.rs`
  - 新增 `server.development`，默认 `false`（生产模式）。
  - 生产模式拒绝：
    - `insecure_cookies=true`
    - `public_origin` / `frontend_origin` 非 `https://`
    - `media.livekit_url` 非 `wss://`
    - 空、`CHANGE_ME`、`devkey`、`devsecret*` LiveKit 凭据
  - `server.development=true` 是本地/测试的显式 opt-in。
  - 新增 8 个单元测试覆盖默认拒绝、dev 成功、insecure/HTTP/WS/占位/空凭据。
- `release/config.toml.example`、`docs/configuration.md` 增加 `development` 与限制说明。
- 测试脚本/测试状态显式标记 `development = true`（`scripts/smoke.sh`、`test-server/run-tests.sh`、`test-server/start-test-env.sh`、`backend/tests/e2e.rs`）。
- Compose 包（`../cloudroom-compose`，非 Git）：
  - `.env` / `.env.example` 改为 `CHANGE_ME` 占位，不再包含固定 LiveKit secret；`.env` 0600。
  - `cloudroom/config.template.toml`：`insecure_cookies=false`、`development=false`。
  - `cloudroom/entrypoint.sh` 与 `livekit/entrypoint.sh` 拒绝 `CHANGE_ME`/`devsecret*`。
  - `scripts/init.sh` 要求显式 `--self-signed` 才生成自签证书；默认要求受信 `fullchain.pem`/`privkey.pem`。
  - 删除包内自签证书；补 Nginx HSTS/CSP/XFO/nosniff/Referrer-Policy 与 dotfile deny。
  - `docker compose config -q` 退出 0；shell 语法检查通过。

### 测试输出

```text
cargo test --locked config::tests
cargo test: 8 passed

cargo test --locked --all-targets
cargo test: 41 passed

scripts/check.sh
18 PASS / 0 FAIL / 0 SKIP
```

首次 `scripts/check.sh` 命中测试字符串被当成密钥（已改为短测试值），随后一次链接 I/O error，重试构建成功；最终门禁通过。

### 剩余风险

- `development=true` 仍允许不安全配置；部署方必须确保生产配置不设置该键（默认 false）。
- 生产校验只拒绝空/已知占位凭据，不评估随机性和长度。
- Compose 包未纳入 Git，未随分支提交；其修改只存在于工作区部署包。

---

## 2. mute duration panic

### 复现

```bash
# 仅隔离实例；修复前会 abort
curl -si -X POST http://127.0.0.1:PORT/api/v1/rooms/ROOM/mutes \
  -b 'cloudroom_session=...' -H 'X-CSRF-Token: ...' \
  -H 'Content-Type: application/json' \
  -d '{"scope":"room","duration_seconds":9223372036854775807}'
```

修复前 `chrono::Duration::seconds(i64::MAX)` 或 `DateTime + TimeDelta` 溢出 panic；release `panic="abort"` 会终止整个进程。

### 改动

- `backend/src/api/rooms.rs` 新增 `mute_expiry(now, Option<i64>)`：
  - `None`：永久禁言（保持原语义）。
  - `Some(1..=365d)`：`try_seconds` + `checked_add_signed`。
  - 负数、0、超过一年：`400 invalid_mute_duration`。
- 新增 5 个单元测试：None、0、负数、极值、一年上限。

### 测试输出

```text
cargo test --locked api::rooms::tests
cargo test: 5 passed

cargo test --locked --all-targets
cargo test: 46 passed

scripts/check.sh
18 PASS / 0 FAIL / 0 SKIP
```

### 剩余风险 / chrono 扫描清单

本波只修 mute 算术；`backend/src/api/rooms.rs` 其他相关点：

- `:353` `state.auth.session_cookie(...).parse().unwrap()`：Cookie 头由服务端生成，不由用户拼接；未改。
- `:438` `DateTime::parse_from_rfc3339`：已有错误映射，未改。
- `:549` `now + chrono::Duration::from_std(ttl).unwrap_or_default()`：`from_std` 溢出时回退零时长，不会 panic；语义可能变成立即过期，待后续审查。
- 测试代码中的 `unwrap`/`Duration::seconds(MAX_MUTE_SECONDS)` 为固定安全值。

仍建议后续把 release `panic="abort"` 改为 handler 级隔离策略；本波未改构建 profile。

---

## 3. X-Forwarded-For 信任

### 复现

修复前 `client_ip` 只判断 `trusted_proxy.is_some()`，取 XFF 第一个值；新增 e2e 在 `trusted_proxy=10.0.0.1`、实际 peer=127.0.0.1 时发送轮换 XFF：

```text
forged XFF bypassed the login IP limiter: [401, 401, ... 12 次]
```

### 改动

- `backend/src/api/security.rs`
  - 新增 `PeerAddr` 提取器，从 `ConnectInfo<SocketAddr>` 取实际 TCP peer。
  - `client_ip` 只在 `trusted_proxy` 显式配置且与实际 peer 匹配时使用 XFF（Unix socket 无 peer 时允许显式代理）。
  - 只接受 XFF 最右跳并解析为 IP；非法值回退到 peer。
  - `ConnectionInfo`：TCP 服务通过 `into_make_service_with_connect_info::<SocketAddr>()` 注入 peer。
- `auth_routes.rs`（register/login/upgrade-guest）与 `rooms.rs`（join）接入 `PeerAddr`。
- `frontend-server` 不再转发客户端 XFF，改为用实际 TCP peer 重写。
- Nginx release/Compose 模板改为 `X-Forwarded-For $remote_addr`（覆盖而非追加），并注释只信任该代理。
- 新增 4 个 `security` 单元测试与 1 个 e2e：伪造 XFF 最终返回 429。

### 测试输出

```text
cargo test --locked --test e2e forged_xff_from_untrusted_peer_cannot_bypass_login_limit
test result: ok. 1 passed

cargo test --locked api::security::tests
test result: ok. 4 passed

cargo test --locked --all-targets
cargo test: 51 passed

scripts/check.sh
18 PASS / 0 FAIL / 0 SKIP
```

### 剩余风险

- Unix socket 没有 `SocketAddr`：当显式配置 `trusted_proxy` 时仍信任 XFF；依赖 socket 文件权限限制为本地代理。
- 只支持精确 IP，不支持 CIDR；IPv4-mapped IPv6 未做归一化。
- 代理链多跳时只取最右值；当前部署假设单个受信 Nginx。
- Nginx 模板未加 `limit_req`/`limit_conn`；应用限流之外仍建议边缘限流。

---

## 4. WebSocket 资源限制

### 复现/测试

新增：

- Hub 单元测试：每用户/全局连接上限与释放、队列满通知、替换 sender 后旧连接不再 current。
- WS 单元测试：事件预算上限、0 预算立即拒绝。
- e2e：`ws_max_message_bytes=1024` 发送 4 KB 消息 → 关闭；`ws_events_per_second=2` 发送 4 个 ping → 关闭。

### 改动

- `backend/src/config.rs` `[security]` 新增：
  - `ws_max_message_bytes=65536`
  - `ws_queue_messages=128`
  - `ws_events_per_second=30`
  - `ws_max_connections_per_user=4`
  - `ws_max_connections_total=1024`
- `backend/src/api/ws.rs`
  - `max_message_size`/`max_frame_size` 设置入站上限。
  - 每连接有界 `mpsc::channel`；`ServerMessage::Close`/`Notify` 在队列满、被替换时发送 close frame。
  - 每连接一秒 token bucket；超限记录并关闭。
  - 旧连接用 `is_current` 检查，被替换后不再处理事件。
- `backend/src/infra/hub.rs`
  - `Hub::with_limits(total, per_user)`；注册/关闭计数。
  - 慢消费者队列满时通知 writer 关闭，不无限增长。
  - `unregister_all` 通知旧 writer 关闭。
- `backend/src/main.rs` 按配置创建 Hub；e2e 同步。
- `docs/configuration.md`、`release/config.toml.example` 增加配置项。

### 测试输出

```text
cargo test --locked --lib tests
cargo test: 34 passed（含 Hub/WS 单元）

cargo test --locked --test e2e ws_rejects_oversized_messages_and_event_bursts
test result: ok. 1 passed

cargo test --locked --all-targets
cargo test: 57 passed

scripts/check.sh
18 PASS / 0 FAIL / 0 SKIP
```

### 剩余风险

- 被替换/队列满的旧连接仍可能处于 TCP 读等待，连接计数保留到它真正关闭；上限能防止无限增长，但可能短时间拒绝重连。
- 只限制应用层连接数，未限制每房间连接数；Nginx 边缘仍无 `limit_conn`。
- 队列满时 writer 可能正阻塞在 `send`，close 需要等该次写完成；队列本身有界，不会继续增长。
- `max_body_bytes` 仍未接入 `DefaultBodyLimit`；头像解码前尺寸检查、`upgrade-guest` Argon2 无限流、LiveKit token 撤销、已建立 WS 的会话撤销等未在本波修复。

---

## 验收结论

- 四项修复均单独提交；每项完成后执行 `cargo test --locked --all-targets` 与 `scripts/check.sh`，最终均通过。
- 代码分支干净，未推送/未合并 main。
- Compose 部署包的安全默认值已改，但它不是 Git 仓库内容，无法出现在本分支提交中；部署时需单独同步或重新打包。
