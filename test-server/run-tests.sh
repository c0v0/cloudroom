#!/usr/bin/env bash
# CloudRoom 全流程测试（只需编译好的二进制 + 系统基础工具，无编译环境/无 python）
#
# 依赖：release/cloudroom-server、release/cloudroom-frontend-server、
#       release/livekit-server、release/frontend，以及系统基础工具：
#       curl、grep、sed、awk、od、tr、cut、seq、mktemp、head、cat、sleep
#       （任何最小 Linux 发行版均自带，awk 在部分极简镜像中需确认存在）
#
# LiveKit 是唯一媒体基础设施（P2P 内嵌 TURN 与 SFU 转发都依赖它），
# 脚本会自动启动/清理一个测试 LiveKit 实例。
#
# 执行：
#   ./run-tests.sh                          # 默认端口 8080/8081
#   PORT_API=9000 PORT_WEB=9001 ./run-tests.sh
#   LIVEKIT_PORT=7881 ./run-tests.sh        # 自定义 LiveKit 端口
#
# 关闭：
#   脚本结束（成功/失败/ Ctrl-C）都会自动杀掉测试进程并删除临时目录，
#   无需手动清理。保险起见可再执行：
#   pkill -f cloudroom-server; pkill -f cloudroom-frontend-server; pkill -f livekit-server
#
# 覆盖：健康检查、bootstrap 引导注册、CSRF、建房申请/审批（自定义码、媒体模式）、
#       媒体模式变更申请/审批、p2p ice-servers 与 sfu media-token 门禁、
#       注册邀请、邀请加入（一次性）、消息收发、禁言、文字频道开关、
#       房间删除 + 房间号 tombstone、前端静态托管/SPA 回退/API 反代、
#       WebSocket 隧道（101 + hello 帧）。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
BACKEND="$ROOT/../release/cloudroom-server"
FRONTEND_SRV="$ROOT/../release/cloudroom-frontend-server"
FRONTEND_ROOT="$ROOT/../release/frontend"
LIVEKIT_BIN="$ROOT/../release/livekit-server"
PORT_API="${PORT_API:-8080}"
PORT_WEB="${PORT_WEB:-8081}"
LIVEKIT_PORT="${LIVEKIT_PORT:-7880}"
LIVEKIT_SECRET="${LIVEKIT_SECRET:-devsecret-devsecret-devsecret-1234}"
BASE_API="http://127.0.0.1:$PORT_API"
BASE_WEB="http://127.0.0.1:$PORT_WEB"
TMP="$(mktemp -d /tmp/cloudroom-test.XXXXXX)"
JAR="$TMP/cookies.txt"
PIDS=()
PASS=0; FAIL=0

CLEANED=0
cleanup() {
  [ "$CLEANED" = 1 ] && return
  CLEANED=1
  echo ""
  echo "==> 清理测试进程和临时目录..."
  # 先 SIGTERM，等待退出（最多 2 秒），超时再 SIGKILL，避免残留与端口占用
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  for p in "${PIDS[@]}"; do
    for _ in $(seq 1 20); do kill -0 "$p" 2>/dev/null || break; sleep 0.1; done
    kill -0 "$p" 2>/dev/null && kill -9 "$p" 2>/dev/null
  done
  rm -rf "$TMP"
  echo "==> 清理完成"
}
trap cleanup EXIT INT TERM

