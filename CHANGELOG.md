# Changelog

- 2026-10-20: 固化 UI 语音冒烟脚本 `test-server/ui-voice-check.mjs`：真实 Chrome/CDP 验证登录、进入 SFU 房间、media-token、`/rtc/` 101、connected、publish、leave 与 console 错误；不新增依赖，未接入 `scripts/check.sh`。
- 2026-10-20: 清理 mechanical clippy 警告（29 条），为 9 条公开签名/API 相关 lint 添加精确 allow；`scripts/check.sh` 固定 clippy `-D warnings`，完整门禁 15 PASS / 0 FAIL / 2 SKIP。
- 2026-10-20: 修复 `GET /api/v1/me/rooms` 列映射错位：房间 SELECT 与 `row_to_room` 统一使用共享列清单，补回 `announcement`/`max_members`，接口恢复 200；仅查询/映射与测试变更。
- 2026-10-20: 分层重构合并（后端 `api/app/domain/infra`、前端 MVVM）；修复 SFU 房间“加入语音”误用首轮 P2P 会话的问题；无对外 API/配置变更；已知问题 `/api/v1/me/rooms` 500（既有，见 `docs/debt.md`）。
