# CloudRoom 测试环境（单可执行文件 + 自签 HTTPS）

中文界面。一个**单文件静态编译**的前端服务器，用来在没有 Nginx / Node 的环境里快速联调 CloudRoom 前端与后端。

```
test-server/
├── cloudroom-frontend-server   # 单可执行文件（静态 musl，零依赖，~3 MB）
├── start-test-env.sh           # 一键测试环境（后端 + 前端服务器 + LiveKit）
├── run-tests.sh                # 零依赖自动化全流程测试
├── sfu-verify.sh               # LiveKit 双模式集成验证
├── ui-voice-check.mjs          # 真实 Chrome/CDP 的 SFU 语音 UI 冒烟
└── README.md
```

## 它能做什么

- 托管前端静态文件（默认 `./frontend` 目录，即 `npm run build` 的产物）
- 把 `/api/v1/*` 反代到后端 `cloudroom-server`（补 `X-Forwarded-*` 头）
- WebSocket 升级 = 原始 TCP 隧道（聊天 / P2P 语音信令完整可用）
- SPA 路由回退（`/abc1234` 这类房间 URL 正确返回前端页面）

## 一键测试环境（推荐）

```bash
./start-test-env.sh          # 启动：后端 + 前端服务器 + LiveKit + 自签证书，保持运行
./start-test-env.sh stop     # 关闭（或 Ctrl-C）
./start-test-env.sh status   # 查看状态
```

启动后输出：

```
 本机访问:   https://localhost:8081/ (自签证书，浏览器需手动信任)
 局域网访问: https://<本机IP>:8081/
```

- 自动生成 365 天自签证书（SAN 含 localhost / 127.0.0.1 / 本机局域网 IP）
- 监听 `0.0.0.0`（全 IP，`BIND_HOST=127.0.0.1` 可改回仅本机）
- **HTTPS 是 WebRTC 语音的前置条件**（`getUserMedia` 要求安全上下文）
- **LiveKit 是唯一媒体基础设施**（P2P 内嵌 TURN 与 SFU 转发都依赖它），`start-test-env.sh` 会强制启动 `release/livekit-server`，不存在 `LIVEKIT_ENABLED=0` 选项
- **LiveKit 信令经前端服务器 `/rtc/` 的 WSS 原始隧道反代**（LiveKit 自身不提供 TLS，与生产 Nginx 反代模式一致）：https 页面连的是同源 `wss://<host>:8081`，无混合内容问题，也只需信任前端一张自签证书
- LiveKit 媒体（UDP 50000-50100）仍由浏览器直连：浏览器不在本机时需发布媒体端口；信令地址与媒体候选地址会**自动跟随探测到的局域网 IP**（`wss://<LAN_IP>:8081`），无需手动设置
- 第一个注册账号 = super_admin（引导链接见启动输出）
- 媒体模式：创建房间时选择 P2P 或 SFU；P2P 房间跨 NAT 用 LiveKit 内嵌 TURN（UDP 3478）

环境变量（均可省略，脚本自动推断）：`PORT_API`、`PORT_WEB`、`LIVEKIT_PORT`、`LIVEKIT_PUBLIC_URL`（手动指定 LiveKit 信令地址）、`LIVEKIT_NODE_IP`（手动指定媒体候选地址）、`LIVEKIT_SECRET`、`BIND_HOST`。

## 非局域网（公网 / 云主机 / 远程服务器）

自动探测到的是服务器私网 IP（如 `172.x`、`10.x`、`192.168.x`），公网浏览器够不着，必须手动指定浏览器可达的公网地址：

```bash
# 1. 服务器放行端口：8081/tcp 与 50000-50100/udp（安全组/防火墙/NAT 映射）
# 2. 启动时指定浏览器可达地址（域名与地址栏保持一致）：
LIVEKIT_PUBLIC_URL=wss://<公网IP或域名>:8081 \
LIVEKIT_NODE_IP=<公网IP> \
./start-test-env.sh
# 3. 浏览器访问 https://<公网IP或域名>:8081/（首次信任自签证书）
```

- `LIVEKIT_PUBLIC_URL` 必须与浏览器地址栏完全一致（用域名就写域名）。
- `LIVEKIT_NODE_IP` 填浏览器可路由的公网 IP（云主机填弹性公网 IP），媒体候选才会公布正确地址。
- **不要把后端 8080 或 LiveKit 7880 暴露到公网**：测试环境使用固定开发密钥和明文 API；只放行前端 `8081/tcp` 与媒体 `50000-50100/udp`（TURN 另加 `3478/udp`、`50200-50250/udp`）。
- **媒体 UDP 50000-50100 必须直通**：SSH 端口转发只支持 TCP，无法转发 UDP，因此仅用 `ssh -L` 访问时语音媒体不通。
- 自签证书仅适合临时测试；公网长期使用请按 `docs/deployment.md` 部署 Nginx + 正式证书。

## 手动启动（可选）

```bash
./cloudroom-frontend-server \
    --host 0.0.0.0 --port 8081 \
    --root ../release/frontend \
    --target http://127.0.0.1:8080 \
    --rtc-target 127.0.0.1:7880 \
    --tls-cert cert.pem --tls-key key.pem    # 不传则纯 http
```

