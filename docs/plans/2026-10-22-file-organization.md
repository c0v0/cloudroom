# 文件与目录盘点（2026-10-22）

- 模式：初始盘点为只读；执行批次 0-3 后仅更新路径引用，未改代码/测试。
- 范围：`git ls-files` 全部受控文件 + `git ls-files --others --exclude-standard` 未跟踪文件；排除 `.git`、`node_modules`、`target`、`dist`、`release` 二进制等 gitignore 生成物。
- 基线：盘点时为 `main`；执行分支为 `docs/file-organization`。
- 说明：规模统计保留执行前 174 个受控文件；文件表中的路径已更新到归档后的位置。

## 1. 范围与规模

| 区域 | 受控文件数 | 说明 |
|---|---:|---|
| 根目录文件 | 4 | `.gitignore`、`AGENTS.md`、`CHANGELOG.md`、`README.md` |
| `.githooks/` | 2 | 提交信息与暂存区密钥检查 |
| `backend/` | 42 | 38 个 Rust 源文件 + Cargo/配置 + e2e |
| `docs/` | 24 | 11 个主题文档 + 13 个 `plans/` 文件（含索引） |
| `frontend/` | 56 | 45 个 `src/` 文件 + 11 个配置/公共资源 |
| `frontend-server/` | 3 | Cargo 清单/锁 + 单文件服务器 |
| `release/` | 7 | 配置/安装/systemd/Nginx/LiveKit 模板 + 中文指南 |
| `scripts/` | 11 | 门禁、构建、复现、冒烟、密钥/许可、hooks |
| `docs/archive/spikes/` | 20 | 3 个历史媒体可行性实验 |
| `test-server/` | 5 | 测试环境、全流程、SFU、UI 冒烟 |
| **合计** | **174** | 158 个 100644 + 16 个 100755 |

摘要：

- 处置分布：`keep` **151**、`archive` **23**、`merge` 0、`move` 0、`delete` 0、`unknown` 0。
- 无逐字节相同文件；3 个 PNG 是显式 PWA 资源，不是误入库。
- 没有受控的 `dist/`、`target/`、`coverage/`、`*.tsbuildinfo` 或 release 二进制。
- 当前有 12 个被忽略路径（依赖/构建产物/运行态），均符合预期。

## 2. 目录结构

顶层（不含 `.git`）只有 4 个文件 + 8 个目录，没有源码或脚本散落在根目录：

```text
cloudroom/
├── .githooks/                  # 配置：本地 hooks
├── backend/                    # 源码：Rust 后端（api/app/domain/infra）
├── docs/                       # 文档 + plans + archive
├── frontend/                   # 源码：React/TS + Vite
├── frontend-server/            # 源码：静态前端/反代/WSS 隧道
├── release/                    # 发布包：模板/脚本；二进制与 frontend/ 被忽略
├── scripts/                    # 工程脚本
├── test-server/                # 测试支撑脚本
├── .gitignore
├── AGENTS.md
├── CHANGELOG.md
└── README.md
```

### 2.1 现状判断

- **清晰**：`backend/src` 按 `api → app → domain → infra` 分层；`frontend/src` 按 `components/hooks/lib/pages` 分；测试与实现同目录；`test-server` 与 `scripts` 职责分开。
- **`release/`**：作为发布包目录合理；当前只跟踪 7 个模板/脚本，`cloudroom-*`、`livekit-server`、`frontend/` 均被 `.gitignore` 排除，没有二进制入库。
- **`docs/archive/spikes/`**：历史媒体实验已归档；顶层不再保留研究目录，生产代码和门禁都不依赖。
- **`docs/plans/`**：已按纯计划、计划+验收、历史计划分类，历史计划移到 `docs/archive/plans/`。

### 2.2 建议的最终结构

```text
cloudroom/
├── .githooks/
├── backend/
├── docs/
│   ├── plans/                 # 只放当前发布周期内的计划/验收
│   └── archive/
│       ├── plans/             # 已关闭、超过一个发布周期的计划
│       └── spikes/            # 历史媒体 spike
├── frontend/
├── frontend-server/
├── release/                   # 只放模板/脚本；二进制与 frontend/ 不入库
├── scripts/
├── test-server/
├── .gitignore
├── AGENTS.md
├── CHANGELOG.md
└── README.md
```

不强制把 `release/` 改名：它是部署包边界，不是源码目录；但应在 README 中明确“二进制/前端产物不入库”。

## 3. 文件处置表

处置定义：

- `keep`：保留在原位置，继续作为生产/测试/文档/配置入口。
- `archive`：保留内容作为历史证据，冻结不再扩展；批次中再决定是否移到 `docs/archive/`。
- `merge`：合并为一份实现或文档；本轮没有推荐项。
- `move`：仅改名/换目录、不改内容；本轮没有立即执行项。
- `delete`：删除；本轮没有推荐项。
- `unknown`：用途无法从仓库确认；本轮没有此类文件。

### 3.1 根目录与 hooks


#### 根目录

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `.gitignore` | 限制构建产物、运行态、密钥与日志入库。 | 仓库根；`git status --ignored` 显示 12 个忽略项。 | `keep` | high |
| 2 | `AGENTS.md` | 仓库工作流协议、行为规则和项目命令/边界。 | 仓库根；执行轮/验收轮协议；本任务规则来源。 | `keep` | high |
| 3 | `CHANGELOG.md` | 按日期记录用户可见变更、验证结果和已知问题。 | README 开发者入口；`docs/release-checklist.md` 要求更新。 | `keep` | high |
| 4 | `README.md` | 产品范围、组件、开发/构建/验收入口和文档索引。 | 仓库根入口；引用 docs、release、scripts。 | `keep` | high |

#### Git hooks

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `.githooks/commit-msg` | 校验提交信息 `type(scope): 说明` 格式。 | `scripts/install-hooks.sh` 设置 `core.hooksPath=.githooks`。 | `keep` | high |
| 2 | `.githooks/pre-commit` | 提交前仅扫描暂存区的密钥。 | `scripts/install-hooks.sh`；调用 `scripts/secret-scan.sh --staged`。 | `keep` | high |


### 3.2 backend


