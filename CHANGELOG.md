# Changelog

- 2026-10-20: 修复 `GET /api/v1/me/rooms` 列映射错位：房间 SELECT 与 `row_to_room` 统一使用共享列清单，补回 `announcement`/`max_members`，接口恢复 200；仅查询/映射与测试变更。
- 2026-10-20: 分层重构合并（后端 `api/app/domain/infra`、前端 MVVM）；修复 SFU 房间“加入语音”误用首轮 P2P 会话的问题；无对外 API/配置变更；已知问题 `/api/v1/me/rooms` 500（既有，见 `docs/debt.md`）。
