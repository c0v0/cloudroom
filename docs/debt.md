# 技术债务 / 留待项登记表（2026-10-22）

- 状态只用：`fixed` / `blocked` / `accepted` / `wontfix`。
- 收口分支：`docs/deferred-register`（已合并到 `main`）。
- 当前门禁：`scripts/check.sh` **18 PASS / 0 FAIL / 0 SKIP**；两个 Rust 锁文件 `cargo audit` exit 0、无 warning。

| ID | 条目 | 状态 | 阻塞条件 | 解除后动作（命令/提示词） | 验证方式 | 复检触发 |
|---|---|---|---|---|---|---|
| D-001 | 前端入口 chunk 体积与懒加载 | fixed | — | 无需动作 | `npm run build`；入口约 13.7 kB raw / 5.1 kB gzip，低于 500 kB | 构建配置/路由改动 |
| D-002 | app 层 Batch 1-8 | fixed | — | 无需动作 | `cargo test --locked`；`grep -rF 'crate::infra::repo' backend/src/api`（95 行） | 新增多步业务或替换仓储 |
| D-003 | Room 消息列表与 Admin tabs 展示层测试 | fixed | — | 无需动作 | `npm test`：13 文件 / 55 测试 | 展示组件改动 |
| D-004 | 离开/被踢成员访问拒绝 | fixed | — | 无需动作 | `cargo test --test e2e left_member` | access 逻辑改动 |
| D-005 | 邀请撤销终态（已用/过期 409） | fixed | — | 无需动作 | `cargo test --test e2e room_mutes_and_invite_revoke_paths` | invite 状态机改动 |
| D-006 | 房主转让单一 owner | fixed | — | 无需动作 | `cargo test --test e2e admin_reject_force_delete_and_transfer` | 转让逻辑改动 |
| D-007 | 静态 musl 发布与无 glibc 运行 | fixed | — | 无需动作 | `scripts/build-musl.sh all`；`file release/cloudroom-server` 为 static-pie；Alpine 验证记录 | 工具链/依赖升级 |
| D-008 | ESLint 与 cargo-audit 门禁 | fixed | — | 无需动作 | `scripts/check.sh` 18/0/0；`npm run lint -- --max-warnings=0` | check 配置改动 |
| D-009 | `chacha20` yanked warning | fixed | — | 无需动作 | `cargo audit --file backend/Cargo.lock` exit 0、无 warning | 依赖更新 |
| D-010 | `rustls-pemfile` unmaintained warning | fixed | — | 无需动作 | `cargo audit --file frontend-server/Cargo.lock` exit 0；`scripts/ui-smoke.sh` TLS 冒烟通过 | 依赖更新 |
| D-011 | 本地 UI 语音冒烟入口 | fixed | — | 无需动作 | `scripts/ui-smoke.sh` PASS；`CHROME_PATH=/nonexistent/chrome scripts/ui-smoke.sh` exit 1 | 测试环境/脚本改动 |
| D-012 | README/测试脚本/发布文档 | fixed | — | 无需动作 | `test-server/run-tests.sh` 64/0；README 测试顺序与前置条件 | 文档结构改动 |
| D-013 | 生产 TLS、CA 与浏览器信任链 | blocked | 需要部署服务器、真实域名、CA 证书、DNS、Nginx 与生产浏览器 | `curl -fsS https://<domain>/api/v1/health` | 返回 `{"status":"ok",...}`；浏览器访问无证书告警；WSS 同源可用 | 拿到生产域名/证书后 |
| D-014 | Nginx/systemd 实际部署 | blocked | 需要目标 Linux 主机、已安装 Nginx/systemd、允许启动服务 | `sudo nginx -t` | `syntax is ok` / `test is successful`；`systemctl is-active cloudroom-server` = active | 有部署机或一次性环境 |
| D-015 | `release/install.sh` 实际安装 | blocked | 需要可写 `/usr/local/bin`、`/etc/systemd/system` 的部署机或容器 | `sudo ./install.sh` | `systemctl is-active cloudroom-server` = active；`/api/v1/health` 返回 ok | 有隔离部署环境 |
| D-016 | 历史 owner 角色/`owner_user_id` 数据修正 | blocked | 需要生产库副本/备份、迁移窗口、按 `docs/plans/2026-10-21-owner-transfer.md` 审批 | `sqlite3 <db> "SELECT COUNT(*) FROM room_members m JOIN rooms r ON r.room_id=m.room_id WHERE m.room_role='owner' AND r.kind='standard' AND r.owner_user_id IS NOT NULL AND m.user_id<>r.owner_user_id;"` | mismatch=0；`owner_user_id IS NULL` 的标准房单独列出处理；执行前已建备份表 | 拿到部署库副本后 |
| D-017 | 真实麦克风与设备差异 | blocked | 需要真实浏览器 + 麦克风/扬声器，覆盖权限拒绝、回声消除、降噪、不同采样率 | 手动：Chrome 打开房间并加入语音；`CHROME_PATH=<chrome> node test-server/ui-voice-check.mjs --headless=false ...` 仅作辅助（脚本使用 fake device） | 权限、采集、回声消除、降噪、采样率均符合预期；无静音/断流 | 有真实设备环境 |
| D-018 | UI 语音冒烟接入 CI | blocked | 需要 CI 提供 Chrome、常驻测试服务与 LiveKit UDP 端口 | CI job 运行 `scripts/ui-smoke.sh` | exit 0；console 0 errors；无残留进程 | CI 具备浏览器与媒体环境 |
| D-019 | 叶子+中间证书 PEM 链 TLS 冒烟 | fixed | — | 无需动作 | 已生成 root/intermediate/leaf 链；`openssl verify -CAfile root.pem -untrusted intermediate.pem leaf.pem` 输出 `leaf.pem: OK`；`frontend-server --tls-cert fullchain.pem` 启动后 `curl --cacert root.pem https://localhost:18443/api/v1/health` 返回 200，s_client 链为 leaf → intermediate → root 且 `Verify return code: 0 (ok)` | 证书链/加载代码改动 |
| D-020 | `ui-smoke.sh` 失败路径环境与进程清理 | fixed | — | 无需动作 | 已用 `CHROME_PATH=/nonexistent/chrome scripts/ui-smoke.sh` 验证 exit 1；`cloudroom-server`/`cloudroom-frontend-server`/`livekit-server` 无残留进程，8080/8081/7880 未监听，`/tmp/cloudroom-test-env` 不存在 | 脚本/start-test-env 清理逻辑改动 |
| D-021 | 剩余 95 处 api→infra 直连与 WS 30 处 | accepted | —（风险接受） | 无；按复检触发复核 | `cargo test --locked`；WS 时序测试 `ws_mute_speak_and_reaction_sequences` | 新增多步业务、替换仓储或 WS 顺序契约变化 |
| D-022 | Room 整体 socket/语音集成与媒体 hooks 未做组件测试 | accepted | —（风险接受） | 无；按复检触发复核 | `scripts/ui-smoke.sh` + 人工媒体检查 | 适配器改动、真实设备问题复现或可稳定模拟媒体 |
| D-023 | 迁移前 `left_reason=NULL` 兼容 | accepted | —（风险接受） | 无；按复检触发复核 | `cargo test --test e2e kicked_rejoin_requires_new_invite`、`left_member` | 能从审计/备份回填 `left_reason` 或访问策略收紧 |
| D-024 | 直接房并发/`claim_code` 语义 | accepted | —（风险接受） | 无；按复检触发复核 | `cargo test --test e2e direct_room_paths` | 事务模型/连接池改变或产品要求并发返回 409 |
| D-025 | LiveKit 懒加载 chunk > 500 kB | accepted | —（风险接受） | 无；按复检触发复核 | `npm run build` 产物约 530 kB raw / 138 kB gzip | 语音首屏性能要求变化或可进一步拆分 |
| D-026 | 9 条精确 clippy allow | accepted | —（风险接受） | 无；按复检触发复核 | `scripts/check.sh` clippy `-D warnings` | 对应公开 API/仓储签名重构 |
| D-027 | 独立 coturn | wontfix | —（不做） | 无 | 部署文档不提供 coturn 路径 | LiveKit TURN 无法满足生产 NAT/规模要求 |
| D-028 | UI 快照测试 | wontfix | —（不做） | 无 | 行为级 RTL + UI 冒烟 | 出现稳定视觉规范或截图基线需求 |
| D-029 | LiveKit 撤销无持久队列 | accepted | —（best-effort；无跨重启持久队列） | 无；进程崩溃/LiveKit 持续不可用时等待 media-token TTL 或重新触发业务操作 | `cargo test --test e2e livekit_`；真实 LiveKit 踢人/删房通过；故障路径 fake 500 计量失败 | 需要“保证撤销”SLA 或出现持续 LiveKit 故障 |
| D-030 | 消息列表虚拟化（B3） | accepted | —（初始 50 条 + 每次加载更早 50 条；`MessageItem` 已 memo，仅新增行触发重渲染） | 无；触发条件：单房常驻 > 500 条或滚动 p95 帧 > 16ms | DevTools Performance 录制滚动；`npm test` | 消息分页策略/单房历史上限调整 |
| D-031 | 全量 i18n（B14） | wontfix | —（产品定位中文自托管，全量抽取引入长期维护成本） | 无；触发条件：出现多语言部署需求 | `npm run build`；现有中文文案回归 | 产品需要英文/多语言时重开 |
