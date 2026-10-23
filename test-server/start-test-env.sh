#!/usr/bin/env bash
# CloudRoom 手动测试环境：启动后保持运行，供你自己操作测试（浏览器/curl）
#
# 与 run-tests.sh 的区别：
#   run-tests.sh        = 自动化验收，跑完（成功/失败/Ctrl-C）自动关闭
#   start-test-env.sh   = 挂载常驻环境，你自己慢慢测，测完再关
#
# 用法：
#   ./start-test-env.sh              # 启动，保持前台运行；Ctrl-C 关闭并清理
#   ./start-test-env.sh stop         # 停止（终端已关时用它）
#   ./start-test-env.sh status       # 查看运行状态
#   PORT_API=9000 PORT_WEB=9001 ./start-test-env.sh   # 自定义端口
#   LIVEKIT_PORT=7880 ./start-test-env.sh             # 自定义 LiveKit 端口
#   LIVEKIT_PUBLIC_URL=wss://192.168.1.5:8081 ./start-test-env.sh
#       # 手动指定 LiveKit 信令地址（一般不需要：未设置时自动跟随局域网 IP）。
#       # 浏览器不在本机时需发布 8081 与媒体端口 50000-50100/udp。
#
# LiveKit 是唯一媒体基础设施（P2P 内嵌 TURN 与 SFU 转发都依赖它），必须存在。
#
# 依赖：release/ 下的二进制（cloudroom-server / cloudroom-frontend-server /
#       livekit-server）+ 系统基础工具（curl/grep/sed/awk/cat/seq/mktemp）
set -uo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
BACKEND="$ROOT/../release/cloudroom-server"
FRONTEND_SRV="$ROOT/../release/cloudroom-frontend-server"
FRONTEND_ROOT="$ROOT/../release/frontend"
PORT_API="${PORT_API:-8080}"
PORT_WEB="${PORT_WEB:-8081}"
LIVEKIT_PORT="${LIVEKIT_PORT:-7880}"
# LiveKit 信令经前端服务器 /rtc/ 的 WSS 隧道反代（LiveKit 自身无 TLS）；
# 默认与前端同源：探测到局域网 IP 时用 wss://<LAN_IP>:<PORT_WEB>，否则 localhost。
# 探测逻辑在 LAN_IP 得出后生效，见下方 NODE_IP/LIVEKIT_URL 段。
LIVEKIT_URL="${LIVEKIT_PUBLIC_URL:-wss://localhost:$PORT_WEB}"
LIVEKIT_SECRET="${LIVEKIT_SECRET:-devsecret-devsecret-devsecret-1234}"
LIVEKIT_BIN="$ROOT/../release/livekit-server"
# 监听地址：0.0.0.0 = 所有网卡（局域网/容器外部可访问）；设 127.0.0.1 仅本机
BIND_HOST="${BIND_HOST:-0.0.0.0}"
BASE_API="http://127.0.0.1:$PORT_API"
BASE_WEB="http://127.0.0.1:$PORT_WEB"
STATE_DIR="$ROOT/.test-env"
PID_BACKEND="$STATE_DIR/backend.pid"
PID_FRONTEND="$STATE_DIR/frontend.pid"
PID_LIVEKIT="$STATE_DIR/livekit.pid"
DATA_DIR="/tmp/cloudroom-test-env"
LOG_DIR="/tmp/cloudroom-test-env/logs"

fail() { echo "错误: $1" >&2; exit 1; }

ensure_assets() {
  [ -x "$BACKEND" ]      || fail "缺少后端二进制: $BACKEND"
  [ -x "$FRONTEND_SRV" ] || fail "缺少前端服务器二进制: $FRONTEND_SRV"
  [ -f "$FRONTEND_ROOT/index.html" ] || fail "缺少前端文件: $FRONTEND_ROOT/index.html"
  [ -x "$LIVEKIT_BIN" ]  || fail "缺少 LiveKit 服务器: $LIVEKIT_BIN"
}

is_running() { # $1 = pid 文件
  [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null
}

stop_all() {
  for f in "$PID_BACKEND" "$PID_FRONTEND" "$PID_LIVEKIT"; do
    if [ -f "$f" ]; then
      pid=$(cat "$f")
      kill "$pid" 2>/dev/null
      for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
      kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
      rm -f "$f"
    fi
  done
  [ -d "$DATA_DIR" ] && rm -rf "$DATA_DIR"
}

case "${1:-start}" in
  stop)
    ensure_assets
    stop_all
    echo "测试环境已停止（进程和临时数据已清理）"
    exit 0
    ;;
  status)
    b=停; f=停; l=停
    is_running "$PID_BACKEND"  && b=运行
    is_running "$PID_FRONTEND" && f=运行
    is_running "$PID_LIVEKIT"  && l=运行
    echo "后端($BIND_HOST:$PORT_API): $b"
    echo "前端($BIND_HOST:$PORT_WEB): $f"
    echo "LiveKit(0.0.0.0:$LIVEKIT_PORT): $l"
    [ "$b" = 运行 ] && echo "  -> bootstrap 链接见 $LOG_DIR/backend.log"
    exit 0
    ;;
