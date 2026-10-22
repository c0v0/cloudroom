# 归档

本目录保留已关闭或被后续实现取代的过程记录，供追溯使用。

## 规则

- **不删除历史**：归档只移动文件，不改写原文内容；旧路径通过提交历史和本索引查找。
- **不作为当前状态入口**：当前 API、配置、用户能力和未完成项分别以 `docs/api.md`、`docs/configuration.md`、`docs/features.md`、`docs/debt.md` 和代码/测试为准。
- **命名保持稳定**：归档文件保留原文件名和日期；需要改名时只在同一提交内更新所有引用。

## plans/

- [`2026-10-19-代码分层整理.md`](plans/2026-10-19-代码分层整理.md) — 后端 `api/app/domain/infra` 分层只读盘点（含 2 项未验证）。
- [`2026-10-20-app-layer-migration.md`](plans/2026-10-20-app-layer-migration.md) — API 到 `app` 的多批次迁移方案（无独立验收表）。
- [`2026-10-21-frontend-component-tests.md`](plans/2026-10-21-frontend-component-tests.md) — 首批前端组件/hook 测试方案（无独立验收表）。
