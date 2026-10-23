# CloudRoom 部署安装指南（中文）

本文档针对本文件夹（`release/`）中的部署包，从一台全新的 Linux x86_64（Debian/Ubuntu）主机开始，讲清楚怎么装、怎么配、怎么验证、怎么备份升级。

## 0. 文件夹里有什么

```
release/
├── install.sh                     # 一键安装脚本（需 root，安装 CloudRoom + LiveKit）
├── config.toml.example            # 生产配置模板（复制后修改）
├── cloudroom-server               # 后端二进制（静态编译，无任何运行依赖）
├── livekit-server                 # LiveKit 媒体服务二进制（媒体面）
├── frontend/                      # 前端静态文件（index.html + assets/）
├── nginx/cloudroom.conf.example   # Nginx 站点模板（TLS 终止 + 反代 + WSS）
├── systemd/cloudroom-server.service  # CloudRoom systemd 服务单元（加固）
└── livekit/
    ├── livekit.yaml.example           # LiveKit 配置模板（含内嵌 TURN）
    └── livekit-server.service.example # LiveKit systemd 服务模板
```

媒体架构：

- **CloudRoom** = 控制面：账号、建房申请/审批、文字频道、媒体凭证签发。后端永远只监听 `127.0.0.1`（或 Unix socket），公网入口只有 Nginx。
- **LiveKit** = 媒体面：WebRTC 信令、UDP 媒体转发、内嵌 TURN。浏览器直连 LiveKit。
- 房间创建时必须选择媒体模式：**P2P**（浏览器直连，跨 NAT 用 LiveKit 内嵌 TURN）或 **SFU**（LiveKit 转发）。房主之后切换模式必须提交申请、由管理员审批，与建房同一通道。
- **不需要 coturn**：P2P 的 relay 凭证由后端按 LiveKit 内嵌 TURN 格式签发；系统里不存在 coturn 共享密钥。

## 1. 一键安装

把整个 `release/` 文件夹上传到服务器后：

```bash
sudo ./install.sh
```

脚本完成：系统用户 `cloudroom`/`livekit`、目录、两个二进制、前端文件、两份配置、两个 systemd 服务模板。**已有配置不会被覆盖**；配置含 `CHANGE_ME` 占位符时脚本不自动启动服务——编辑好密钥/域名后手动启用：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now livekit-server
sudo systemctl enable --now cloudroom-server
```

> 没有 systemd 的环境（如容器）脚本会提示手动启动命令。

## 2. 手动安装（与脚本等价，便于理解）

```bash
# 系统用户和目录
sudo useradd --system --home /var/lib/cloudroom --shell /usr/sbin/nologin cloudroom
sudo useradd --system --home /var/lib/livekit --shell /usr/sbin/nologin livekit
sudo install -d -o cloudroom -g cloudroom -m 0750 /var/lib/cloudroom /var/lib/cloudroom/avatars
sudo install -d -o root -g root -m 0755 /etc/cloudroom
sudo install -d -o cloudroom -g cloudroom -m 0750 /run/cloudroom
sudo install -d -o www-data -g www-data -m 0755 /var/www/cloudroom

# 二进制（CloudRoom 注意权限：服务用户必须可执行）
sudo install -m 0750 cloudroom-server /usr/local/bin/cloudroom-server
sudo chown root:cloudroom /usr/local/bin/cloudroom-server
sudo install -m 0755 livekit-server /usr/local/bin/livekit-server

# 前端
sudo cp -r frontend/. /var/www/cloudroom/

# 配置
sudo cp config.toml.example /etc/cloudroom/config.toml
sudo chown root:cloudroom /etc/cloudroom/config.toml
sudo chmod 0640 /etc/cloudroom/config.toml
sudo install -m 0640 livekit/livekit.yaml.example /etc/livekit.yaml

# systemd
sudo cp systemd/cloudroom-server.service /etc/systemd/system/
sudo cp livekit/livekit-server.service.example /etc/systemd/system/livekit-server.service
sudo systemctl daemon-reload
```

## 3. 配置（`/etc/livekit.yaml` 与 `/etc/cloudroom/config.toml`）

### 3.1 LiveKit

```bash
# 生成 API key / secret（各一个，32 字节十六进制）
openssl rand -hex 32   # api_key
openssl rand -hex 32   # api_secret
sudo editor /etc/livekit.yaml   # 填入 keys 段；修改 domain；可选 TLS
```

要点：`port: 7880` 信令（WS/WSS）；`rtc.udp_port: 50000` 起 100 个 UDP 媒体端口；**内嵌 TURN 默认启用**（`turn.enabled: true` + `udp_port: 3478` + relay `50200-50250`）——P2P 与 SFU 共用这个 relay，无需 coturn。TLS 二选一：(a) `tls.cert_file/key_file` 让 LiveKit 直接提供 `wss://`；(b) 用独立 Nginx/Caddy 反代 7880（WSS 模式）。媒体 UDP 永远直连不经反代。