#### API 层

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `backend/src/api/admin.rs` | 管理端 HTTP 处理器：统计、房间/申请、用户、邀请、审计。 | `backend/src/api/mod.rs`；`backend/src/api/routes.rs` 挂载。 | `keep` | high |
| 2 | `backend/src/api/auth_routes.rs` | 注册、登录、登出 HTTP 处理器。 | `backend/src/api/mod.rs`；`routes.rs` 挂载。 | `keep` | high |
| 3 | `backend/src/api/extract.rs` | 会话 Cookie 到可选/必需用户的 Axum 提取器。 | `api/mod.rs` 重导出；各 handler 使用。 | `keep` | high |
| 4 | `backend/src/api/json.rs` | API 响应的 JSON 视图构造器。 | `api/mod.rs` 重导出；handler 使用。 | `keep` | high |
| 5 | `backend/src/api/me.rs` | 个人资料、改密、头像、我的房间 HTTP 处理器。 | `api/mod.rs`；`routes.rs` 挂载。 | `keep` | high |
| 6 | `backend/src/api/mod.rs` | API 层模块声明与公共导出。 | `backend/src/lib.rs` 声明 `pub mod api`。 | `keep` | high |
| 7 | `backend/src/api/rooms.rs` | 房间、成员、消息、邀请、静音、媒体凭证 HTTP 处理器。 | `api/mod.rs`；`routes.rs` 挂载。 | `keep` | high |
| 8 | `backend/src/api/routes.rs` | 构建 Axum 路由、公开端点和请求日志。 | `backend/src/main.rs` 调用 `build_router`。 | `keep` | high |
| 9 | `backend/src/api/security.rs` | 客户端 IP、CSRF/Origin 校验和 WebSocket 升级守卫。 | `api/mod.rs` 重导出；`routes.rs` 使用。 | `keep` | high |
| 10 | `backend/src/api/ws.rs` | 房间 WebSocket 端点、事件循环、房间状态和语音信令。 | `api/routes.rs` 挂载；`useRoomSocket` 对端。 | `keep` | high |

#### 应用层

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `backend/src/app/access.rs` | 成员关系与能力检查的应用层入口。 | `app/mod.rs`；`api/mod.rs` 重导出；rooms/ws 使用。 | `keep` | high |
| 2 | `backend/src/app/admin.rs` | 管理端写流程：申请决策、所有权、用户、邀请。 | `app/mod.rs`；`api/admin.rs` 调用。 | `keep` | high |
| 3 | `backend/src/app/audit.rs` | 共享审计日志写入辅助。 | `app/mod.rs`；`api/mod.rs` 重导出。 | `keep` | high |
| 4 | `backend/src/app/avatar.rs` | 头像上传/删除工作流与旧文件清理。 | `app/mod.rs`；`api/me.rs` 调用。 | `keep` | high |
| 5 | `backend/src/app/events.rs` | 通过内存 Hub 向房间广播应用事件。 | `app/mod.rs`；`api/rooms.rs` 调用。 | `keep` | high |
| 6 | `backend/src/app/invites.rs` | 房间和管理端共用的邀请使用/撤销规则。 | `app/mod.rs`；`api/admin.rs`/`rooms.rs` 调用。 | `keep` | high |
| 7 | `backend/src/app/messages.rs` | 发消息用例：校验、持久化、序号、回复快照、静音检查。 | `app/mod.rs`；`api/rooms.rs` 调用。 | `keep` | high |
| 8 | `backend/src/app/mod.rs` | 应用层模块声明。 | `backend/src/lib.rs` 声明 `pub mod app`。 | `keep` | high |
| 9 | `backend/src/app/requests.rs` | 建房/媒体模式变更申请的创建、校验与自动批准。 | `app/mod.rs`；`api/rooms.rs` 调用。 | `keep` | high |
| 10 | `backend/src/app/rooms.rs` | 房间生命周期与语音访问的应用工作流。 | `app/mod.rs`；`api/rooms.rs` 与 `app/sweeper.rs` 使用。 | `keep` | high |
| 11 | `backend/src/app/state.rs` | 所有 handler 共享的 AppState。 | `app/mod.rs`；`api/mod.rs` 重导出。 | `keep` | high |
| 12 | `backend/src/app/sweeper.rs` | 后台清理：过期房间、私聊空闲、宽限期、会话、限流桶。 | `app/mod.rs`；`backend/src/main.rs` 启动。 | `keep` | high |

#### 领域层

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `backend/src/domain/codes.rs` | 房间号生成、规范化和校验规则。 | `domain/mod.rs`；`app/rooms.rs` 使用。 | `keep` | high |
| 2 | `backend/src/domain/mod.rs` | 领域层模块声明（无 I/O）。 | `backend/src/lib.rs` 声明 `pub mod domain`。 | `keep` | high |
| 3 | `backend/src/domain/models.rs` | 镜像 SQLite schema 的领域模型与序列化类型。 | `domain/mod.rs`；各层共享。 | `keep` | high |

#### 基础设施层

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `backend/src/infra/auth.rs` | Argon2id 密码哈希、会话/CSRF token、Cookie 编解码。 | `infra/mod.rs`；`api/extract.rs` 与 `app/state.rs` 使用。 | `keep` | high |
| 2 | `backend/src/infra/avatars.rs` | 头像签名检测、解码、尺寸限制、重编码和随机文件名。 | `infra/mod.rs`；`app/avatar.rs` 使用。 | `keep` | high |
| 3 | `backend/src/infra/db.rs` | SQLite 连接、pragmas、单写互斥和迁移。 | `infra/mod.rs`；`app/state.rs` 与 `main.rs` 使用。 | `keep` | high |
| 4 | `backend/src/infra/hub.rs` | 内存 WebSocket 连接表、广播、语音在线状态和信令中继。 | `infra/mod.rs`；`app/events.rs` 与 `api/ws.rs` 使用。 | `keep` | high |
| 5 | `backend/src/infra/livekit.rs` | LiveKit 适配器：SFU JWT 与 P2P TURN 凭证。 | `infra/mod.rs`；`api/rooms.rs` 使用。 | `keep` | high |
| 6 | `backend/src/infra/mod.rs` | 基础设施层模块声明。 | `backend/src/lib.rs` 声明 `pub mod infra`。 | `keep` | high |
| 7 | `backend/src/infra/rate_limit.rs` | 按桶/键的内存滑动窗口限流器。 | `infra/mod.rs`；`app/state.rs` 与 `api` 使用。 | `keep` | high |
| 8 | `backend/src/infra/repo.rs` | SQLite 仓储：SQL 的唯一集中实现。 | `infra/mod.rs`；`app` 工作流调用。 | `keep` | high |

