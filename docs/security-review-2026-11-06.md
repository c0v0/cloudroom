# 2026-11-06 全量代码与公网安全审计

范围：`backend/`、`frontend/`、`frontend-server/`、`release/`、`test-server/`、部署文档。
方式：独立只读审查 + 本地静态扫描 + 构建/测试/依赖审计。结论以当前工作树为准。

## 结论

未发现新的 Critical/High 认证绕过、IDOR、SQL 注入、XSS、路径穿越或 RCE。
本轮修复了 1 个 Medium（收藏 CSV 公式注入）、1 个 Medium（CORS 缺少 PUT 导致跨域前端功能失败）
以及若干 Low/防御纵深问题；剩余风险均为已登记的部署/架构边界。

## 本轮修复

| 级别 | 问题 | 修复 |
|---|---|---|
| Medium | 收藏 CSV 公式注入：消息正文以 `=`/`+`/`-`/`@` 开头时 Excel 执行公式 | 前后端 CSV 单元均加 `'` 前缀，覆盖前导空格/制表符；补回归测试 |
| Medium | 跨域生产前端无法调用 `PUT`（CORS 只允许 GET/POST/DELETE） | `allow_methods` 增加 PUT；`allow_headers` 增加 Range/If-Range；e2e 断言 |
| Medium | 静态 docroot 由 `www-data` 可写，Web 进程被攻破可持久化 XSS/篡改 ACME | `install.sh` 改为 `root:www-data` 且去掉组/其他写权限；补发布包完整性前置检查 |
| Medium | OIDC state 仅存服务端，攻击者可把自己的回调塞给受害者（login CSRF） | state 绑定 `cloudroom_oidc_state` HttpOnly/SameSite=Lax Cookie；回调校验并清除 |
| Medium | 测试环境后端监听 `0.0.0.0`、固定 LiveKit 密钥 | 后端固定 `127.0.0.1`；LiveKit secret 每次随机生成；README 禁止暴露 8080/7880 |
| Low | 长连接 WS 使用连接时房间快照，房主后开全员禁言仍可 `voice_join` | `check_voice` 每次重读房间状态/媒体模式 |
| Low | 未认证 webhook 可用随机 token 填满限流表，满表时 O(100k) 扫描阻塞全局锁 | 满表改为 O(1) 淘汰；prune 仍定期清理 |
| Low | 链接预览 URL/推送 endpoint 可能写入日志（含 capability token/换行） | 只记录 URL 哈希 / endpoint host |
| Low | `is_public_ip` 未拒绝 NAT64、IPv6 文档段、IPv4 保留段 | 补齐拒绝列表并加测试 |
| Low | `/metrics` 未被参考 Nginx 显式拒绝；房间号 `metrics`/`twirp` 冲突 | Nginx `= /metrics`、`/twirp/` 返回 404；加入保留房间号 |
| Low | 前端 `?thread=` 未校验/编码，可构造跨 API 路径 GET | 只接受 32 位十六进制消息 ID，API 路径编码 |
| Low | 前端 `assetUrl(...) ?? raw` 绕过 scheme 白名单 | 无效 scheme 不再回退到原始 URL；附件/头像/CSS 同样处理 |
| Low | `api()` 允许绝对 URL 且仍附带 CSRF 头 | 绝对 URL 必须与配置 origin 同源，否则拒绝 |
| Low | Service Worker 推送点击未校验目标 origin；导航错误页可被缓存为壳 | 只允许同源 target；仅缓存 `ok && basic` 导航 |
| Low | 自托管静态服务器代理响应无 `no-store` | 代理 API 响应默认加 `Cache-Control: no-store`（附件/头像保留私有缓存） |
| Low | Nginx 上传体限制 8m < 20MiB 媒体；`/rtc` token 可进 access/error log | 24m；API/RTC access_log off，RTC error_log 关；CSP 补 LiveKit WSS |
| Low | LiveKit 模板建议开放 7880；unit 缺少沙箱 | 明确只开 7881/媒体/TURN；补 hardening 与 fail-closed 占位符检查 |
| Low | 手动部署 LiveKit 配置 root:root 0640，服务用户不可读 | 文档改为 `root:livekit` |

## 已验证

- 前端：`tsc -b`、ESLint `--max-warnings=0`、Vitest **83 passed**、Vite build 通过。
- 后端：`cargo test --locked --lib` **56 passed / 0 failed**；CORS 定向 e2e 通过（1 passed）。
- 依赖：`npm audit --omit=dev --audit-level=high` → **0 vulnerabilities**。
- Shell：`bash -n release/install.sh test-server/start-test-env.sh` 通过。

## 剩余风险（部署/架构边界）

- LiveKit 媒体撤销仍是 best-effort，TTL 为兜底；TURN allocation 配额需在 LiveKit/防火墙侧限制。
- P2P 语音禁言是协作式的；服务端只保证准入检查，无法阻止已建立连接继续收音频。
- SQLite 单写者、无静态加密；生产需磁盘加密、权限与加密备份。
- 无 E2EE；服务端可读文本历史。
- 房间号是定位符而非凭证；7 位随机码的抗枚举依赖速率限制。
- `/metrics` 仅应内网/回环抓取；参考 Nginx 已显式 404，直接暴露后端端口仍需避免。
- 生产 TLS/Nginx/systemd、真实麦克风、真实 Web Push 投递仍需部署环境验收。
