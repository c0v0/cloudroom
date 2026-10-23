#!/usr/bin/env bash
# 冒烟测试：用户可见入口的最小闭环（不需要 LiveKit、不开端口外网）。
#
# 现在覆盖：release/cloudroom-server 启动 → GET /api/v1/health = status:ok
#          → GET /api/v1/config/public = 200 → 关停清理。
# 没有 release/cloudroom-server 时打印 SKIP 并返回 0（骨架状态，方便先接入门禁）。
# 全流程（建房/审批/邀请/消息/WS/媒体凭证、P2P 与 SFU）由 test-server/run-tests.sh 覆盖，
# 由 scripts/check.sh 调度。
#
# 用法：scripts/smoke.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/release/cloudroom-server"

if [ ! -x "$BIN" ]; then
  echo "[smoke] SKIP：缺少 $BIN（先跑 scripts/build-release.sh）"
  exit 0
fi

PORT="${SMOKE_PORT:-}"
if [ -z "$PORT" ] && command -v python3 >/dev/null 2>&1; then
  PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()' 2>/dev/null || true)"
fi
PORT="${PORT:-18765}"
BASE="http://127.0.0.1:$PORT"

TMP="$(mktemp -d /tmp/cloudroom-smoke.XXXXXX)"
PID=""
cleanup() {
  [ -n "$PID" ] && kill "$PID" 2>/dev/null
  [ -n "$PID" ] && wait "$PID" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

# LiveKit 密码只是启动校验用的临时值（真实密钥在部署者的 config.toml，永不入库）
LK_SECRET="smoke-$(date +%s)-$RANDOM$RANDOM"
cat > "$TMP/config.toml" <<EOF
[server]
listen = "127.0.0.1:$PORT"
public_origin = "$BASE"
frontend_origin = "$BASE"
trusted_proxy = "127.0.0.1"
insecure_cookies = true
development = true

[data]
dir = "$TMP/data"

[media]
livekit_url = "wss://127.0.0.1:7880"
livekit_api_key = "smoke-key"
livekit_api_secret = "$LK_SECRET"
EOF

"$BIN" --config "$TMP/config.toml" > "$TMP/backend.log" 2>&1 &
PID=$!

FAIL=0
ok()  { echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }

echo "[smoke] 启动 $BIN（port $PORT）"
UP=0
for _ in $(seq 1 40); do
  kill -0 "$PID" 2>/dev/null || break
  if curl -s --max-time 1 "$BASE/api/v1/health" 2>/dev/null | grep -q '"status":"ok"'; then
    UP=1; break
  fi
  sleep 0.25
done

if [ "$UP" = 1 ]; then
  ok "健康检查 /api/v1/health"
else
  bad "后端未就绪或已退出，日志尾部：$(tail -3 "$TMP/backend.log" | tr '\n' ' ')"
fi

CODE="$(curl -s -o "$TMP/public.json" -w '%{http_code}' --max-time 2 "$BASE/api/v1/config/public" 2>/dev/null || echo 000)"
if [ "$CODE" = 200 ]; then
  ok "公共配置 /api/v1/config/public = 200"
else
  bad "公共配置返回 $CODE"
fi

if [ "$FAIL" -eq 0 ]; then
  echo "[smoke] PASS"
  exit 0
fi
echo "[smoke] FAIL（$FAIL 项）"
exit 1