#### 入口、共享基础与测试

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `backend/.cargo/config.toml` | musl 目标的链接器与自包含配置。 | `scripts/build-musl.sh` 构建 x86_64-unknown-linux-musl。 | `keep` | high |
| 2 | `backend/Cargo.lock` | 后端依赖锁定与审计/复现输入。 | `scripts/check.sh`、`scripts/repro.sh`、`cargo audit`。 | `keep` | high |
| 3 | `backend/Cargo.toml` | 后端 crate 清单、依赖和二进制目标。 | `cargo build/test --locked`；`scripts/build-musl.sh`。 | `keep` | high |
| 4 | `backend/tests/e2e.rs` | 后端端到端测试：认证、房间、邀请、消息、WS、媒体凭证。 | `cargo test --locked`；`scripts/check.sh` 的 Rust 测试阶段。 | `keep` | high |
| 5 | `backend/src/config.rs` | TOML 配置加载、默认值、校验和环境覆盖。 | `backend/src/main.rs`；文档 `docs/configuration.md`。 | `keep` | high |
| 6 | `backend/src/error.rs` | 统一 `{error:{code,message}}` API 错误类型。 | `backend/src/lib.rs` 重导出；所有层使用。 | `keep` | high |
| 7 | `backend/src/lib.rs` | 库 crate 入口，声明分层模块与公共类型。 | `backend/Cargo.toml` 的 `[lib]` 默认目标；`main.rs` 使用。 | `keep` | high |
| 8 | `backend/src/main.rs` | 二进制入口：配置、DB、Hub、路由、后台任务和关停。 | `backend/Cargo.toml` 的 `[[bin]]`。 | `keep` | high |
| 9 | `backend/src/util.rs` | 共享的时间、随机数、哈希和文本校验辅助。 | `backend/src/lib.rs` 声明；各层调用。 | `keep` | high |


### 3.3 docs

#### 主题文档


| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `docs/api.md` | HTTP/WebSocket 公开契约、错误和事件语义。 | README 目录索引；实现与测试引用。 | `keep` | high |
| 2 | `docs/architecture.md` | 控制面/媒体面、P2P/SFU、审批时序和边界。 | README 目录索引；`docs/security.md` 交叉引用。 | `keep` | high |
| 3 | `docs/backlog.md` | 范围外待办，当前仅一条 LiveKit 二进制获取项。 | AGENTS §1.5；README 未直接索引。 | `keep` | high |
| 4 | `docs/configuration.md` | 配置键、默认值和部署示例。 | README 目录索引；`release/config.toml.example` 指向。 | `keep` | high |
| 5 | `docs/debt.md` | 28 项技术债/留待项登记表及复检条件。 | AGENTS §1.12；`docs/plans/2026-10-22-deferred-register.md`。 | `keep` | high |
| 6 | `docs/decisions.md` | ADR：对外变化、验收基线、分层和访问控制决策。 | AGENTS；`docs/release-checklist.md`；实现引用。 | `keep` | high |
| 7 | `docs/deploy-checklist.md` | TLS/Nginx/systemd/安装/历史数据/真实设备逐步验收。 | README 目录索引；`docs/debt.md` D-013~D-018。 | `keep` | high |
| 8 | `docs/deployment.md` | 英文生产部署指南（Nginx、systemd、LiveKit、备份）。 | README 目录索引；`test-server/README.md` 引用。 | `keep` | high |
| 9 | `docs/features.md` | 用户可见能力与验证命令清单。 | AGENTS 执行轮要求；README 产品范围。 | `keep` | high |
| 10 | `docs/release-checklist.md` | 每次发版的固定顺序、哈希、CHANGELOG 和 tag。 | README 目录索引；`docs/plans/2026-10-22-deferred-register.md`。 | `keep` | high |
| 11 | `docs/security.md` | 威胁模型、认证/会话/CSRF/媒体边界。 | README 目录索引；架构/部署文档交叉引用。 | `keep` | high |


#### `docs/plans/`


| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `docs/archive/plans/2026-10-19-代码分层整理.md` | 分层重构的只读盘点、验收表和提交列表（含 2 项未验证）。 | `docs/decisions.md:47` 引用；无运行时引用。 | `archive` | medium |
| 2 | `docs/archive/plans/2026-10-20-app-layer-migration.md` | API→app 迁移的批次计划；未包含执行验收记录。 | `backend/src/app/mod.rs` 反映迁移；无 acceptance 表。 | `archive` | medium |
| 3 | `docs/plans/2026-10-21-admin-tabs.md` | Admin 邀请/用户 tab 抽取计划、验收和提交 `8038881`。 | `Admin.tsx` 引用 `InvitesTab`/`UsersTab`；测试文件。 | `keep` | high |
| 4 | `docs/archive/plans/2026-10-21-frontend-component-tests.md` | 首批前端组件/hooks 测试计划；无执行验收表。 | `frontend/vitest.config.ts`、`src/test/setup.ts` 和测试文件。 | `archive` | medium |
| 5 | `docs/plans/2026-10-21-kicked-rejoin.md` | 被踢用户重新加入需新邀请的决策与实现说明。 | `backend/src/app/rooms.rs`、e2e；`docs/debt.md`。 | `keep` | high |
| 6 | `docs/plans/2026-10-21-left-at-access.md` | 已离开成员 HTTP/WS 访问拒绝的决策与实现说明。 | `backend/src/app/access.rs`、`api/ws.rs`、e2e。 | `keep` | high |
| 7 | `docs/plans/2026-10-21-owner-transfer.md` | 房主转让/降级和 direct room 例外。 | `backend/src/app/admin.rs`、`infra/repo.rs`；D-016。 | `keep` | high |
| 8 | `docs/plans/2026-10-21-ws-events-decision.md` | WS 30 处 repo 调用保留在 API 的决策与测试。 | `backend/src/api/ws.rs`；`docs/debt.md` D-021。 | `keep` | high |
| 9 | `docs/plans/2026-10-22-close-gate-gaps.md` | ESLint 与 cargo-audit 门禁补齐的验收记录。 | `frontend/eslint.config.js`、`scripts/check.sh`。 | `keep` | high |
| 10 | `docs/plans/2026-10-22-debt-closure.md` | 依赖告警、测试/部署债务与 UI 冒烟收口记录。 | `docs/debt.md`、`scripts/ui-smoke.sh`。 | `keep` | high |
| 11 | `docs/plans/2026-10-22-deferred-register.md` | 留待项登记表与 D-019/D-020 本地验证记录。 | `docs/debt.md`；README 索引。 | `keep` | high |
| 12 | `docs/plans/2026-10-22-room-message-list.md` | Room 消息列表抽取、验收与提交 `41847be`。 | `frontend/src/components/MessageList*`、`Room.tsx`。 | `keep` | high |
| 13 | `docs/plans/README.md` | plans 目录命名与验收记录模板。 | AGENTS 执行轮；目录自身。 | `keep` | high |


### 3.4 frontend

#### 工程配置与公共资源


| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `frontend/eslint.config.js` | ESLint js/ts/react-hooks 规则和忽略目录。 | `npm run lint`；`scripts/check.sh`。 | `keep` | high |
| 2 | `frontend/index.html` | SPA HTML 入口、PWA manifest 和图标引用。 | `frontend/src/main.tsx`；`public/manifest.json`。 | `keep` | high |
| 3 | `frontend/package-lock.json` | 前端依赖锁定与干净复现输入。 | `npm ci`；`scripts/check.sh`/`repro.sh`。 | `keep` | high |
| 4 | `frontend/package.json` | 前端依赖、脚本和 Vite/ESLint/Vitest 入口。 | `npm test/typecheck/build/lint`；`scripts/check.sh`。 | `keep` | high |
| 5 | `frontend/public/apple-touch-icon.png` | iOS 主屏图标。 | `index.html:10`。 | `keep` | high |
| 6 | `frontend/public/icon-192.png` | PWA/浏览器 192px 图标。 | `index.html:9`；`manifest.json`。 | `keep` | high |
| 7 | `frontend/public/icon-512.png` | PWA maskable 512px 图标。 | `manifest.json`。 | `keep` | high |
| 8 | `frontend/public/manifest.json` | PWA 名称、主题和图标声明。 | `frontend/index.html:8`。 | `keep` | high |
| 9 | `frontend/tsconfig.json` | TypeScript 项目和编译选项。 | `npm run typecheck/build`；Vite。 | `keep` | high |
| 10 | `frontend/vite.config.ts` | Vite 构建、API 代理和 chunk 拆分。 | `npm run dev/build`；`index.html`。 | `keep` | high |
| 11 | `frontend/vitest.config.ts` | Vitest jsdom、测试 glob 和 setup 文件。 | `npm test`；`src/test/setup.ts`。 | `keep` | high |


#### 应用入口、页面与样式


| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `frontend/src/App.test.tsx` | App 路由与角色可见性的 RTL 测试。 | `vitest.config.ts`；`App.tsx`。 | `keep` | high |
| 2 | `frontend/src/App.tsx` | 应用路由、懒加载页面、登录态和主题壳。 | `main.tsx` 渲染；`auth.tsx`。 | `keep` | high |
| 3 | `frontend/src/api.ts` | 同源 fetch 客户端、错误解析、CSRF 和 WS URL。 | 各页面/hooks 导入；`auth.tsx`。 | `keep` | high |
| 4 | `frontend/src/auth.tsx` | AuthProvider/useAuth 登录态上下文。 | `main.tsx`；页面与 `App.tsx`。 | `keep` | high |
| 5 | `frontend/src/main.tsx` | ReactDOM 入口：Router、AuthProvider、全局样式。 | `index.html` 的 module script。 | `keep` | high |
| 6 | `frontend/src/pages/Admin.test.tsx` | Admin 默认 tab 和 tab 切换测试。 | `vitest.config.ts`；`Admin.tsx`。 | `keep` | high |
| 7 | `frontend/src/pages/Admin.tsx` | 管理后台壳、tab 状态与统计；邀请/用户 tab 已抽出。 | `components/InvitesTab`、`UsersTab`；`Admin.test.tsx`。 | `keep` | high |
| 8 | `frontend/src/pages/Home.tsx` | 首页房间列表、建房申请和导航。 | `App.tsx` 路由；`roomCode.ts`。 | `keep` | high |
| 9 | `frontend/src/pages/Login.tsx` | 登录表单与导航。 | `App.tsx` 路由；`auth.tsx`。 | `keep` | high |
| 10 | `frontend/src/pages/Register.test.tsx` | 注册邀请流程的 RTL 测试。 | `vitest.config.ts`；`Register.tsx`。 | `keep` | high |
| 11 | `frontend/src/pages/Register.tsx` | 邀请/bootstrap 注册流程。 | `App.tsx` 路由；`Register.test.tsx`。 | `keep` | high |
| 12 | `frontend/src/pages/Room.tsx` | 房间容器：WS、成员、消息、语音和设置；消息列表已抽出。 | `App.tsx` 路由；`MessageList`、`useRoomSocket`、`useVoiceSession`。 | `keep` | high |
| 13 | `frontend/src/pages/Settings.test.tsx` | 设置密码校验等 RTL 测试。 | `vitest.config.ts`；`Settings.tsx`。 | `keep` | high |
| 14 | `frontend/src/pages/Settings.tsx` | 个人设置：密码、头像、降噪、主题和麦克风测试。 | `Room.tsx` 打开；`auth.tsx`。 | `keep` | high |
| 15 | `frontend/src/styles.css` | 全局主题、布局、消息、语音和响应式样式。 | `main.tsx:6` 导入。 | `keep` | high |
| 16 | `frontend/src/test/setup.ts` | Vitest 全局 setup：jest-dom、清理和 mock。 | `frontend/vitest.config.ts` 的 setupFiles。 | `keep` | high |
| 17 | `frontend/src/types/rnnoise-wasm.d.ts` | 补充 `@jitsi/rnnoise-wasm` 的最小 TypeScript 声明。 | `denoise.ts` 导入；`tsconfig` 类型检查。 | `keep` | high |
| 18 | `frontend/src/vite-env.d.ts` | Vite 客户端类型引用。 | `tsconfig`/Vite 类型检查。 | `keep` | high |


#### 组件、hooks 与纯函数


| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `frontend/src/components/InvitesTab.test.tsx` | 邀请 tab 的可见性、空态、409 和撤销测试。 | `vitest.config.ts`；`InvitesTab.tsx`。 | `keep` | high |
| 2 | `frontend/src/components/InvitesTab.tsx` | 邀请 tab 容器与 InvitesView 展示组件。 | `Admin.tsx:5`；`InvitesTab.test.tsx`。 | `keep` | high |
| 3 | `frontend/src/components/MessageList.test.tsx` | 消息顺序、合并去重、时间、空态、系统消息、回复引用测试。 | `vitest.config.ts`；`MessageList.tsx`。 | `keep` | high |
| 4 | `frontend/src/components/MessageList.tsx` | Room 消息列表/消息项展示组件。 | `Room.tsx:13`；`MessageList.test.tsx`。 | `keep` | high |
| 5 | `frontend/src/components/UsersTab.test.tsx` | 用户 tab 权限、临时密码、错误和空态测试。 | `vitest.config.ts`；`UsersTab.tsx`。 | `keep` | high |
| 6 | `frontend/src/components/UsersTab.tsx` | 用户 tab 容器与 UsersView 展示/权限控件。 | `Admin.tsx:6`；`UsersTab.test.tsx`。 | `keep` | high |
| 7 | `frontend/src/hooks/chime.ts` | Web Audio 合成入会/离会提示音。 | `Room.tsx:9` 导入。 | `keep` | high |
| 8 | `frontend/src/hooks/denoise.ts` | 降噪模式、偏好存储与 rnnoise WASM 引擎。 | Settings/useMicTest/useP2PVoice/useLiveKitVoice 导入。 | `keep` | high |
| 9 | `frontend/src/hooks/remoteAudio.ts` | 远端音频播放链、音量/动态偏好。 | useP2PVoice/useLiveKitVoice/Room 导入。 | `keep` | high |
| 10 | `frontend/src/hooks/theme.ts` | 主题偏好与 `data-theme` 应用。 | `App.tsx:4`、`Settings.tsx:8`。 | `keep` | high |
| 11 | `frontend/src/hooks/throttle.ts` | 用户操作的前沿/尾部节流。 | `Room.tsx:10`；语音批准/静音操作。 | `keep` | high |
| 12 | `frontend/src/hooks/useLiveKitVoice.ts` | SFU 模式 LiveKit 发布/订阅与音量。 | `useVoiceSession.ts:5`；`Room.tsx` 间接。 | `keep` | high |
| 13 | `frontend/src/hooks/useMicTest.ts` | 本地麦克风测试：原声/降噪双轨与音量。 | `Room.tsx:11`；`Settings` 测试入口。 | `keep` | high |
| 14 | `frontend/src/hooks/useP2PVoice.ts` | P2P 模式 WebRTC 连接、信令、音量和清理。 | `useVoiceSession.ts:3`；`Room.tsx`。 | `keep` | high |
| 15 | `frontend/src/hooks/useRoomSocket.test.ts` | WS 连接/重连/清理的 hook 测试。 | `vitest.config.ts`；`useRoomSocket.ts`。 | `keep` | high |
| 16 | `frontend/src/hooks/useRoomSocket.ts` | 房间 WebSocket 连接、重连、事件分发。 | `Room.tsx:5`；`useRoomSocket.test.ts`。 | `keep` | high |
| 17 | `frontend/src/hooks/useVoiceSession.test.tsx` | 语音模式选择和适配器接口的 hook 测试。 | `vitest.config.ts`；`useVoiceSession.ts`。 | `keep` | high |
| 18 | `frontend/src/hooks/useVoiceSession.ts` | P2P/SFU 语音适配器选择与会话状态。 | `Room.tsx:7`；`useVoiceSession.test.tsx`。 | `keep` | high |
| 19 | `frontend/src/hooks/volumeMeter.ts` | 每流 AnalyserNode 音量检测和 VAD 阈值。 | useP2PVoice/useLiveKitVoice/Settings 导入。 | `keep` | high |
| 20 | `frontend/src/lib/messages.test.ts` | 消息合并/去重的纯函数测试。 | `vitest.config.ts`；`messages.ts`。 | `keep` | high |
| 21 | `frontend/src/lib/messages.ts` | 消息追加、顺序保持和按 id 去重。 | `Room.tsx:14`；`messages.test.ts`。 | `keep` | high |
| 22 | `frontend/src/lib/roles.test.ts` | 角色标签映射测试。 | `vitest.config.ts`；`roles.ts`。 | `keep` | high |
| 23 | `frontend/src/lib/roles.ts` | 角色到中文标签的映射。 | `UsersTab.tsx`、`Settings.tsx`；`roles.test.ts`。 | `keep` | high |
| 24 | `frontend/src/lib/roomCode.test.ts` | 房间码规则测试。 | `vitest.config.ts`；`roomCode.ts`。 | `keep` | high |
| 25 | `frontend/src/lib/roomCode.ts` | 房间码规范化和路由段匹配。 | `App.tsx`、`Home.tsx`；`roomCode.test.ts`。 | `keep` | high |
| 26 | `frontend/src/lib/time.test.ts` | 时间格式化边界测试。 | `vitest.config.ts`；`time.ts`。 | `keep` | high |
| 27 | `frontend/src/lib/time.ts` | 消息时间格式化。 | `MessageList.tsx`；`time.test.ts`。 | `keep` | high |


### 3.5 frontend-server（3）

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `frontend-server/Cargo.lock` | 前端服务器依赖锁定与审计/复现输入。 | `cargo audit`；`--locked`。 | `keep` | high |
| 2 | `frontend-server/Cargo.toml` | 静态前端服务器 crate 清单和 release profile。 | `scripts/build-musl.sh`/`build-release.sh`。 | `keep` | high |
| 3 | `frontend-server/src/main.rs` | 静态文件、API 反代、WSS 隧道和 TLS 的单文件服务器。 | `frontend-server/Cargo.toml`；部署清单/测试环境。 | `keep` | high |


### 3.6 release（7）

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `release/config.toml.example` | 生产服务端配置模板，覆盖 server/data/auth/rooms/media/voice。 | `backend/src/main.rs` 的配置加载；`docs/configuration.md`。 | `keep` | high |
| 2 | `release/install.sh` | Debian/Ubuntu 一键安装后端、前端、LiveKit、配置和 systemd。 | `docs/deploy-checklist.md` §4；`release/部署安装指南.md`。 | `keep` | high |
| 3 | `release/livekit/livekit-server.service.example` | LiveKit systemd 服务模板。 | `docs/deployment.md`；`release/install.sh`。 | `keep` | high |
| 4 | `release/livekit/livekit.yaml.example` | LiveKit 配置模板：端口、RTC、内嵌 TURN 和 key 占位。 | `docs/deployment.md`；`release/install.sh`。 | `keep` | high |
| 5 | `release/nginx/cloudroom.conf.example` | Nginx TLS/反代/WSS 站点模板。 | `docs/deployment.md`；`test-server/README.md`。 | `keep` | high |
| 6 | `release/systemd/cloudroom-server.service` | CloudRoom 后端 systemd 加固模板。 | `docs/deployment.md`；`release/install.sh`。 | `keep` | high |
| 7 | `release/部署安装指南.md` | 中文生产部署安装指南（与 docs/deployment.md 双语并行）。 | `README.md:41`；`scripts/build-release.sh`。 | `keep` | medium |


