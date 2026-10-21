# Admin invites/users tab extraction（2026-10-21）

- 分支：`refactor/admin-tabs`
- 基线：`baseline-20261021-13`（`main` @ `1c2d853`）
- 关联计划：`local://admin-tabs-plan.md`

## 验收标准与结果

| # | 验收标准 | 命令 | 结果 |
|---|---|---|---|
| 1 | 默认 tab 为总览，tab 可切换到邀请/用户 | `npx vitest run src/pages/Admin.test.tsx` | PASS（2 tests） |
| 2 | active 邀请显示撤销；已使用/已过期不显示；空表无占位 | `npx vitest run src/components/InvitesTab.test.tsx` | PASS（3 tests） |
| 3 | 409 错误文案显示在 `.error` | 同上（mock 409） | PASS |
| 4 | super_admin 无禁用/角色选择；非 super 无角色选择；空/错误/临时密码 | `npx vitest run src/components/UsersTab.test.tsx` | PASS（3 tests） |
| 5 | 前端完整测试 | `npm test` | PASS（13 files / 55 tests） |
| 6 | 类型检查与构建 | `npm run typecheck && npm run build` | PASS |
| 7 | 全项目门禁 | `scripts/check.sh` | PASS（16 / 0 / 2 SKIP） |

SKIP 为既有 ESLint 未配置、cargo-audit 未安装。

## 变更

- 新增 `frontend/src/components/UsersTab.tsx`
  - `UsersTab` 保留 users fetch、error、临时密码、禁用/启用、重置密码、角色更新和 `window.confirm`。
  - `UsersView` 只接收 `users`、`isSuper`、`error`、`temporaryPassword` 和三个动作回调；保留原表格 DOM/class、按钮和角色下拉可见性。
- 新增 `frontend/src/components/InvitesTab.tsx`
  - `InvitesTab` 保留 invites fetch、生成邀请、撤销、剪贴板、`created`/`copied`/`error` 状态。
  - `InvitesView` 只接收数据、错误、创建链接/复制状态和回调；保留原表格、状态优先级、撤销按钮条件。
- `frontend/src/pages/Admin.tsx`
  - 引入两个组件，删除内联 `UsersTab`/`InvitesTab`。
  - tab 状态、`TABS`、默认 `overview`、`isSuper` 和其他 tabs 未改。
  - 移除不再使用的 `Invite`/`User`/`roleLabel` 导入。
- 新增 3 个测试文件：
  - `frontend/src/pages/Admin.test.tsx`
  - `frontend/src/components/InvitesTab.test.tsx`
  - `frontend/src/components/UsersTab.test.tsx`

无 CSS、API、后端、auth、路由或权限判定变更；没有新增分页/排序。

## 提交列表

- `8038881` refactor(admin): extract invites and users tab views
  - 6 files changed, 537 insertions(+), 203 deletions(-)

## 关键命令输出

```text
$ npx vitest run src/pages/Admin.test.tsx src/components/InvitesTab.test.tsx src/components/UsersTab.test.tsx
Test Files  3 passed (3)
Tests       8 passed (8)

$ npm test
Test Files  13 passed (13)
Tests       55 passed (55)

$ npm run typecheck
PASS（无输出错误）

$ npm run build
✓ built in 17.83s

$ scripts/check.sh
结果：16 PASS / 0 FAIL / 2 SKIP
```

## 遗留与回退

- 遗留：无新增待办；`ui-voice-check` 和 Admin 浏览器视觉检查未在本轮执行，Admin 行为由 RTL/构建/集成门禁覆盖。
- 回退：回退 `8038881`，恢复 `Admin.tsx` 内联 tabs，删除新增的 2 个组件与 3 个测试文件；无数据、API、CSS 或后端回退。