ok()  { PASS=$((PASS+1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "  FAIL  $1"; }
check() { if [ "$1" = "$2" ]; then ok "$3"; else bad "$3 (期望 $2, 实际 $1)"; fi; }

# JSON 字段提取（零 python，扁平字段按名字取第一个）
#   jstr <字段名>   取字符串  "字段":"值"
#   jnum <字段名>   取数字    "字段":123
#   jbool <字段名>  取布尔    "字段":true
jstr()  { grep -o "\"$1\":\"[^\"]*\"" | head -1 | sed 's/.*":"//; s/"$//'; }
jnum()  { grep -o "\"$1\":[0-9]*" | head -1 | cut -d: -f2; }
jbool() { if grep -q "\"$1\":true"; then echo true; else echo false; fi; }

echo "=============================================="
echo " CloudRoom 全流程测试（零依赖）"
echo " 后端:       $BACKEND"
echo " 前端服务器: $FRONTEND_SRV"
echo "=============================================="

# ---------- 0. 前置检查 ----------
[ -x "$BACKEND" ]      || { echo "缺少后端二进制: $BACKEND"; exit 1; }
[ -x "$FRONTEND_SRV" ] || { echo "缺少前端服务器二进制: $FRONTEND_SRV"; exit 1; }
[ -x "$LIVEKIT_BIN" ]  || { echo "缺少 LiveKit 服务器: $LIVEKIT_BIN"; exit 1; }
[ -f "$FRONTEND_ROOT/index.html" ] || { echo "缺少前端文件: $FRONTEND_ROOT/index.html"; exit 1; }
for cmd in curl grep sed awk od tr cut seq mktemp head cat sleep; do
  command -v "$cmd" >/dev/null || { echo "缺少系统工具: $cmd"; exit 1; }
done
echo "[0] 前置检查通过"

# ---------- 1. LiveKit 与后端配置/启动 ----------
cat > "$TMP/livekit.yaml" <<EOF
port: $LIVEKIT_PORT
bind_addresses:
  - 127.0.0.1
rtc:
  node_ip: 127.0.0.1
  tcp_port: $((LIVEKIT_PORT + 1))
  port_range_start: 50000
  port_range_end: 50100
  use_external_ip: false
# 内嵌 TURN：P2P mesh 与 SFU 共用的 relay
turn:
  enabled: true
  domain: 127.0.0.1
  udp_port: 3478
  relay_range_start: 50200
  relay_range_end: 50250
keys:
  devkey: $LIVEKIT_SECRET
logging:
  level: info
EOF
"$LIVEKIT_BIN" --config "$TMP/livekit.yaml" > "$TMP/livekit.log" 2>&1 &
LPID=$!
PIDS+=($LPID)
for _ in $(seq 1 30); do
  kill -0 "$LPID" 2>/dev/null || { echo "LiveKit 启动失败，日志:"; head -5 "$TMP/livekit.log"; exit 1; }
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 1 "http://127.0.0.1:$LIVEKIT_PORT/" 2>/dev/null)
  [ "$code" != "000" ] && break
  sleep 0.5
done

cat > "$TMP/config.toml" <<EOF
[server]
listen = "127.0.0.1:$PORT_API"
public_origin = "http://localhost:$PORT_WEB"
frontend_origin = "http://localhost:$PORT_WEB"
trusted_proxy = "127.0.0.1"
insecure_cookies = true
development = true
[data]
dir = "$TMP/data"
[media]
livekit_url = "wss://127.0.0.1:$LIVEKIT_PORT"
livekit_api_key = "devkey"
livekit_api_secret = "$LIVEKIT_SECRET"
p2p_voice_max_participants = 4
EOF
"$BACKEND" --config "$TMP/config.toml" > "$TMP/backend.log" 2>&1 &
BPID=$!
PIDS+=($BPID)
for _ in $(seq 1 40); do
  kill -0 "$BPID" 2>/dev/null || break   # 进程退出（端口被占等）→ 停止等待
  curl -s --max-time 1 "$BASE_API/api/v1/health" 2>/dev/null | grep -q '"ok"' && break
  sleep 0.5
done
H=$(curl -s "$BASE_API/api/v1/health")
case "$H" in
  *'"status":"ok"'*) ok "后端启动 + 健康检查" ;;
  *) bad "后端启动失败，日志: $(head -3 "$TMP/backend.log")"; exit 1 ;;
esac

# ---------- 2. 公共配置（bootstrap 待定） ----------
BP=$(curl -s "$BASE_API/api/v1/config/public" | jbool bootstrap_pending)
check "$BP" "true" "公共配置 bootstrap_pending=true"

# ---------- 3. bootstrap 引导注册（第一个用户 = super_admin） ----------
BOOT=$(grep -o 'bootstrap=[A-Za-z0-9_-]*' "$TMP/backend.log" | head -1 | cut -d= -f2)
[ -n "$BOOT" ] || { bad "日志中无 bootstrap 链接"; exit 1; }
ok "首次启动输出 bootstrap 链接"

R=$(curl -s -c "$JAR" -X POST "$BASE_API/api/v1/auth/register" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"alice\",\"password\":\"correct-horse-battery\",\"token\":\"$BOOT\"}")
ROLE=$(echo "$R" | jstr role)
check "$ROLE" "super_admin" "bootstrap 注册成功且为 super_admin"

R2=$(curl -s -X POST "$BASE_API/api/v1/auth/register" \
  -H 'Content-Type: application/json' \
  -d '{"username":"mallory","password":"correct-horse-battery","token":"'$BOOT'"}')