### 3.2 CloudRoom

```bash
sudo editor /etc/cloudroom/config.toml
```

最少必须改两项 + LiveKit 三键：

```toml
[server]
public_origin = "https://chat.example.com"   # 你的域名，必须与 Nginx 一致
frontend_origin = "https://chat.example.com"

[data]
dir = "/var/lib/cloudroom"                    # 一般不用改

[media]
livekit_url = "wss://lk.example.com"          # 浏览器可达的 LiveKit 地址（生产必须 wss://）
livekit_api_key = "<与 /etc/livekit.yaml 的 key 一致>"
livekit_api_secret = "<与 /etc/livekit.yaml 的 secret 一致>"
p2p_voice_max_participants = 4
```

常用项速查（完整键表见 `docs/configuration.md`）：

| 键 | 默认 | 说明 |
|---|---|---|
| `server.listen` | `127.0.0.1:8080` | 也可 `unix:/run/cloudroom/cloudroom.sock` |
| `auth.registration_mode` | `invite_only` | 管理员发注册邀请；`open` 为公开注册（不推荐） |
| `rooms.generated_code_length` | `7` | 自动房间号长度，**只影响之后新建的房间** |
| `rooms.dm_idle_timeout` | `1h` | 私聊任一方离线超过即删除 |
| `media.livekit_url` | 必填 | 缺失任一 LiveKit 键服务拒绝启动；生产必须 `wss://` |
| `media.p2p_voice_max_participants` | `4` | P2P 语音上限，超过返回 `sfu_required`（可申请切换 SFU） |
| `security.media_token_ttl` | `10m` | LiveKit 媒体 JWT 有效期（被踢后仍可能用到过期，建议配合移除参与者） |
| `server.insecure_cookies` | `false` | **生产必须保持 false** |

改完配置重启：`sudo systemctl restart cloudroom-server`（LiveKit 先启动）。

## 4. Nginx（TLS 终止 + 反代 + WSS）

```bash
sudo apt-get install -y nginx certbot python3-certbot-nginx
sudo cp nginx/cloudroom.conf.example /etc/nginx/sites-available/cloudroom.conf
```

编辑 `/etc/nginx/sites-available/cloudroom.conf`，把 `chat.example.com` 换成你的域名。启用并签发证书：

```bash
sudo ln -s /etc/nginx/sites-available/cloudroom.conf /etc/nginx/sites-enabled/
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx
sudo certbot --nginx -d chat.example.com     # 自动签发 + 续期
```

模板已包含：80→443 跳转、安全响应头（HSTS/CSP/nosniff）、SPA 路由回退、`/api/v1/` 反代、WebSocket Upgrade、长连接超时、头像上传体限制（3m）。**Nginx 不代理 WebRTC UDP 媒体**（P2P/LiveKit 由浏览器直连）。

## 5. 首次引导（最重要的一步）

第一次启动后，查看 bootstrap 注册链接（只显示一次，24 小时有效）：

```bash
sudo cat /var/lib/cloudroom/bootstrap-url
# 输出形如:
#   https://chat.example.com/register?bootstrap=xxxx...
```

浏览器打开该链接注册**第一个账号**——它自动成为 `super_admin`（超级管理员）。之后：

- 超级管理员在 `/admin` 面板审批建房申请、发注册邀请、管理用户
- 普通注册用户默认**不能直接建房**，需提交申请由管理员批准
- 注册默认邀请制：管理员在 Admin → invites 页生成注册邀请链接

链接丢失且尚未注册时，可在主机上重新生成：

```bash
sudo -u cloudroom cloudroom-server --config /etc/cloudroom/config.toml --rotate-bootstrap
```

已有超级管理员后该命令会拒绝执行。

## 6. 建房、媒体模式与语音

1. 首页“申请创建房间”：**必须选择媒体模式**（P2P 或 SFU），超级管理员可指定自定义房间号。
2. 管理员在 `/admin` → 申请 页审批（会看到申请类型、目标房间和媒体模式）。
3. 打开 `https://chat.example.com/<房间号>` → 发消息 → 加入语音：
   - P2P 房间：浏览器先取 `GET /api/v1/rooms/{房间号}/ice-servers` 的 LiveKit 内嵌 TURN 短期凭证，然后经 CloudRoom WSS 交换 SDP/ICE 直连。
   - SFU 房间：浏览器 `POST /api/v1/rooms/{房间号}/media-token` 取短期 JWT，直连 LiveKit 只发布/订阅麦克风。
