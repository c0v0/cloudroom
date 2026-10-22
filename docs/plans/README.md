# plans/ — 计划、决策与验收记录

命名：`<YYYY-MM-DD>-<任务名>.md`。本目录是过程记录，不删除历史；已关闭的计划在批次归档时移到 `docs/archive/plans/`。

## 分类

- **纯计划**：只记录方案、边界和验证方式，执行结果可能在其他文件或测试中；使用前先确认状态。
- **计划+验收**：同一文件包含计划、验收标准、提交和命令输出，可直接作为完成证据。
- **历史计划**：已被后续实现或决定取代，只保留追溯价值，不再作为当前入口。

## 当前文件

| 文件 | 类型 | 一句话用途 |
|---|---|---|
| `2026-10-21-admin-tabs.md` | 计划+验收 | 将 Admin 的邀请/用户 tab 抽为 `InvitesTab`/`UsersTab`，含测试和提交 `8038881`。 |
| `2026-10-21-kicked-rejoin.md` | 纯计划 | 被踢用户重新加入必须使用新邀请的决策与实现说明。 |
| `2026-10-21-left-at-access.md` | 纯计划 | 已离开成员 HTTP/WS 访问拒绝的决策与实现说明。 |
| `2026-10-21-owner-transfer.md` | 纯计划 | 房主转让后降级其他 owner，以及 direct room 例外的实现说明。 |
| `2026-10-21-ws-events-decision.md` | 纯计划 | 保留 WS 30 处 repo 调用在 API 层的决策与测试策略。 |
| `2026-10-22-close-gate-gaps.md` | 计划+验收 | 补齐 ESLint 与 cargo-audit 门禁的验收记录。 |
| `2026-10-22-debt-closure.md` | 计划+验收 | 依赖告警、测试/部署债务和 UI 冒烟收口记录。 |
| `2026-10-22-deferred-register.md` | 计划+验收 | 28 项 debt 登记表及 D-019/D-020 本地验证结果。 |
| `2026-10-22-room-message-list.md` | 计划+验收 | Room 消息列表抽取为 `MessageList`，含测试和提交 `41847be`。 |
| `2026-10-22-file-organization.md` | 计划+验收 | 174 文件盘点、批次 0-3 归档执行记录与最终门禁结果。 |
| `2026-10-22-aec-echo-cancellation.md` | 计划+验收 | AEC 实际状态校验、rnnoise 开关一致、提示与插件门槛评估。 |
| `README.md` | 索引 | 本目录的分类、命名和文件用途说明。 |

## 已归档

以下文件保留原文，用于追溯，不再作为当前状态入口：

- [`2026-10-19-代码分层整理.md`](../archive/plans/2026-10-19-代码分层整理.md) — 后端分层只读盘点（含 2 项未验证）。
- [`2026-10-20-app-layer-migration.md`](../archive/plans/2026-10-20-app-layer-migration.md) — API 到 `app` 的迁移方案（无独立验收表）。
- [`2026-10-21-frontend-component-tests.md`](../archive/plans/2026-10-21-frontend-component-tests.md) — 首批前端组件/hook 测试方案（无独立验收表）。

历史媒体 spike 见 [`docs/archive/spikes/README.md`](../archive/spikes/README.md)。