### 3.7 scripts（11）

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `scripts/build-musl.sh` | 可复现的 x86_64-musl static-pie 发布构建（backend/frontend-server/frontend）。 | README；`docs/release-checklist.md`；`backend/.cargo/config.toml`。 | `keep` | high |
| 2 | `scripts/build-release.sh` | 生成本地/测试用 `release/` 运行产物，优先静态 musl。 | `scripts/check.sh`、`scripts/smoke.sh`；`docs/release-checklist.md`。 | `keep` | high |
| 3 | `scripts/check.sh` | 唯一一键验收：密钥、Rust、前端、E2E、漏洞、许可、锁文件。 | AGENTS §2；README；`docs/decisions.md` ADR-002。 | `keep` | high |
| 4 | `scripts/install-hooks.sh` | 设置本地 `core.hooksPath` 启用仓库 hooks。 | `.githooks/*`；脚本头注释。 | `keep` | high |
| 5 | `scripts/license-exceptions.txt` | 许可未知项的精确例外清单。 | `scripts/license-report.sh` 读取。 | `keep` | high |
| 6 | `scripts/license-report.sh` | 依赖许可清单和未知许可门禁。 | `scripts/check.sh` 第 10 步。 | `keep` | high |
| 7 | `scripts/repro.sh` | 干净 clone 后按 lockfile 安装/构建/测试。 | `docs/backlog.md`；`scripts/check.sh` 注释。 | `keep` | high |
| 8 | `scripts/secret-allowlist.txt` | 密钥扫描的精确误报白名单。 | `scripts/secret-scan.sh`；`.githooks/pre-commit`。 | `keep` | high |
| 9 | `scripts/secret-scan.sh` | 工作区、暂存区、全历史密钥扫描。 | `scripts/check.sh` 第 1 步；pre-commit。 | `keep` | high |
| 10 | `scripts/smoke.sh` | 后端最小健康闭环冒烟（health/public config/清理）。 | `docs/features.md`；`docs/backlog.md`。 | `keep` | high |
| 11 | `scripts/ui-smoke.sh` | 本地 UI 语音冒烟：启动测试环境、创建 SFU 用户/房间、Chrome CDP。 | `docs/release-checklist.md`；`docs/debt.md` D-018/D-020。 | `keep` | high |


### 3.8 spikes（20）

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `docs/archive/spikes/001-p2p-turn/README.md` | P2P TURN relay 凭证/relay-only 可行性（结论 FAIL）的实验说明、运行命令和结论。 | `docs/archive/spikes/README.md`；无生产代码引用。 | `archive` | medium |
| 2 | `docs/archive/spikes/001-p2p-turn/package-lock.json` | 001-p2p-turn 实验的依赖锁定。 | `docs/archive/spikes/001-p2p-turn/package.json`；npm 安装。 | `archive` | high |
| 3 | `docs/archive/spikes/001-p2p-turn/package.json` | 001-p2p-turn 实验的 Node 依赖和运行脚本。 | `docs/archive/spikes/001-p2p-turn/README.md`；目录内 lockfile。 | `archive` | high |
| 4 | `docs/archive/spikes/001-p2p-turn/probe.js` | 001-p2p-turn 实验的页面内 WebRTC 探测逻辑。 | `docs/archive/spikes/001-p2p-turn/run.mjs`；目录 README。 | `archive` | high |
| 5 | `docs/archive/spikes/001-p2p-turn/result.json` | 001-p2p-turn 实验的实测结果摘要。 | `docs/archive/spikes/001-p2p-turn/README.md`；`docs/archive/spikes/README.md`。 | `archive` | high |
| 6 | `docs/archive/spikes/001-p2p-turn/run.mjs` | 001-p2p-turn 实验的 Node/Playwright 驱动。 | `docs/archive/spikes/001-p2p-turn/README.md`；`node run.mjs`。 | `archive` | high |
| 7 | `docs/archive/spikes/002-sfu-tunnel/README.md` | LiveKit SFU 经 frontend-server `/rtc/` 隧道的双向音频（结论未判定）的实验说明、运行命令和结论。 | `docs/archive/spikes/README.md`；无生产代码引用。 | `archive` | medium |
| 8 | `docs/archive/spikes/002-sfu-tunnel/package-lock.json` | 002-sfu-tunnel 实验的依赖锁定。 | `docs/archive/spikes/002-sfu-tunnel/package.json`；npm 安装。 | `archive` | high |
| 9 | `docs/archive/spikes/002-sfu-tunnel/package.json` | 002-sfu-tunnel 实验的 Node 依赖和运行脚本。 | `docs/archive/spikes/002-sfu-tunnel/README.md`；目录内 lockfile。 | `archive` | high |
| 10 | `docs/archive/spikes/002-sfu-tunnel/probe.js` | 002-sfu-tunnel 实验的页面内 WebRTC 探测逻辑。 | `docs/archive/spikes/002-sfu-tunnel/run.mjs`；目录 README。 | `archive` | high |
| 11 | `docs/archive/spikes/002-sfu-tunnel/result.json` | 002-sfu-tunnel 实验的实测结果摘要。 | `docs/archive/spikes/002-sfu-tunnel/README.md`；`docs/archive/spikes/README.md`。 | `archive` | high |
| 12 | `docs/archive/spikes/002-sfu-tunnel/run.mjs` | 002-sfu-tunnel 实验的 Node/Playwright 驱动。 | `docs/archive/spikes/002-sfu-tunnel/README.md`；`node run.mjs`。 | `archive` | high |
| 13 | `docs/archive/spikes/003-rnnoise-48k/README.md` | rnnoise WASM 在 48kHz AudioWorklet 的稳定性/时延（功能 PASS，时延未判定）的实验说明、运行命令和结论。 | `docs/archive/spikes/README.md`；无生产代码引用。 | `archive` | medium |
| 14 | `docs/archive/spikes/003-rnnoise-48k/index.html` | rnnoise 48k 实验的浏览器页面/Worklet 宿主。 | `docs/archive/spikes/003-rnnoise-48k/run.mjs`；目录 README。 | `archive` | high |
| 15 | `docs/archive/spikes/003-rnnoise-48k/main.js` | rnnoise 48k 实验的页面内处理逻辑。 | `docs/archive/spikes/003-rnnoise-48k/index.html`；目录 README。 | `archive` | high |
| 16 | `docs/archive/spikes/003-rnnoise-48k/package-lock.json` | 003-rnnoise-48k 实验的依赖锁定。 | `docs/archive/spikes/003-rnnoise-48k/package.json`；npm 安装。 | `archive` | high |
| 17 | `docs/archive/spikes/003-rnnoise-48k/package.json` | 003-rnnoise-48k 实验的 Node 依赖和运行脚本。 | `docs/archive/spikes/003-rnnoise-48k/README.md`；目录内 lockfile。 | `archive` | high |
| 18 | `docs/archive/spikes/003-rnnoise-48k/result.json` | 003-rnnoise-48k 实验的实测结果摘要。 | `docs/archive/spikes/003-rnnoise-48k/README.md`；`docs/archive/spikes/README.md`。 | `archive` | high |
| 19 | `docs/archive/spikes/003-rnnoise-48k/run.mjs` | 003-rnnoise-48k 实验的 Node/Playwright 驱动。 | `docs/archive/spikes/003-rnnoise-48k/README.md`；`node run.mjs`。 | `archive` | high |
| 20 | `docs/archive/spikes/README.md` | 三个媒体可行性 spike 的索引、结论和沙箱限制。 | 目录自身；无外部生产引用。 | `archive` | medium |


