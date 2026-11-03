#!/usr/bin/env bash
# CloudRoom LiveKit 语音集成验证：在 start-test-env.sh 已启动的环境上运行。
# 同时验证 p2p（LiveKit 内嵌 TURN 凭证 + ice-servers）与 sfu（media-token JWT）两条路径；
# 不依赖、不验证独立 coturn。
# 依赖：curl/grep/cut/base64/python3（仅用于解码 JWT 展示，缺失时跳过该步）
#
# 用法：./sfu-verify.sh            # 默认 https://localhost:8081，LiveKit 127.0.0.1:7880
#       BASE_WEB=https://host:port ./sfu-verify.sh
set -uo pipefail

BASE_WEB="${BASE_WEB:-https://localhost:8081}"
LIVEKIT_HTTP="${LIVEKIT_HTTP:-http://127.0.0.1:7880}"
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "  FAIL  $1"; }
check() { # $1=实际 $2=期望 $3=描述
  [ "$1" = "$2" ] && ok "$3" || bad "$3 (期望 $2, 实际 $1)"
}

echo "[1] LiveKit 服务健康"
CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "$LIVEKIT_HTTP/")
[ "$CODE" != "000" ] && ok "LiveKit HTTP 端口响应 ($CODE)" || bad "LiveKit 未监听 $LIVEKIT_HTTP"

echo "[2] 后端功能标志"
PUB=$(curl -sk "$BASE_WEB/api/v1/config/public")
echo "$PUB" | grep -q '"voice":true' && ok "features.voice=true" || bad "features.voice 缺失: $PUB"
echo "$PUB" | grep -q '"screen_share":true' && ok "features.screen_share=true" || bad "features.screen_share 异常"
echo "$PUB" | grep -q '"media_playback":false' && ok "features.media_playback=false" || bad "features.media_playback 异常"
echo "$PUB" | grep -q '"voice_modes":\["p2p","sfu"\]' && ok "voice_modes=[p2p,sfu]" || bad "voice_modes 缺失"

echo "[3] 建房（p2p）"
BOOT_URL=$(cat /tmp/cloudroom-test-env/bootstrap-url 2>/dev/null || true)
BOOT=${BOOT_URL##*bootstrap=}
JAR=$(mktemp); trap 'rm -f "$JAR"' EXIT
R=$(curl -sk -c "$JAR" -X POST "$BASE_WEB/api/v1/auth/register" -H 'Content-Type: application/json' \
  -d "{\"username\":\"lk_verify_$RANDOM\",\"password\":\"correct-horse-battery\",\"token\":\"$BOOT\"}")
echo "$R" | grep -q '"role":"super_admin"' || { bad "bootstrap 注册"; exit 1; }
CSRF=$(curl -sk -b "$JAR" "$BASE_WEB/api/v1/me" | grep -o '"csrf_token":"[^"]*"' | cut -d'"' -f4)
R=$(curl -sk -b "$JAR" -X POST "$BASE_WEB/api/v1/room-requests" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"kind":"create_room","note":"lk-verify","desired_code":"880","media_mode":"p2p"}')
[ "$(echo "$R" | grep -o '"code":"880"' | head -1)" = '"code":"880"' ] && ok "免审批建房 880 (p2p)" || bad "建房失败: $R"

echo "[4] p2p 路径：ice-servers 签发 LiveKit 内嵌 TURN 凭证"
R=$(curl -sk -b "$JAR" "$BASE_WEB/api/v1/rooms/880/ice-servers")
echo "$R" | grep -q '"turn:' && ok "p2p ice-servers 返回 LiveKit TURN url" || bad "ice-servers 无 TURN url: $R"
U=$(echo "$R" | grep -o '"username":"[^"]*"' | cut -d'"' -f4)
[ ${#U} -ge 40 ] && ok "TURN username 已签发 (${#U} 字符)" || bad "username 过短"
echo "$R" | grep -q '"ttl":300' && ok "TURN 凭证 TTL=300s" || bad "TTL 异常"

echo "[5] p2p 房间拒绝 media-token"
S=$(curl -sk -b "$JAR" -X POST "$BASE_WEB/api/v1/rooms/880/media-token" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}' -o /dev/null -w '%{http_code}')
check "$S" "409" "p2p 房间 media-token → 409 media_mode_mismatch"

echo "[6] 模式变更（super_admin 免审批）→ sfu"
R=$(curl -sk -b "$JAR" -X POST "$BASE_WEB/api/v1/room-requests" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"kind":"change_media_mode","room_code":"880","media_mode":"sfu"}')
echo "$R" | grep -q '"auto_approved":true' && ok "super_admin 模式变更免审批" || bad "模式变更失败: $R"
echo "$R" | grep -q '"media_mode":"sfu"' && ok "房间已切换为 sfu" || bad "切换结果异常: $R"

echo "[7] sfu 路径：media-token 签发 LiveKit JWT"
R=$(curl -sk -b "$JAR" -X POST "$BASE_WEB/api/v1/rooms/880/media-token" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
URL=$(echo "$R" | grep -o '"url":"[^"]*"' | cut -d'"' -f4)
[ -n "$URL" ] && ok "token url=$URL" || bad "无 url: $R"
JWT=$(echo "$R" | grep -o '"token":"[^"]*"' | cut -d'"' -f4)
[ -n "$JWT" ] && ok "token 已签发 (JWT 三段: $(echo "$JWT" | awk -F. '{print NF}') 段)" || bad "无 token"
if command -v python3 > /dev/null; then
  PAY=$(echo "$JWT" | cut -d. -f2 | tr '_-' '/+')
  D=$(echo "$PAY" | base64 -d 2>/dev/null)
  echo "$D" | grep -q '"room":"880"' && ok "JWT room=880" || bad "JWT room 不符: $D"
  echo "$D" | grep -q '"canPublish":true' && ok "JWT canPublish=true" || bad "JWT canPublish 缺失"
  echo "$D" | grep -q '"canSubscribe":true' && ok "JWT canSubscribe=true" || bad "JWT canSubscribe 缺失"
  echo "$D" | grep -q '"canPublishSources":\["microphone"\]' && ok "JWT 只允许 microphone source" || bad "JWT 未限制 source: $D"
fi

echo "[8] sfu 房间拒绝 ice-servers"
S=$(curl -sk -b "$JAR" "$BASE_WEB/api/v1/rooms/880/ice-servers" -o /dev/null -w '%{http_code}')
check "$S" "409" "sfu 房间 ice-servers → 409 media_mode_mismatch"

echo ""
echo "结果: $PASS 通过, $FAIL 失败"
[ "$FAIL" = 0 ] || exit 1