esac

# ------------------------------ start ------------------------------
ensure_assets
is_running "$PID_BACKEND" && fail "测试环境已在运行（后端 PID $(cat "$PID_BACKEND")）。先执行 ./start-test-env.sh stop"
curl -s --max-time 1 "$BASE_API/api/v1/health" >/dev/null 2>&1 \
  && fail "端口 $PORT_API 已被其他服务占用（非本脚本启动），请换 PORT_API"

mkdir -p "$STATE_DIR" "$LOG_DIR"
stop_all   # 清掉上次残留（数据目录重置，保证全新 bootstrap）
mkdir -p "$DATA_DIR" "$LOG_DIR"

# trap 必须在任何进程启动前设置：中途失败也要保证清理
CLEANED=0
cleanup() {
  [ "$CLEANED" = 1 ] && return
  CLEANED=1
  echo ""
  echo "==> 关闭测试环境..."
  stop_all
  echo "==> 已关闭"
}
trap cleanup EXIT INT TERM

# 探测本机局域网 IPv4（多级回退，兼容 busybox/标准 coreutils）
LAN_IP=$(hostname -I 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+\.' | grep -v '^127\.' | head -1)
[ -z "$LAN_IP" ] && LAN_IP=$(hostname -i 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+\.' | grep -v '^127\.' | head -1)
[ -z "$LAN_IP" ] && LAN_IP=$(ip -4 addr show scope global 2>/dev/null | grep -oP 'inet \K[\d.]+' | head -1)
ALLOWED=""
if [ -n "$LAN_IP" ]; then ALLOWED="allowed_origins = [\"https://$LAN_IP:$PORT_WEB\"]"; fi

# 自签名 TLS 证书（有效期 365 天；SAN 覆盖 localhost / 127.0.0.1 / 局域网 IP）
CERT_DIR="$DATA_DIR/certs"
mkdir -p "$CERT_DIR"
SAN="DNS:localhost,IP:127.0.0.1"
[ -n "$LAN_IP" ] && SAN="$SAN,IP:$LAN_IP"
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$CERT_DIR/key.pem" -out "$CERT_DIR/cert.pem" \
  -days 365 -subj "/CN=localhost" -addext "subjectAltName=$SAN" >/dev/null 2>&1 \
  || { echo "自签名证书生成失败"; exit 1; }

# LiveKit 服务器配置（固定开发 key；媒体端口 50000-50100）
# 单一节点 IP：默认 loopback（同机浏览器走 127.0.0.1 媒体端口最稳，不丢包）；
# 浏览器不在本机时自动使用局域网地址：信令 URL（LIVEKIT_URL）与 LiveKit
# 媒体候选（NODE_IP）都跟随探测到的 LAN_IP；需要手动指定时仍可显式设置
# LIVEKIT_PUBLIC_URL / LIVEKIT_NODE_IP。
if [ -z "${LIVEKIT_PUBLIC_URL:-}" ] && [ -n "$LAN_IP" ]; then
  LIVEKIT_URL="wss://$LAN_IP:$PORT_WEB"
fi
NODE_IP="${LIVEKIT_NODE_IP:-${LAN_IP:-127.0.0.1}}"
cat > "$DATA_DIR/livekit.yaml" <<EOF
port: $LIVEKIT_PORT
bind_addresses:
  - 0.0.0.0
rtc:
  node_ip: $NODE_IP
  tcp_port: $((LIVEKIT_PORT + 1))
  port_range_start: 50000
  port_range_end: 50100
  use_external_ip: false
# LiveKit 内嵌 TURN：P2P mesh 与 SFU 共用的 relay（UDP 3478；TLS 需受信证书，测试环境不开）
turn:
  enabled: true
  domain: $NODE_IP
  udp_port: 3478
  relay_range_start: 50200
  relay_range_end: 50250
keys:
  devkey: $LIVEKIT_SECRET
logging:
  level: info
EOF

cat > "$DATA_DIR/config.toml" <<EOF
[server]
listen = "$BIND_HOST:$PORT_API"
public_origin = "https://localhost:$PORT_WEB"
frontend_origin = "https://localhost:$PORT_WEB"
$ALLOWED
trusted_proxy = "127.0.0.1"
insecure_cookies = true
development = true
[data]
dir = "$DATA_DIR"
[media]
livekit_url = "$LIVEKIT_URL"
livekit_api_key = "devkey"
livekit_api_secret = "$LIVEKIT_SECRET"
p2p_voice_max_participants = 4
EOF

"$LIVEKIT_BIN" --config "$DATA_DIR/livekit.yaml" > "$LOG_DIR/livekit.log" 2>&1 &
LPID=$!
echo $LPID > "$PID_LIVEKIT"
for _ in $(seq 1 30); do
  kill -0 "$LPID" 2>/dev/null || { echo "LiveKit 启动失败，日志:"; head -5 "$LOG_DIR/livekit.log"; exit 1; }
  # LiveKit 健康检查：请求任意路径返回 400/404 即服务已监听
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 1 "http://127.0.0.1:$LIVEKIT_PORT/" 2>/dev/null)
  [ "$code" != "000" ] && break
  sleep 0.5
done
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 1 "http://127.0.0.1:$LIVEKIT_PORT/" 2>/dev/null)
[ "$code" = "000" ] && { echo "LiveKit 未就绪，日志:"; head -5 "$LOG_DIR/livekit.log"; exit 1; }

