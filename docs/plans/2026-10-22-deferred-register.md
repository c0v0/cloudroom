# Deferred register（2026-10-22）

- 分支：`docs/deferred-register`
- 基线：`chore/debt-closure` @ `50b07f9`（含债务状态分类）
- 目标：把 `docs/debt.md` 改成可执行登记表，并补齐部署/发版检查清单。

## 交付

- `docs/debt.md`：28 条登记项，列固定为 ID | 条目 | 状态 | 阻塞条件 | 解除后动作 | 验证方式 | 复检触发。
- `docs/deploy-checklist.md`：证书链/TLS → Nginx → systemd → install.sh → 历史 owner SQL → 真实麦克风，每步命令与期望输出。
- `docs/release-checklist.md`：audit、tests、build 哈希、CHANGELOG、tag、已知限制。
- `AGENTS.md` §1.12：周期/发版体检检查 blocked 项解除条件。
- `README.md` 目录索引补充两个 checklist。
- 新增并完成验证：
  - D-019 叶子+中间证书 PEM 链 TLS 冒烟已 fixed；
  - D-020 `ui-smoke.sh` 失败路径环境/进程清理已 fixed。

## 登记表统计

- 总数：28
- `fixed`：14
- `blocked`：6（D-013 ~ D-018）
- `accepted`：6
- `wontfix`：2

## 当前 blocked 项

- D-013 ~ D-018 分别需要生产域名/证书、Nginx/systemd 主机、部署机、数据库副本、真实设备、CI 媒体环境，无法仅靠本仓库解除。
- D-019/D-020 已在本地验证并改为 fixed（见下）。

## 验证

- D-019：`openssl verify` 输出 `leaf.pem: OK`；`curl --cacert root.pem https://localhost:18443/api/v1/health` 返回 200；`s_client` 显示 leaf→intermediate→root 且 `Verify return code: 0 (ok)`。
- D-020：失败注入 exit 1；无残留进程、8080/8081/7880 未监听、`/tmp/cloudroom-test-env` 不存在。
- `scripts/check.sh`：18 PASS / 0 FAIL / 0 SKIP（文档改动前已通过，登记表不改变代码）。
- `scripts/ui-smoke.sh`：成功 PASS。
- `cargo audit` 两个锁文件：exit 0、无 warning。

## 剩余风险

- 其余 blocked 项仍依赖部署方环境；登记表只保证“解除条件/第一条命令/通过标准”明确，不代表已执行。
- 生产 TLS、Nginx/systemd、真实麦克风和 CI 媒体环境仍未验证。
