# 技术债务清单（2026-10-22）

- 收口分支：`chore/debt-closure`（待合并）
- 合并目标：`main`
- 状态口径：每项只有 `fixed` / `blocked` / `accepted` / `wontfix` 之一。
- 本轮验证：`scripts/check.sh` **18 PASS / 0 FAIL / 0 SKIP**；`scripts/ui-smoke.sh` 成功与失败退出码均验证；两个 Rust 锁文件 `cargo audit` exit 0 且无 warning。

## fixed

### 前端入口 chunk 体积与懒加载 — fixed

- 入口 `dist/assets/index-*.js` 约 13.7 kB raw / 5.1 kB gzip；`Room`、`Admin`、`Settings` 已懒加载，React/LiveKit/rnnoise 按需加载。
- 验证：`npm run build` 产物清单；入口体积仍低于 500 kB。

### app 层 Batch 1-8 — fixed

- join、message send、room request、settings/member lifecycle、mutes/invite revoke、direct room、avatar、admin writes 的多步用例已上移；`backend/src/api` 中 `crate::infra::repo` 直连从 189 降到 95。
- 验证：`cargo test --locked`；多步路径由 `backend/tests/e2e.rs` 覆盖。

### Room 消息列表与 Admin 邀请/用户 tab 展示层测试 — fixed

- `MessageList`/`MessageItem`、`InvitesView`/`UsersView` 已抽取；Admin tab 切换、邀请状态/409、用户权限/空态均有 RTL 覆盖。
- 前端共 13 个测试文件、55 个测试；`npm test` 通过。

### 离开/被踢成员访问拒绝 — fixed

- `app::access::active_member_room` 检查 `left_at`；离开/被踢后 HTTP 房间、消息、设置、成员、禁言、私聊、ICE/media-token 返回 403；WS 新连接拒绝，旧连接在下个事件关闭。
- join/rejoin：主动离开可免邀请恢复；被移除（`left_reason='removed'`）必须使用 kick 后创建的新邀请。
- 验证：`cargo test --test e2e left_member`、`kicked_rejoin_requires_new_invite`。

### 邀请撤销终态 — fixed

- 已使用/已过期邀请撤销返回 409，不修改记录；未使用重复撤销 200，不存在/类型不匹配 404。
- 验证：`cargo test --test e2e room_mutes_and_invite_revoke_paths`。

### 房主转让单一 owner — fixed

- 标准房间转让时先把目标以外 owner 降为 member，再设置目标 owner 和 `owner_user_id`；直接房保留双方 owner。
- 前端收到 `owner_transferred` 刷新房间角色/权限。
- 历史数据修正见 blocked 项；验证：`cargo test --test e2e admin_reject_force_delete_and_transfer`。

### 静态 musl 发布与无 glibc 运行 — fixed

- `scripts/build-musl.sh all` 产出 static-pie；Alpine 无 glibc 运行、旧库升级、静态 test-server 64 项及 UI 语音验证通过。
- 验证：`file release/cloudroom-server`、release 验证记录、`scripts/check.sh`。

### `scripts/check.sh` 的 ESLint 与 cargo-audit 门禁 — fixed

- ESLint：`eslint src --max-warnings=0`，js/ts 推荐规则 + react-hooks，`exhaustive-deps=error`；首次 9 problems（6 errors / 3 warnings）已清零。
- cargo-audit：`cargo-audit 0.22.2`，同时扫描 `backend/Cargo.lock` 与 `frontend-server/Cargo.lock`；H2/rustls/quinn 等修复版本已锁定。
- 验证：`scripts/check.sh` 18 PASS / 0 FAIL / 0 SKIP。

### `chacha20` yanked warning — fixed

- `chacha20 0.10.1` 升级到 0.10.2（backend 传递依赖，`rand 0.10.2` 选择）；两个锁文件 audit 均无 warning、exit 0。
- 验证：`cargo audit --file backend/Cargo.lock`。

### `rustls-pemfile` unmaintained warning — fixed

- `frontend-server` 不再依赖 `rustls-pemfile 2.2.0`；改用 `rustls-pki-types` 1.x 的 `PemObject` 读取证书/私钥（`frontend-server/src/main.rs`）。
- 验证：`cargo check` + `cargo test`；`scripts/ui-smoke.sh` 使用真实 TLS 证书启动前端服务器并通过语音冒烟。

### 本地 UI 语音冒烟入口 — fixed

- 新增 `scripts/ui-smoke.sh`：自动启动/清理测试环境、注册 bootstrap super_admin、创建 SFU 房间，再运行 `test-server/ui-voice-check.mjs`。
- 成功时 exit 0；用 `CHROME_PATH=/nonexistent/chrome` 验证失败时 exit 1。
- 不接入 `scripts/check.sh` 或 CI；保留为本地手动验收工具。

### README/测试脚本与发布文档 — fixed

- README 已拆分常驻测试环境/`sfu-verify`/`run-tests`/`check.sh` 的顺序，并注明 `release/livekit-server` 前置。
- `test-server/run-tests.sh` 未定义 `pass`/`fail` 的旧问题已确认消失；实跑 64 通过 / 0 失败。
- 发布 static-pie 构建与阅读文档已对齐。

