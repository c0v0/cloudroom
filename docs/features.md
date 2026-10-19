# 用户可见能力（features）

> 规则（AGENTS.md「执行轮」）：改动涉及用户可见入口时，实现会话必须在这里更新一行。
> 每条能力必须给出“怎么验证”，验证方式要么是可跑命令，要么是可复现步骤。

## 现状

| 能力 | 入口 | 验证 |
|---|---|---|
| 服务启动与健康检查 | `cloudroom-server --config <cfg>`；`GET /api/v1/health` | `scripts/smoke.sh` |
| 公共配置 | `GET /api/v1/config/public` | `scripts/smoke.sh` |
| 全流程控制面（bootstrap/登录/建房审批/邀请/消息/禁言/删除/tombstone/WS 隧道） | HTTP API + WSS | `test-server/run-tests.sh` |
| 媒体凭证（p2p ice-servers / sfu media-token 与模式错配 409） | `GET /rooms/{code}/ice-servers`、`POST /rooms/{code}/media-token` | `test-server/run-tests.sh` |
| 前端页面与 SPA 回退、API 反代 | 浏览器（Vite 构建产物） | `test-server/start-test-env.sh` 后访问 8081 |

范围与产品描述见 `README.md`「当前范围」；API/WS 契约见 `docs/api.md`。

## 模板（新增能力时复制）

```markdown
- [ ] <能力一句话>（<实现位置 file:line>）— 验证：`<命令>` / <可复现步骤> — 归属会话：<日期>
```

## 已废弃 / 移除

- （空）