"$BACKEND" --config "$DATA_DIR/config.toml" > "$LOG_DIR/backend.log" 2>&1 &
BPID=$!
echo $BPID > "$PID_BACKEND"
for _ in $(seq 1 40); do
  # 进程已退出（如端口被占）→ 立即报错，不干等
  kill -0 "$BPID" 2>/dev/null || { echo "后端启动失败，日志:"; head -5 "$LOG_DIR/backend.log"; exit 1; }
  curl -s --max-time 1 "$BASE_API/api/v1/health" 2>/dev/null | grep -q '"ok"' && break
  sleep 0.5
done

"$FRONTEND_SRV" --host "$BIND_HOST" --port "$PORT_WEB" --root "$FRONTEND_ROOT" --target "http://127.0.0.1:$PORT_API" \
  --rtc-target "127.0.0.1:$LIVEKIT_PORT" \
  --tls-cert "$CERT_DIR/cert.pem" --tls-key "$CERT_DIR/key.pem" \
  > "$LOG_DIR/frontend.log" 2>&1 &
FPID=$!
echo $FPID > "$PID_FRONTEND"
for _ in $(seq 1 20); do
  kill -0 "$FPID" 2>/dev/null || { echo "前端服务器启动失败，日志:"; head -5 "$LOG_DIR/frontend.log"; exit 1; }
  curl -s -o /dev/null --max-time 1 "$BASE_WEB/" && break
  sleep 0.5
done
kill -0 "$FPID" 2>/dev/null || { echo "前端服务器启动失败，日志:"; head -5 "$LOG_DIR/frontend.log"; exit 1; }

BOOT=$(grep -o 'bootstrap=[A-Za-z0-9_-]*' "$LOG_DIR/backend.log" | head -1 | cut -d= -f2)

echo "=============================================="
echo " CloudRoom 测试环境已启动（保持运行，监听 $BIND_HOST）"
echo "  本机访问:   https://localhost:$PORT_WEB/ (自签证书，浏览器需手动信任)"
[ -n "$LAN_IP" ] && echo "  局域网访问: https://$LAN_IP:$PORT_WEB/ (IP: $LAN_IP)"
echo "  证书:        $CERT_DIR/cert.pem (有效期 365 天)"
echo "  后端健康:   $BASE_API/api/v1/health"
echo "  bootstrap:  http://localhost:$PORT_WEB/register?bootstrap=$BOOT"
echo "  （第一个注册的账号 = super_admin）"
echo "  LiveKit 信令: 经前端 $PORT_WEB 端口 /rtc/ WSS 隧道反代（同源，无混合内容问题）"
echo "  对外 LiveKit 地址: $LIVEKIT_URL"
echo "  LiveKit 媒体: UDP 50000-50100（浏览器需可达该端口）"
echo "  LiveKit TURN: udp 3478（relay 候选，跨 NAT 用）"
echo "  （浏览器不在本机时：发布媒体端口 50000-50100/udp，并设 LIVEKIT_NODE_IP=<本机 LAN IP>；"
echo "    信令无需发布 7880——已由前端服务器反代）"
echo " 日志:        $LOG_DIR/"
echo "关闭:         Ctrl-C（或另开终端执行 ./start-test-env.sh stop）"
echo "=============================================="
wait