## blocked

### 生产 TLS、证书与浏览器信任链 — blocked

- 前置条件：部署服务器、真实域名、CA 证书、DNS 与 Nginx/浏览器环境。
- 当前仅有自签证书与 `--ignore-certificate-errors` 的测试；生产 CA、WSS 同源路径未验证。

### Nginx/systemd 实际部署 — blocked

- 前置条件：目标 Linux 主机、已安装 Nginx/systemd，允许执行 `nginx -t`/启动服务。
- 当前环境无 `nginx`/`systemd-analyze`，模板只做了静态检查。

### `release/install.sh` 实际安装 — blocked

- 前置条件：可写 `/usr/local/bin`、`/etc/systemd/system` 的部署机或一次性容器。
- 当前为避免改写系统未执行安装；只做了 `bash -n`。

### 历史房主角色/`owner_user_id` 数据修正 — blocked

- 前置条件：生产数据库副本/备份、迁移窗口、按 `docs/plans/2026-10-21-owner-transfer.md` 的统计与回滚 SQL 审批。
- 当前无部署库；直接房不受影响。执行前必须统计 `owner_user_id IS NULL` 的标准房并备份。

### 真实麦克风与设备差异 — blocked

- 前置条件：真实浏览器 + 麦克风/扬声器，覆盖权限拒绝、回声消除、降噪、不同采样率。
- 当前仅使用 Chrome fake device；设备/权限行为无法在 CI 或本机无设备环境验证。

### UI 语音冒烟接入 CI — blocked

- 前置条件：CI 提供 Chrome、Docker/常驻测试服务与 LiveKit UDP 端口。
- 本地入口 `scripts/ui-smoke.sh` 已固定；CI 仍无该能力，不能把本地 PASS 当作 CI 证明。

## accepted

### 剩余 95 处 api→infra 直连与 WS 30 处 — accepted

- 理由：剩余多为单条读取/写入或 WS transport/auth；30 处状态机与广播顺序耦合，抽到 app 会改变时序边界。
- 影响：接口层仍与 SQLite 仓储耦合；多步用例已优先放在 app。
- 复检触发：新增多步业务、需要替换仓储实现，或 WS 事件顺序契约变化。

### Room 整体 socket/语音集成与媒体 hooks 未做组件测试 — accepted

- 理由：`useMicTest`/`useP2PVoice`/`useLiveKitVoice` 依赖 WebRTC/媒体设备/第三方内部，mock 成本高；展示层与选择逻辑已覆盖。
- 影响：媒体适配器回归依赖 `ui-voice-check` 和人工验证，不能由 jsdom 测试发现。
- 复检触发：适配器逻辑改动、真实设备问题复现，或测试框架能稳定模拟媒体时。

### 迁移前 `left_reason=NULL` 兼容 — accepted

- 理由：历史行无法可靠区分主动离开与被踢；当前统一按主动离开恢复，保持升级兼容。
- 影响：迁移前的被踢历史用户可能凭空邀请重新加入；迁移后的 removed 记录已强制新邀请。
- 复检触发：能从历史审计/备份回填 `left_reason`，或访问策略需要收紧。

### 直接房并发/claim_code 语义 — accepted

- 理由：整个创建编排在 `Db::with` 的单个 SAVEPOINT 内，全局连接锁串行化；重复请求命中 `direct_pairs` 返回同一房间（200）。
- 影响：这是现有 API 语义，不是缺陷但调用方需按幂等成功处理。
- 复检触发：事务模型/连接池改变，或产品要求并发请求返回 409。

### LiveKit 懒加载 chunk > 500 kB — accepted

- 理由：`livekit-*` 约 530 kB raw / 138 kB gzip，仅进入语音时加载；主入口仍低于 500 kB。
- 影响：首次加入语音有下载成本；不调整 `chunkSizeWarningLimit`。
- 复检触发：语音首屏性能要求变化或可进一步拆分 LiveKit。

### 9 条精确 clippy allow — accepted

- 理由：保留现有公开签名/错误契约（`FromStr`、`impl Future`、仓储参数结构、反应聚合类型）；全部是函数/方法级 allow。
- 影响：没有模块级 suppress；未来重构签名时重新评估。
- 复检触发：对应公开 API/仓储签名重构。

## wontfix

### 独立 coturn — wontfix

- 理由：LiveKit 内嵌 TURN 已覆盖 P2P/SFU；独立 coturn 会新增部署面、端口和密钥。
- 影响：部署文档不提供 coturn 路径；TURN 依赖 LiveKit 可用。
- 复检触发：LiveKit TURN 无法满足生产 NAT/规模要求。

### UI 快照测试 — wontfix

- 理由：当前使用行为级 RTL 与 UI 语音冒烟；快照会锁定实现细节并产生噪声。
- 影响：纯视觉回归不自动拦截，依赖冒烟/人工检查。
- 复检触发：出现稳定的视觉规范或浏览器截图基线需求。
