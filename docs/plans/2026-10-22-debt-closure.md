# Debt closure（2026-10-22）

- 分支：`chore/debt-closure`
- 基线：`main` @ `fc2a448`（`baseline-20261022-1`）
- 状态口径：`fixed` / `blocked` / `accepted` / `wontfix`，每项仅一个。

## 结论

### 已修

- `chacha20`：从 0.10.1 升到 0.10.2；两个锁文件 audit 无 warning。
- `rustls-pemfile`：frontend-server 改用 `rustls-pki-types` 的 `PemObject` 读取 PEM；移除 unmaintained 直接依赖。
- 新增 `scripts/ui-smoke.sh`：自动启动/清理测试环境、创建 SFU 用户/房间并运行 `ui-voice-check.mjs`；成功 0、失败 1；不接入 check/CI。
- 既有 ESLint、cargo-audit、访问控制、邀请撤销、房主转让和静态发布项保持 fixed，并在 `docs/debt.md` 保留证据。

### blocked

- 生产 TLS/CA、Nginx/systemd、实际安装：需要服务器/域名/证书/可写部署环境。
- 历史 owner 数据修正：需要部署库副本与迁移窗口。
- 真实麦克风：需要真实设备。
- UI 冒烟进 CI：需要具备 Chrome 和 LiveKit 的 CI 运行环境。

### accepted / wontfix

- 剩余 api→infra 直连、Room/媒体 hooks 测试缺口、历史 `left_reason=NULL`、直接房并发语义、LiveKit chunk、clippy allow：`accepted`，理由/影响/复检触发写入 `docs/debt.md`。
- 独立 coturn、UI 快照：`wontfix`，理由与复验条件写入 `docs/debt.md`。

## 验证

```text
$ scripts/check.sh
结果：18 PASS / 0 FAIL / 0 SKIP

$ scripts/ui-smoke.sh
UI voice smoke: PASS

$ CHROME_PATH=/nonexistent/chrome scripts/ui-smoke.sh
FAIL: 找不到 Chrome: /nonexistent/chrome
exit=1

$ cargo audit --file backend/Cargo.lock
exit 0（无 warning）

$ cargo audit --file frontend-server/Cargo.lock
exit 0（无 warning）

$ scripts/build-musl.sh all
PASS（static-pie）
```

运行时成功冒烟使用新 TLS PEM 加载路径：media-token 200、`/rtc/` 101、PC connected、outbound audio RTP 7 packets、leave 清理、console 0 errors；测试环境已停止。

## 遗留

- 静态发布构建已通过，但生产 TLS/Nginx/systemd/真实设备仍需部署方环境；具体前置条件见 `docs/debt.md` 的 blocked 项。