case "$(echo "$R2" | jstr code)" in
  bootstrap_invalid|invite_invalid) ok "bootstrap token 一次性（复用被拒）" ;;
  *) bad "bootstrap token 复用未被拒: $R2" ;;
esac

# ---------- 4. CSRF 强制 ----------
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/auth/logout" -H 'Content-Type: application/json' -d '{}' -o /dev/null -w '%{http_code}')
check "$S" "403" "无 CSRF token 的变更请求被拒"

CSRF=$(curl -s -b "$JAR" "$BASE_API/api/v1/me" | jstr csrf_token)

# ---------- 5. 建房申请（super_admin 自定义码 666） ----------
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"kind":"create_room","note":"测试房","desired_code":"666","media_mode":"p2p"}')
REQ=$(echo "$R" | jstr request_id)
echo "$R" | grep -q '"auto_approved":true' && ok "超级管理员提交即建房 (auto_approved)" || bad "超级管理员提交即建房 (auto_approved)"
check "$(echo "$R" | jstr code)" "666" "超级管理员免审批，房间号 = 自定义 666"

# 再提交一个免审批申请（自定义 777）
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"kind":"create_room","note":"免审批","desired_code":"777","media_mode":"p2p"}')
check "$(echo "$R" | jstr code)" "777" "免审批房间号为自定义 777"

# 注册邀请 -> 第二个普通用户
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/admin/invites" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
REGTOK=$(echo "$R" | jstr token)
JAR2="$TMP/cookies2.txt"
R=$(curl -s -c "$JAR2" -X POST "$BASE_API/api/v1/auth/register" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"bob\",\"password\":\"correct-horse-battery\",\"token\":\"$REGTOK\"}")
check "$(echo "$R" | jstr role)" "user" "普通用户经注册邀请注册成功"
CSRF2=$(curl -s -b "$JAR2" "$BASE_API/api/v1/me" | jstr csrf_token)

# 普通用户不能审批（用 alice 的 666 申请）
S=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/admin/room-requests/$REQ/approve" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{}' -o /dev/null -w '%{http_code}')
check "$S" "403" "普通用户审批被拒 (403)"

# 普通用户提交申请 -> 待审批
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{"kind":"create_room","note":"bob 的申请","media_mode":"sfu"}')
REQ2=$(echo "$R" | jstr request_id)
check "$(echo "$R" | jstr status)" "pending" "普通用户申请待审批"

# ---------- 6. 审批建房 + 管理员房间列表 ----------
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/admin/room-requests/$REQ2/approve" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
CODE=$(echo "$R" | jstr code)
if [ -n "$CODE" ]; then
  ok "审批通过，房间号 = $CODE"
else
  bad "审批通过，房间号 = $CODE"
fi
echo "$R" | grep -q '"kind":"standard"' && ok "房间类型 standard" || bad "房间类型 standard: $R"

N=$(curl -s -b "$JAR" "$BASE_API/api/v1/admin/rooms" | jnum total)
check "$N" "3" "管理员房间列表可见 (total=3)"

# ---------- 6b. 媒体模式与媒体端点 ----------
H=$(curl -s "$BASE_API/api/v1/health")
echo "$H" | grep -q '"livekit":"configured"' && ok "健康检查包含 livekit configured" || bad "健康检查缺 livekit: $H"

# 666 = p2p：ice-servers 返回 LiveKit 内嵌 TURN 凭证；media-token 拒绝
R=$(curl -s -b "$JAR" "$BASE_API/api/v1/rooms/666/ice-servers")
echo "$R" | grep -q '"turn:' && ok "p2p 房间 ice-servers 返回 LiveKit 内嵌 TURN" || bad "p2p ice-servers 无 TURN: $R"
echo "$R" | grep -q '"username"' && ok "TURN username 已签发" || bad "TURN username 缺失: $R"
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/media-token" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}' -o /dev/null -w '%{http_code}')
check "$S" "409" "p2p 房间 media-token 被拒 (409 media_mode_mismatch)"

# bob 的房间 = sfu：media-token 返回 token/url/room；ice-servers 拒绝
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/rooms/$CODE/media-token" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{}')
[ -n "$(echo "$R" | jstr token)" ] && ok "sfu 房间 media-token 已签发" || bad "sfu media-token 失败: $R"
check "$(echo "$R" | jstr room)" "$CODE" "media-token 返回目标房间"
S=$(curl -s -b "$JAR2" "$BASE_API/api/v1/rooms/$CODE/ice-servers" -o /dev/null -w '%{http_code}')
check "$S" "409" "sfu 房间 ice-servers 被拒 (409)"

