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
- 新增 blocked needs verification：
  - D-019 叶子+中间证书 PEM 链 TLS 冒烟；
  - D-020 `ui-smoke.sh` 失败路径环境/进程清理。

## 登记表统计

- 总数：28
- `fixed`：12
- `blocked`：8（D-013 ~ D-020）
- `accepted`：6
- `wontfix`：2

## 现在就能解除的项

- D-019：本地可用 `openssl` 生成 leaf/intermediate/root + key，先跑 `openssl verify ...`，再用 `release/cloudroom-frontend-server` + `curl --cacert` 验证；不需要外部服务。
- D-020：本地可执行 `CHROME_PATH=/nonexistent/chrome scripts/ui-smoke.sh`，然后检查进程/端口/`/tmp/cloudroom-test-env` 是否清理；不需要外部服务。
- 其余 blocked（D-013 ~ D-018）分别需要生产域名/证书、Nginx/systemd 主机、部署机、数据库副本、真实设备、CI 媒体环境，无法仅靠本仓库解除。

## 验证

- `scripts/check.sh`：18 PASS / 0 FAIL / 0 SKIP（本轮文档改动前已通过，登记表不改变代码）。
- `scripts/ui-smoke.sh`：成功 PASS；失败路径 exit 1（脚本行为已存在，清理检查登记为 D-020）。
- `cargo audit` 两个锁文件：exit 0、无 warning。

## 剩余风险

- blocked 项仍依赖部署方环境；登记表只保证“解除条件/第一条命令/通过标准”明确，不代表已执行。
- D-019 的证书链验证与 D-020 的失败清理验证仍未完成，按登记表保持 blocked。