### 3.9 test-server（5）

| # | 路径 | 作用 | 引用证据 | 处置 | 置信度 |
|---:|---|---|---|---|---|
| 1 | `test-server/README.md` | 测试环境/全流程/语音验证脚本的入口说明。 | README 组件表；`scripts/check.sh`/`ui-smoke.sh`。 | `keep` | high |
| 2 | `test-server/run-tests.sh` | 零依赖全流程自动化验收（控制面/WS/媒体凭证/静态托管）。 | `scripts/check.sh` 第 8 步；`docs/features.md`。 | `keep` | high |
| 3 | `test-server/sfu-verify.sh` | 在常驻环境上验证 P2P TURN 与 SFU JWT 两条媒体路径。 | `release/部署安装指南.md`；`docs/plans/2026-10-22-close-gate-gaps.md`。 | `keep` | high |
| 4 | `test-server/start-test-env.sh` | 常驻本地测试环境：后端、前端服务器、LiveKit、自签 TLS。 | `scripts/ui-smoke.sh`；spikes README；test-server README。 | `keep` | high |
| 5 | `test-server/ui-voice-check.mjs` | Chrome CDP 登录、进房、发布/离开的 UI 语音冒烟。 | `scripts/ui-smoke.sh`；`test-server/README.md`。 | `keep` | high |


## 4. 垃圾、生成物与 `.gitignore`

### 4.1 结论

- **未发现** `*.bak`、`*~`、`.DS_Store`、`Thumbs.db`、`*.log`、`*.tmp`、copy/副本文件。
- **未发现**受控的 `dist/`、`target/`、`node_modules/`、`coverage/`、`*.tsbuildinfo`、release 二进制。
- `frontend/public/*.png` 是 PWA 图标，`Cargo.lock`/`package-lock.json` 是复现/审计输入，均为有意入库。
- 当前被忽略的 12 个路径：`backend/target/`、`frontend-server/target/`、`frontend/dist/`、`frontend/node_modules/`、`frontend/tsconfig.tsbuildinfo`、3 个 release 二进制/前端目录、3 个 spike `node_modules/`、`test-server/.test-env/`。

### 4.2 `.gitignore` 遗漏

现有规则已覆盖依赖、Rust/前端产物、release 二进制、测试运行态、密钥、数据库和日志。建议补充以下低风险模式：

```gitignore
# 编辑器/OS
.DS_Store
Thumbs.db
*.swp
*.swo

# 编辑器/工具缓存
.eslintcache
.vitest/
coverage/
playwright-report/
test-results/
```

说明：`*.pem`/`*.key` 已被忽略，但不要把真实证书放进仓库；生产证书由部署方管理。

## 5. 重复与近似重复

逐字节检查（`git ls-files | xargs sha256sum`）没有重复文件。以下“同名/近似”不是错误：

| 对象 | 判断 | 建议 |
|---|---|---|
| `backend/src/api/admin.rs` / `app/admin.rs` | 两层职责不同：HTTP 与写流程 | 保留 |
| `backend/src/api/rooms.rs` / `app/rooms.rs` | 传输层与房间工作流 | 保留 |
| `backend/Cargo.*` / `frontend-server/Cargo.*` | 两个独立 crate | 保留 |
| `frontend/package-lock.json` / spike lockfiles | 不同实验的依赖锁定 | 保留；spike 归档 |
| `docs/deployment.md` / `release/部署安装指南.md` | 英文/中文两套部署指南，内容有重叠但受众不同 | 保留双语；增加互相链接与“同步修改”提示 |
| `docs/deploy-checklist.md` / 两份部署指南 | 检查单 vs 操作手册 | 保留；命令重复是验收可复制性需要 |
| `scripts/build-release.sh` / `build-musl.sh` | 本地/测试 release vs 可复现 musl static-pie | 保留；在 README 明确选择条件 |
| `test-server/run-tests.sh` / `sfu-verify.sh` | 全流程 vs 常驻环境上的 P2P/SFU 专项 | 保留 |
| `docs/archive/spikes/001` / `002` 的 `run.mjs` | 独立实验的相似脚手架 | 不合并；归档为历史 |
| `docs/plans/2026-10-21-admin-tabs.md` | 计划 + 验收记录，代码已抽取 | 保留；索引中标注为“计划+验收” |

## 6. 文档整理

### 6.1 发现

1. **失效引用（已修复）**：`docs/decisions.md:38` 曾指向不存在的旧计划文件，批次 0 已改为实际存在的 `docs/archive/plans/2026-10-19-代码分层整理.md`。
2. **README 索引（已修复）**：批次 0 已新增“文档索引”，覆盖 `docs/features.md`、`docs/backlog.md`、`docs/decisions.md`、`docs/plans/README.md` 和归档后的 spike 索引。
3. **plans 语义（已修复）**：`docs/plans/README.md` 已按纯计划、计划+验收、历史计划重写，并列出当前文件与归档入口。
4. **历史计划（已归档）**：3 个历史/计划-only 文件已移到 `docs/archive/plans/`，保留原文，不再作为当前完成状态入口。
5. **双语部署文档**：`docs/deployment.md` 与 `release/部署安装指南.md` 是英文/中文并行版本，不是重复文件；需要明确同步责任，避免配置键漂移。
6. **`docs/debt.md` 是活登记表**：28 项、14 fixed、6 blocked、6 accepted、2 wontfix；不应归档。
7. **`docs/backlog.md` 是范围外待办**：当前唯一 open item 是 `release/livekit-server` 获取方式，应保留并与 D-018 区分。

### 6.2 归档建议

- **现在不要归档** `docs/debt.md`、当前发布周期内的 2026-10-22 计划/验收。
- 将 3 个历史/计划-only 文件移到 `docs/archive/plans/`：
  - `2026-10-19-代码分层整理.md`（建议同时改名为 `2026-10-19-layered-structure.md`）
  - `2026-10-20-app-layer-migration.md`
  - `2026-10-21-frontend-component-tests.md`
- 移动后更新 `docs/decisions.md`、`docs/plans/README.md` 的引用；不要删除历史内容。
- `docs/plans/README.md` 增加 `status: plan | accepted | superseded` 约定；不强制拆分物理目录。

## 7. 脚本整理