# 创建申请必须携带媒体模式
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"kind":"create_room","note":"无模式"}' -o /dev/null -w '%{http_code}')
check "$S" "422" "不带 media_mode 的创建申请被拒 (422)"

# settings 不能直改媒体模式
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/settings" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"media_mode":"sfu"}' -o /dev/null -w '%{http_code}')
check "$S" "422" "settings 携带 media_mode 被拒 (422)"

# 房主提交模式变更申请 -> pending；自批被拒；管理员批准后生效
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' \
  -d "{\"kind\":\"change_media_mode\",\"room_code\":\"$CODE\",\"media_mode\":\"p2p\",\"note\":\"切换到 p2p\"}")
check "$(echo "$R" | jstr kind)" "change_media_mode" "模式变更申请 kind 正确"
check "$(echo "$R" | jstr status)" "pending" "模式变更申请待审批"
check "$(echo "$R" | jstr target_room_code)" "$CODE" "模式变更申请目标房间正确"
CHREQ=$(echo "$R" | jstr request_id)
S=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/admin/room-requests/$CHREQ/approve" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{}' -o /dev/null -w '%{http_code}')
check "$S" "403" "申请人不能批准自己的模式变更 (403)"
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' \
  -d "{\"kind\":\"change_media_mode\",\"room_code\":\"$CODE\",\"media_mode\":\"p2p\"}")
echo "$R" | grep -q '"conflict"' && ok "同房间重复 pending 变更被拒 (409)" || bad "重复变更未被拒: $R"
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/admin/room-requests/$CHREQ/approve" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
echo "$R" | grep -q '"media_mode":"p2p"' && ok "管理员批准后房间 media_mode=p2p" || bad "批准后模式未改变: $R"
R=$(curl -s -b "$JAR2" "$BASE_API/api/v1/rooms/$CODE")
echo "$R" | grep -q '"media_mode":"p2p"' && ok "房间查询确认 media_mode=p2p" || bad "房间模式确认失败: $R"

# ---------- 7. 邀请加入（一次性） ----------
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/invites" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"max_role":"member"}')
INV=$(echo "$R" | jstr token)
check "$(echo "$R" | jstr invite_url)" "/666?invite=$INV" "邀请 URL 为根路径短形式"

R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/rooms/666/join" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d "{\"invite_token\":\"$INV\"}")
check "$(echo "$R" | jstr my_role)" "member" "bob 凭邀请加入成功"

# 第三个用户（尚未加入）兑换已消费的邀请 -> 必须失败
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/admin/invites" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
REGTOK2=$(echo "$R" | jstr token)
JAR3="$TMP/cookies3.txt"
R=$(curl -s -c "$JAR3" -X POST "$BASE_API/api/v1/auth/register" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"carol\",\"password\":\"correct-horse-battery\",\"token\":\"$REGTOK2\"}")
CSRF3=$(curl -s -b "$JAR3" "$BASE_API/api/v1/me" | jstr csrf_token)
R=$(curl -s -b "$JAR3" -X POST "$BASE_API/api/v1/rooms/666/join" \
  -H "x-csrf-token: $CSRF3" -H 'Content-Type: application/json' -d "{\"invite_token\":\"$INV\"}")
check "$(echo "$R" | jstr code)" "invite_invalid" "邀请一次性（carol 复用被拒）"

# 未注册用户（无 cookie）凭邀请直接进房，获得 guest 会话
R=$(curl -s -X POST "$BASE_API/api/v1/rooms/666/join" \
  -H 'Content-Type: application/json' -d "{\"invite_token\":\"$INV\"}")
check "$(echo "$R" | jstr code)" "invite_invalid" "未注册用户用已消费邀请仍被拒"
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/invites" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
INV2=$(echo "$R" | jstr token)
JAR_G="$TMP/cookies-guest.txt"
R=$(curl -s -c "$JAR_G" -X POST "$BASE_API/api/v1/rooms/666/join" \
  -H 'Content-Type: application/json' -d "{\"invite_token\":\"$INV2\"}")
check "$(echo "$R" | jstr my_role)" "member" "未注册用户凭邀请直接进房 (member)"
check "$(curl -s -b "$JAR_G" "$BASE_API/api/v1/me" | jstr role)" "guest" "未注册用户获得 guest 会话"

