# CloudRoom

自托管实时房间系统：身份、房间授权、建房审批、文字频道和实时语音。前端是独立的 React/TypeScript + Vite 单页应用，后端是单个静态链接的 Rust 可执行文件（`cloudroom-server`，SQLite 存储），实时媒体统一由 [LiveKit](https://livekit.io) 提供。

## 当前范围

- 文字控制面：账号（bootstrap 首个超级管理员、邀请制注册、访客升级）、建房申请/审批、房间 URL 与一次性邀请、文字消息、昵称、禁言、审计、过期清理。
- 语音（SFU 房间支持屏幕共享；不含摄像头视频与文件播放）：
  - **P2P 房间**：浏览器直连 + CloudRoom WSS 中继 SDP/ICE；跨 NAT 使用 LiveKit 内嵌 TURN 的短期凭证。
  - **SFU 房间**：通过 CloudRoom 签发短期 LiveKit JWT，`livekit-client` 只发布/订阅麦克风。
- 语音体验：
  - 说话动效：成员头像绿圈随音量变化（独立 AnalyserNode 逐流检测）。
  - 成员列表展示自己/他人的麦克风、扬声器静音状态（WS `media_state` 广播）。
  - 用户侧声音设置（个人设置弹窗，偏好存本地，语音中修改即时生效）：降噪三态（关 / WebRTC 内置 / rnnoise WASM 本地降噪 48kHz）、回声消除开关、自动增益开关。
  - 音频码率设置（SFU 生效）：档位与房主权限由配置 `voice.bitrate_tiers` 控制（默认 ≤128 任何房主、160/192 管理员房主、256–510（Opus 上限）超管房主；档位含 48 语音/音乐分界）。
  - 房间级全员禁言（仅房主可开关）：禁言中所有成员（含房主）需申请发言，房主批准后获发言权；房主申请自动通过；房主可单独取消任一获权用户的发言权限。
  - 入会/离会提示音（Web Audio 合成）。
  - 关闭标签页（断连）识别：8 秒延迟窗口确认离线后播报"xxx 离开了房间"，成员关系保留。
  - 高频进出房间限流（加入/离开每分钟各 20 次，可配置）。
- 创建房间时必须选择 `p2p` 或 `sfu`；房主后续切换媒体模式必须提交与建房相同的申请，由管理员审批。独立 coturn 不被安装、配置或使用。

## 组件

| 组件 | 说明 |
|---|---|
| `backend/` | Rust/Axum 单体：HTTP API + WebSocket 控制面 + SQLite。静态 musl 构建，无运行依赖。 |
| `frontend/` | React 18 + Vite + `livekit-client`：房间页、语音适配器（`useP2PVoice` / `useLiveKitVoice` / `useVoiceSession`）、控制面 socket（`useRoomSocket`）。 |
| `frontend-server/` | 可选的单文件静态服务器 + `/api/v1/` 反代 + WSS 隧道（测试/内网联调用）。 |
| `test-server/` | 一键测试环境与自动化验收脚本（自动拉起后端、前端服务器与 LiveKit）。 |
| `release/` | 发布包：二进制、前端构建产物、Nginx/systemd/LiveKit 配置模板、安装脚本与中文部署指南。 |

## 目录速览

- `docs/architecture.md` — 架构与媒体路径（P2P / SFU）、审批时序、未来能力边界
- `docs/api.md` — API / WebSocket 契约
- `docs/configuration.md` — 配置键参考
- `docs/deployment.md` — 英文部署指南
- `docs/deploy-checklist.md` — TLS/Nginx/systemd/安装/历史数据/真实设备验收
- `docs/release-checklist.md` — 每次发版的固定步骤
- `docs/security.md` — 威胁模型与安全边界
- `release/deployment-zh.md` — 中文部署安装指南

## 文档索引

- [docs/features.md](docs/features.md) — 用户可见能力、入口和验证命令
- [docs/backlog.md](docs/backlog.md) — 范围外待办与已完成的工程项
- [docs/decisions.md](docs/decisions.md) — 对外变化、验收基线和架构决策（ADR）
- [docs/plans/README.md](docs/plans/README.md) — 当前计划、决策与验收记录索引
- [docs/archive/spikes/README.md](docs/archive/spikes/README.md) — 历史媒体可行性实验与结论

## 开发

### 本地开发构建（非发布产物）

后端（需要 Rust + musl target）：

```bash
cd backend
cargo test
cargo build --release --target x86_64-unknown-linux-musl --bin cloudroom-server
# 这是本地开发构建，不是发布产物：生成非 PIE 静态二进制，
# 与 scripts/build-musl.sh 的 static-pie 产物哈希不同。
```

前端：

```bash
cd frontend
npm ci
npm run build      # 产物: dist/（本地开发）
```

### 发布构建

发布包统一使用：

```bash
./scripts/build-musl.sh all
```

产物位于 `release/`：`cloudroom-server` 与 `cloudroom-frontend-server` 为
static-pie + stripped（无 `NEEDED`/`INTERP`），前端为 `release/frontend/`。
不要用上面的 `cargo build` 命令生成发布包；两者的链接形态和哈希不同。

## 测试

先完成上面的发布构建，并准备 `release/livekit-server`（测试环境的 LiveKit 依赖）。

```bash
# 方式一：常驻测试环境（终端 A；后端 + 前端服务器 + LiveKit，自签 HTTPS）
cd test-server
./start-test-env.sh
```

另开终端 B 做集成验证（两种媒体模式、JWT/TURN 凭证、审批流程）：

```bash
cd test-server
BASE_WEB=https://localhost:8081 ./sfu-verify.sh
```

验证完在任意终端停止常驻环境：

```bash
cd test-server
./start-test-env.sh stop
```

零依赖全流程测试会自行启动/清理后端、前端服务器与 LiveKit；不要与常驻环境同时运行：

```bash
cd test-server
./run-tests.sh
```

一键验收（密钥扫描、Rust fmt/clippy/测试、前端类型检查/测试/构建、端到端集成、依赖检查）：

```bash
./scripts/check.sh
```

## 生产部署

见 `docs/deployment.md`（英文）与 `release/deployment-zh.md`（中文）：Nginx 终止 TLS 并代理控制面，CloudRoom 只监听 loopback，LiveKit 提供媒体面（Nginx `/rtc` WSS 信令、7881/TCP 回退、50000-50100/UDP，内嵌 TURN 3478）。

## 未来能力边界

- **屏幕分享（P2P）**：SFU 房间已支持 LiveKit screen-share track；P2P 房间仍只发布麦克风，需要扩展 video track 与信令。
- **多媒体文件在线播放**：采用对象存储的鉴权资源与签名 URL，LiveKit data message / WSS 只同步播放状态；不经 P2P DataChannel 搬运大文件。
- 文件播放在 `GET /api/v1/config/public` 的 `features` 中固定为 `false`，未实现前不提供入口。