- 11 个 `scripts/` 文件都有明确引用，没有无人调用脚本：
  - `check.sh`：唯一验收入口；`docs/decisions.md` ADR-002。
  - `secret-scan.sh` + `secret-allowlist.txt`：check 第 1 步与 pre-commit。
  - `build-musl.sh`：README、release checklist、build-release。
  - `build-release.sh`：本地 release 产物；check/smoke 前置。
  - `repro.sh`：干净 clone 复现；check 注释与 backlog。
  - `license-report.sh` + `license-exceptions.txt`：check 第 10 步。
  - `install-hooks.sh`：启用 `.githooks`。
  - `smoke.sh`：最小健康冒烟。
  - `ui-smoke.sh`：UI 语音冒烟；release checklist/debt。
- 命名统一为 kebab-case；`test-server/` 内的脚本也统一。`docs/archive/spikes/*/run.mjs` 没有执行位，但按 README 用 `node run.mjs` 调用，不是错误。
- **保留** `build-release.sh` 与 `build-musl.sh` 两份：前者服务本地/测试，后者是发布可复现构建；合并会丢失“是否强制 musl”的语义。
- **保留** `smoke.sh` 与 `run-tests.sh`：一个是快速健康检查，一个是完整控制面/媒体验收。

## 8. 删除/归档/移动批次

> 本节只是建议顺序，未执行。每批 ≤20 个文件。

### 批次 0（安全，4 个文件，仅文档/配置）

1. `docs/decisions.md`：修正第 38 行失效引用。
2. `docs/plans/README.md`：说明 plans 可为计划或验收，增加状态约定。
3. `README.md`：补齐 `backlog/decisions/features/plans/spikes` 索引。
4. `.gitignore`：加入第 4.2 节模式。

验收：`git diff` 只涉及上述 4 个文件；链接扫描无新增失效引用。

### 批次 1（低风险，3 个文件，归档旧计划）

已归档到 `docs/archive/plans/`：

- `docs/archive/plans/2026-10-19-代码分层整理.md`
- `docs/archive/plans/2026-10-20-app-layer-migration.md`
- `docs/archive/plans/2026-10-21-frontend-component-tests.md`

同时更新 `docs/decisions.md` 和 `docs/plans/README.md` 的引用。验收：`git grep` 不再指向旧路径；历史内容未删。

### 批次 2（中风险，20 个文件，归档 spikes）

已将原顶层研究目录整体归档到 `docs/archive/spikes/`：

- `docs/archive/spikes/README.md` + 3 个实验目录共 20 个文件。
- 已修正 `docs/archive/spikes/002-sfu-tunnel/run.mjs` 的前端依赖路径为 `../../../../frontend/node_modules`，并更新 README 中从仓库根执行的命令。
- 验收：旧顶层路径 0 命中；spike 不再被生产/门禁引用；`docs/archive/spikes/README.md` 内命令可复现（或明确标注依赖本机 Playwright 缓存）。

### 批次 3（可选，1 个文件 + 引用，命名统一）

- `release/部署安装指南.md` → `release/deployment-zh.md`；同步 README、`docs/deployment.md`、`scripts/build-release.sh` 注释、`release/install.sh` 注释。
- 理由：仓库其余文档使用英文文件名；中文文件名只在发布包内保留。
- 若发布包消费者依赖旧文件名，**不做**；这是可选项，不是当前缺陷。

### 删除批次

- **空**。没有可安全删除的垃圾、重复实现或过期模板；spike 与旧计划均应归档而非删除。

## 9. 未知项

没有“用途无法确认”的文件；以下为**状态/结论未知**，不是文件用途未知：

| 文件/项 | 未知点 | 验证方法 |
|---|---|---|
| `docs/archive/spikes/002-sfu-tunnel/result.json` | 结果为“未判定”，负载下 PC 建连失败 | 按 `docs/archive/spikes/002-sfu-tunnel/README.md` 重跑 `node run.mjs`，记录成功/失败次数 |
| `docs/archive/spikes/003-rnnoise-48k/result.json` | 功能 PASS，时延未判定 | 重跑离线/负载时延测试，记录 p95 |
| `docs/archive/plans/2026-10-20-app-layer-migration.md` | 计划是否全部执行、各批次计数无验收记录 | `git log --oneline -- backend/src/app` + `git grep -c 'crate::infra::repo' backend/src/api` |
| `docs/archive/plans/2026-10-21-frontend-component-tests.md` | 计划覆盖范围与最终测试清单无验收表 | `npm test` 输出、`vitest.config.ts`、测试文件清单 |
| `docs/archive/plans/2026-10-19-代码分层整理.md` | 2 项“未验证”（编译/运行时） | `scripts/check.sh` 与 `scripts/smoke.sh` 结果 |
| `release/livekit-server` | 不在仓库（gitignore），干净机器不可直接跑集成 | 按 `docs/backlog.md` 固定版本 + sha256 或补下载脚本 |

## 10. 不做的项与理由

- **不删除 `docs/archive/spikes/`**：它们是媒体链路结论的唯一证据；删除会丢失失败/未判定结果。归档即可。
- **不合并 `docs/deployment.md` 与 `release/部署安装指南.md`**：英文/中文受众不同，合并会降低发布包可用性；应改为双语文档同步。
- **不合并 `build-release.sh` 与 `build-musl.sh`**：构建保证不同；合并会让“本地快速构建”和“可复现发布”互相污染。
- **不合并 checklists**：部署验收与发版验收触发时点不同。
- **不在本轮移动 `release/` 模板或二进制**：模板被部署文档引用；二进制本来就被 gitignore。
- **不在本轮拆 `Room.tsx` / `Admin.tsx`**：这是行为重构，不属于文件整理；现有 `MessageList`、`InvitesTab`、`UsersTab` 已按职责抽出。
- **不归档 `debt.md` 或当前 2026-10-22 计划**：它们是当前发布周期的活文档/验收证据。
- **不新增 CI、不改测试、不改依赖、不提交、不推送**：本任务只读。
- **不执行批次 0~3**：需要你批准后再按 AGENTS 执行轮逐步做，尤其批次 2 涉及相对路径。

## 11. 复核命令与结果

```text
$ git ls-files | wc -l
174

$ git ls-files --others --exclude-standard
（空；本报告写入前）

$ git status --porcelain=v1 --branch
## main

$ git status --ignored --porcelain=v1 | grep -c '^!!'
12

$ git ls-files -z | xargs -0 sha256sum | sort | uniq -d -w64
（无输出：无逐字节重复）

$ git grep -n '工程底座' docs/decisions.md README.md
（无输出：旧引用已修正）
```

未运行 `scripts/check.sh`：本任务没有代码/测试变更；上述结论来自文件、引用和 git 元数据。

## 12. 交付边界

- 本报告：`docs/plans/2026-10-22-file-organization.md`。
- 未修改其他文件；未删除任何文件；未执行批次中的移动/归档。
- 报告本身是未跟踪文件，等待你审阅后再决定是否提交。