4. **切换媒体模式**：房主在房间页“媒体模式变更”提交申请（与建房同一通道）；管理员审批后生效。房间设置里没有媒体模式选项，不能绕过审批。

## 7. 防火墙与端口

| 端口 | 协议 | 用途 |
|---|---|---|
| 80 / 443 | tcp | Nginx：HTTPS 跳转/ACME / 前端 / API / WSS |
| 7880 | tcp | LiveKit 信令（WS/WSS，TLS 或反代后只对反代开放） |
| 7881 | tcp | LiveKit RTC TCP 回退 |
| 50000-50100 | udp | LiveKit 媒体直连（不经 Nginx） |
| 3478 | udp | LiveKit 内嵌 TURN（P2P 与 SFU 共用 relay） |
| 50200-50250 | udp | TURN relay 数据 |

CloudRoom 后端只监听回环，公网无需开放 8080。

## 8. 验证清单

```bash
# 健康检查（应返回 {"status":"ok",...}，含 sqlite 与 livekit）
curl -fsS https://chat.example.com/api/v1/health

# 公共配置（应看到 voice=true、voice_modes=[p2p,sfu]、screen_share/media_playback=false）
curl -fsS https://chat.example.com/api/v1/config/public

# 页面和 SPA 路由（房间 URL 形态 /abc1234 应返回前端页面）
curl -s -o /dev/null -w '%{http_code}\n' https://chat.example.com/
curl -s -o /dev/null -w '%{http_code}\n' https://chat.example.com/abc1234

# 后端必须只监听回环（公网 IP 的 8080 应无法连接）
```

测试环境：`test-server/start-test-env.sh` 自动拉起 CloudRoom + 前端服务器 + LiveKit（固定 devkey、内嵌 TURN 3478），`test-server/sfu-verify.sh` 一键验证两种媒体模式的凭证与 JWT。

## 9. 备份、升级、恢复

备份（在线安全备份，服务运行中即可执行）：

```bash
sudo -u cloudroom mkdir -p /var/lib/cloudroom/backups
sudo -u cloudroom sqlite3 /var/lib/cloudroom/cloudroom.db \
  ".backup '/var/lib/cloudroom/backups/cloudroom-$(date +%F).db'"
```

> SQLite 不加密落盘：备份务必加密（borg/restic/age），生产建议主机磁盘加密。

升级：

```bash
sudo install -m 0750 新版本cloudroom-server /usr/local/bin/cloudroom-server
sudo chown root:cloudroom /usr/local/bin/cloudroom-server
# 先备份，再重启；迁移自动执行，失败会中止启动且不破坏旧库
sudo systemctl restart cloudroom-server
```

本版本的 `2026-08-08-0004-explicit-room-media-mode` 迁移会把旧 `auto` 房间保守转为 `sfu`，并扩展申请表（申请类型/目标房间/媒体模式）。旧客户端未带 `media_mode` 或通过 settings 直改模式会收到 422，需与新版一起升级。

恢复：停服务 → 用备份文件替换 `cloudroom.db` → 启动。

## 10. 常见问题

| 症状 | 处理 |
|---|---|
| Nginx 502 | `systemctl status cloudroom-server`；二进制权限必须 `root:cloudroom 0750` |
| 登录后立刻掉线（cookie 未设置） | 确认 `X-Forwarded-Proto: https` 已透传、`trusted_proxy` 正确、`insecure_cookies=false` |
| WSS 频繁断开 | Nginx 的 `proxy_read_timeout`/`proxy_send_timeout` ≥ 3600s（模板已带） |
| 后端拒绝启动 | `[media]` 三键缺失，或生产环境 `livekit_url` 不是 `wss://` |
| 公网语音不通 | 检查 LiveKit 端口（7880/7881/50000-50100/udp）与内嵌 TURN 3478/udp + 50200-50250/udp 是否开放 |
| 加入语音报 `sfu_required` | 房间是 P2P 且超过 `p2p_voice_max_participants`；申请切换 SFU 模式 |
| 报 `media_mode_mismatch` | 在 SFU 房间调 `ice-servers` 或在 P2P 房间调 `media-token`（接口与模式绑定） |
| 头像上传失败 | 仅 PNG/JPEG/WebP、≤2 MiB、≤1024×1024 |
| 改房间号长度没生效 | 配置只影响**之后新建**的房间，旧房间号保持不变（设计如此） |
| 普通用户能看到 Admin 入口 | 不会：非管理员看不到入口，访问 `/admin` 或管理 API 均返回 403 |
