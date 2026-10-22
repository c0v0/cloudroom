#!/usr/bin/env bash
# 本地一条命令 UI 语音冒烟：启动测试环境、创建 SFU 用户/房间、运行 Chrome CDP 检查。
# 不接入 scripts/check.sh / CI；脚本结束（成功或失败）都会关闭测试环境。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BASE_URL="${UI_BASE_URL:-https://localhost:8081}"
ROOM="ui-smoke"
USER="uismoke$$"
PASS="correct-horse-battery"
START_LOG="$(mktemp /tmp/cloudroom-ui-smoke-start.XXXXXX)"
JAR="$(mktemp /tmp/cloudroom-ui-smoke-cookies.XXXXXX)"
START_PID=""

cleanup() {
  if [ -n "$START_PID" ]; then
    kill "$START_PID" 2>/dev/null || true
    wait "$START_PID" 2>/dev/null || true
  fi
  ( cd test-server && ./start-test-env.sh stop >/dev/null 2>&1 || true )
  rm -f "$START_LOG" "$JAR"
}
trap cleanup EXIT INT TERM

for bin in release/cloudroom-server release/cloudroom-frontend-server \
  release/livekit-server release/frontend/index.html; do
  [ -e "$bin" ] || { echo "缺少 $bin；先运行 scripts/build-musl.sh all 并准备 livekit-server" >&2; exit 1; }
done
command -v node >/dev/null || { echo "缺少 node（需要 Node 22）" >&2; exit 1; }

# 避免与已有环境冲突；本脚本只操作自己的常驻测试环境。
( cd test-server && ./start-test-env.sh stop >/dev/null 2>&1 || true )
echo "启动 test-server（约 5–10 秒）…"
( LIVEKIT_PUBLIC_URL=wss://localhost:8081 LIVEKIT_NODE_IP=127.0.0.1 \
  test-server/start-test-env.sh >"$START_LOG" 2>&1 ) &
START_PID=$!

READY=0
for _ in $(seq 1 90); do
  if curl -sk --max-time 1 "$BASE_URL/api/v1/health" 2>/dev/null | grep -q '"status":"ok"'; then
    READY=1
    break
  fi
  sleep 0.5
done
if [ "$READY" != 1 ]; then
  echo "test-server 未就绪，启动日志：" >&2
  tail -30 "$START_LOG" >&2
  exit 1
fi

BOOT=$(grep -o 'bootstrap=[A-Za-z0-9_-]*' /tmp/cloudroom-test-env/logs/backend.log 2>/dev/null | head -1 | cut -d= -f2)
[ -n "$BOOT" ] || { echo "未找到 bootstrap token" >&2; exit 1; }

REGISTER=$(curl -sk -c "$JAR" -X POST "$BASE_URL/api/v1/auth/register" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"$USER\",\"password\":\"$PASS\",\"token\":\"$BOOT\"}")
echo "$REGISTER" | grep -q '"role":"super_admin"' || {
  echo "bootstrap 注册失败：$REGISTER" >&2
  exit 1
}

CSRF=$(curl -sk -b "$JAR" "$BASE_URL/api/v1/me" | sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
[ -n "$CSRF" ] || { echo "无法获取 CSRF token" >&2; exit 1; }

ROOM_REQ=$(curl -sk -b "$JAR" -X POST "$BASE_URL/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' \
  -d "{\"kind\":\"create_room\",\"note\":\"ui smoke\",\"desired_code\":\"$ROOM\",\"media_mode\":\"sfu\"}")
echo "$ROOM_REQ" | grep -q "\"code\":\"$ROOM\"" || {
  echo "创建 SFU 房间失败：$ROOM_REQ" >&2
  exit 1
}

echo "运行 ui-voice-check（用户 $USER，房间 $ROOM）…"
node test-server/ui-voice-check.mjs \
  --base-url "$BASE_URL" --user "$USER" --pass "$PASS" --room "$ROOM" --timeout 60