浏览器打开 `https://localhost:8081/`：注册 → 申请建房（选择 P2P/SFU）→ 管理后台审批 → 进房间 → 发消息 → 加入语音。

| 参数 | 默认 | 说明 |
|---|---|---|
| `--port` | `8081` | 监听端口（只绑定 127.0.0.1） |
| `--root` | `frontend` | 前端构建产物目录（必须含 index.html） |
| `--target` | `http://127.0.0.1:8080` | 后端地址 |
| `--host` | `127.0.0.1` | 监听地址（`0.0.0.0` = 全 IP） |
| `--tls-cert` / `--tls-key` | 无 | PEM 证书/私钥，提供后以 HTTPS/WSS 运行 |

## 后端配套配置

```toml
[server]
public_origin = "http://localhost:8081"
frontend_origin = "http://localhost:8081"
insecure_cookies = true     # 仅本地 http 测试；生产必须 false

[media]
livekit_url = "wss://127.0.0.1:7880"
livekit_api_key = "devkey"
livekit_api_secret = "<替换为随机 secret：openssl rand -hex 32>"
```

`start-test-env.sh` 会自动生成这份配置；后端要求 LiveKit 三键必填。

## 自动化测试

```bash
./run-tests.sh          # 自动启动/清理 后端 + 前端服务器 + LiveKit，跑完整流程
./sfu-verify.sh         # 在 start-test-env.sh 环境上验证两种媒体模式（需 BASE_WEB=https://localhost:8081）
```

### UI 语音冒烟（`ui-voice-check.mjs`）

本地一键入口（自动启动/清理测试环境、创建 SFU 用户与房间）：

```bash
./scripts/ui-smoke.sh
```

成功 exit 0；失败（Chrome/服务/断言）exit 非 0。该脚本不接入 `scripts/check.sh` 或 CI。

直接用 `ui-voice-check.mjs` 时仍需准备运行中的测试环境、用户和 SFU 房间：

用 Node 22 内置 `fetch` / `WebSocket` 驱动真实 Chrome，验证 SFU 房间的“加入语音 → 发布 → 离开”完整链路。不新增 npm 依赖、不下载浏览器；未接入 `scripts/check.sh`（需要浏览器与常驻服务）。

前置条件：

- 测试环境已启动，目标用户与 **SFU** 房间已存在。
- Chrome 可执行文件：`CHROME_PATH`，默认
  `/home/agent/.cache/ms-playwright/chromium-1248/chrome-linux64/chrome`。
- Node 22（使用内置 `fetch` / `WebSocket`）。
- 若环境默认把 LiveKit 信令指向 `172.17.0.1`，浏览器可能被沙箱策略拦截并返回 403；启动测试环境时改为本机地址：

```bash
LIVEKIT_PUBLIC_URL=wss://localhost:8081 \
LIVEKIT_NODE_IP=127.0.0.1 \
./test-server/start-test-env.sh
```

用法：

```bash
node test-server/ui-voice-check.mjs \
  --base-url https://localhost:8081 \
  --user alice \
  --pass 'correct-horse-battery' \
  --room voicecheck \
  --timeout 40
```

| 参数 | 默认 | 说明 |
|---|---|---|
| `--base-url` | `https://localhost:8081` | 前端地址 |
| `--user` / `--pass` | 必填 | 登录账号 |
| `--room` | 必填 | 已存在的 SFU 房间 code |
| `--timeout` | `40` | 整体超时（秒） |
| `--headless` | 开 | 传 `--headless=false` 可关闭 headless |

脚本启动 Chrome 时自动带上 `--ignore-certificate-errors`、`--use-fake-device-for-media-stream`、`--use-fake-ui-for-media-stream`、`--autoplay-policy=no-user-gesture-required`。fake-ui 自动授权麦克风，不需要真实设备或手动点权限弹窗。

检查项：登录并进入 `/{room}`、`POST /api/v1/rooms/{room}/media-token` 为 200、`/rtc/` WebSocket 为 101、`RTCPeerConnection=connected`、至少一次 audio sendonly / outbound RTP、离开后 PC closed 且 LiveKit WS 关闭、console error 为 0（仅显式过滤登录前 `/api/v1/me` 的 401 探测）。失败时打印首个失败断言、相关请求/WS 和 console 错误，退出码 1；成功退出码 0。

## 与 Nginx 生产环境的区别

| | 测试服务器 | Nginx 生产 |
|---|---|---|
| TLS | 自签证书（浏览器需手动信任一次） | certbot 正式证书 |
| 安全响应头 | 无 | CSP / HSTS / nosniff 全套 |
| 适用 | 本地开发、验收、内网 | 公网部署 |

安全头、证书、限流不属于本工具职责；生产请用 `release/nginx/cloudroom.conf.example`。

## 重新编译（需要 Rust + musl 工具链）

```bash
cd ../frontend-server
cargo build --release --target x86_64-unknown-linux-musl
# 产物: target/x86_64-unknown-linux-musl/release/cloudroom-frontend-server
```