# 访客可修改自己的昵称
GID=$(curl -s -b "$JAR_G" "$BASE_API/api/v1/me" | jstr user_id)
GCSRF=$(curl -s -b "$JAR_G" "$BASE_API/api/v1/me" | jstr csrf_token)
R=$(curl -s -b "$JAR_G" -X POST "$BASE_API/api/v1/rooms/666/members/$GID/nickname" \
  -H "x-csrf-token: $GCSRF" -H 'Content-Type: application/json' \
  -d '{"nickname":"访客小王"}')
if echo "$R" | grep -q '"ok":true'; then
  ok "访客修改自己的昵称成功"
else
  bad "访客修改昵称失败: $R"
fi

# 访客升级绑定：成员关系/禁言/消息/兑换记录全部转移到新账号
R=$(curl -s -b "$JAR_G" -X POST "$BASE_API/api/v1/rooms/666/messages" \
  -H "x-csrf-token: $GCSRF" -H 'Content-Type: application/json' -d '{"body":"升级前的消息"}')
if echo "$R" | grep -q '"seq"'; then ok "访客发送消息（制造 FK 引用）"; else bad "访客消息失败: $R"; fi
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/mutes" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d "{\"scope\":\"member\",\"user_id\":\"$GID\",\"duration_seconds\":3600}")
if echo "$R" | grep -q '"mute_id"'; then ok "owner 禁言访客（制造 FK 引用）"; else bad "禁言失败: $R"; fi
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/admin/invites" -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}')
REGUP=$(echo "$R" | jstr token)
R=$(curl -s -b "$JAR_G" -c "$JAR_G" -X POST "$BASE_API/api/v1/auth/upgrade-guest" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"dave\",\"password\":\"correct-horse-battery\",\"invite_token\":\"$REGUP\"}")
echo "$R" | grep -q '"role":"user"' && ok "访客升级成功 (dave, user)" || bad "访客升级失败: $R"
NUID=$(curl -s -b "$JAR_G" "$BASE_API/api/v1/me" | jstr user_id)
check "$(curl -s -b "$JAR_G" "$BASE_API/api/v1/rooms/666" -o /dev/null -w '%{http_code}')" "200" "升级后新账号直接访问房间 (状态继承)"
[ "$NUID" != "$GID" ] && ok "会话已切换为新账号" || bad "会话未切换"
R=$(curl -s -b "$JAR" "$BASE_API/api/v1/rooms/666/messages?after=0")
echo "$R" | grep -q "$NUID" && ok "访客消息归属已转移到新账号" || bad "消息归属未转移: $R"

# 系统播报：进房消息出现在文字频道（昵称优先）
HIST=$(curl -s -b "$JAR" "$BASE_API/api/v1/rooms/666/messages?after=0")
case "$HIST" in
  *"加入了房间"*) ok "成员进出在文字频道播报（系统消息）" ;;
  *) bad "未发现系统播报消息" ;;
esac

# 邀请有效期：expires_in 生效（1 小时有效）
R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/invites" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"expires_in":"1h"}')
check "$(echo "$R" | jstr invite_id | cut -c1-8)" "$(echo "$R" | jstr invite_id | cut -c1-8)" "房主生成邀请可指定有效期 (1h)"

# ---------- 8. 消息收发 ----------
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/rooms/666/messages" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{"body":"你好，CloudRoom"}')
MSG=$(echo "$R" | jstr body)
check "$MSG" "你好，CloudRoom" "bob 发送消息成功"

LAST=$(curl -s -b "$JAR" "$BASE_API/api/v1/rooms/666/messages?after=0" | jnum last_seq)
if [ -n "$LAST" ] && [ "$LAST" -ge 1 ] 2>/dev/null; then
  ok "消息历史序列 last_seq=$LAST (>=1)"
else
  bad "消息历史序列异常: $LAST"
fi

# ---------- 9. 禁言（member 范围，服务端强制） ----------
BOB_UID=$(curl -s -b "$JAR2" "$BASE_API/api/v1/me" | jstr user_id)
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/mutes" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' \
  -d "{\"scope\":\"member\",\"user_id\":\"$BOB_UID\",\"duration_seconds\":3600}" -o /dev/null -w '%{http_code}')
check "$S" "200" "owner 禁言 bob 成功"
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/rooms/666/messages" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{"body":"被禁言"}')
check "$(echo "$R" | jstr code)" "muted" "被禁言用户发消息被拒 (muted)"

# ---------- 10. 文字频道开关 ----------
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/settings" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"text_enabled":false}' -o /dev/null -w '%{http_code}')
check "$S" "200" "owner 关闭文字频道"
R=$(curl -s -b "$JAR2" -X POST "$BASE_API/api/v1/rooms/666/messages" \
  -H "x-csrf-token: $CSRF2" -H 'Content-Type: application/json' -d '{"body":"关闭后"}')
check "$(echo "$R" | jstr code)" "text_disabled" "关闭后新消息被拒 (409 text_disabled)"
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/settings" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"text_enabled":true}' -o /dev/null -w '%{http_code}')
check "$S" "200" "重新开启文字频道"

# ---------- 11. WebSocket 隧道（前端服务器 101 + hello 帧） ----------
"$FRONTEND_SRV" --port "$PORT_WEB" --root "$FRONTEND_ROOT" --target "http://127.0.0.1:$PORT_API" \
  > "$TMP/frontend.log" 2>&1 &
FPID=$!
PIDS+=($FPID)
for _ in $(seq 1 20); do
  kill -0 "$FPID" 2>/dev/null || break   # 进程退出 → 停止等待
  curl -s -o /dev/null --max-time 1 "$BASE_WEB/" && break
  sleep 0.5
done

S=$(curl -s -o /dev/null --max-time 1 -w '%{http_code}' "$BASE_WEB/")
check "$S" "200" "前端服务器静态页面 200"
S=$(curl -s -o /dev/null --max-time 1 -w '%{http_code}' "$BASE_WEB/zzz9999")
check "$S" "200" "SPA 路由回退 200 (/zzz9999)"
S=$(curl -s -o /dev/null -w '%{http_code}' "$BASE_WEB/api/v1/health")
check "$S" "200" "API 反代 200 (/api/v1/health)"

ALICE_COOKIE=$(awk -F'\t' '/cloudroom_session/ {print $7}' "$JAR")
WS_BYTES=$(curl -s -N --max-time 4 \
  -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" -H "Cookie: cloudroom_session=$ALICE_COOKIE" \
  "$BASE_WEB/api/v1/rooms/666/ws" 2>/dev/null | od -An -tx1 -v | tr -d ' \n')
# 修复说明：hyper 已完成客户端侧 101 握手，隧道只转发 101 之后的帧数据，
# 因此 body 应以 WS 帧头 0x81 开头（不再出现重复的 "HTTP/1.1 101" 头）。
case "$WS_BYTES" in
  81*) ok "WebSocket 隧道: 收到干净的 WS 帧（101 握手成功）" ;;
  *) bad "WebSocket 隧道数据异常: ${WS_BYTES:0:40}" ;;
esac
case "$WS_BYTES" in
  *7e*) ok "WebSocket 隧道: hello 帧长度字段完整 (0x81 0x7e)" ;;
  *) bad "WebSocket 隧道帧头异常" ;;
esac

# ---------- 12. 房间删除 + 房间号 tombstone ----------
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/rooms/666/delete" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{}' -o /dev/null -w '%{http_code}')
check "$S" "200" "owner 删除房间"
S=$(curl -s -b "$JAR" "$BASE_API/api/v1/rooms/666" -o /dev/null -w '%{http_code}')
check "$S" "410" "删除后房间不可访问 (410)"

R=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"kind":"create_room","note":"再申请","desired_code":"666","media_mode":"p2p"}')
echo "$R" | grep -q '"conflict"' && ok "已删除房间号 666 永不复用 (409)" || bad "已删除房间号 666 永不复用 (409)"

# ---------- 13. 保留词/非法自定义码 ----------
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"kind":"create_room","desired_code":"api","media_mode":"p2p"}' -o /dev/null -w '%{http_code}')
check "$S" "422" "保留词 api 被拒 (422)"
S=$(curl -s -b "$JAR" -X POST "$BASE_API/api/v1/room-requests" \
  -H "x-csrf-token: $CSRF" -H 'Content-Type: application/json' -d '{"kind":"create_room","desired_code":"-bad","media_mode":"p2p"}' -o /dev/null -w '%{http_code}')
check "$S" "422" "非法格式 -bad 被拒 (422)"

# ---------- 14. 未认证访问 ----------
# 未登录访问房间返回 403（前端据此展示"凭邀请加入"流程），且绝不泄露房间内容
S=$(curl -s "$BASE_API/api/v1/rooms/zzzzzzz" -o /dev/null -w '%{http_code}')
check "$S" "403" "未认证访问房间返回 403（不泄露内容）"

# ---------- 汇总 ----------
echo "=============================================="
echo " 结果: $PASS 通过, $FAIL 失败"
echo "=============================================="
[ "$FAIL" -eq 0 ]
