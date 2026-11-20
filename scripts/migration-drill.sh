#!/usr/bin/env bash
# 迁移演练：在数据库副本上启动新二进制，确认迁移全部应用、健康检查通过。
#
# 用法：
#   sudo ./scripts/migration-drill.sh /var/lib/cloudroom/cloudroom.db /etc/cloudroom/config.toml
#
# 不会修改生产库；副本与临时配置都在 mktemp 目录，退出时清理。
set -euo pipefail

DB="${1:?用法: migration-drill.sh <db 路径> <config.toml>}"
CFG="${2:?用法: migration-drill.sh <db 路径> <config.toml>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${CLOUDROOM_BIN:-$ROOT/release/cloudroom-server}"
PORT="${DRILL_PORT:-18080}"

[[ -f "$DB" ]] || { echo "数据库不存在: $DB" >&2; exit 1; }
[[ -f "$CFG" ]] || { echo "配置不存在: $CFG" >&2; exit 1; }
[[ -x "$BIN" ]] || { echo "二进制不存在或不可执行: $BIN" >&2; exit 1; }

TMP="$(mktemp -d)"
PID=""
cleanup() {
  [[ -n "$PID" ]] && kill "$PID" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

if command -v sqlite3 >/dev/null 2>&1; then
  sqlite3 "$DB" ".backup '"$TMP/cloudroom.db"'"
else
  cp "$DB" "$TMP/cloudroom.db"
fi
# 只改数据目录与监听端口；LiveKit 等密钥沿用生产配置。
sed -e "s|^dir = .*|dir = \"$TMP\"|" -e "s|^listen = .*|listen = \"127.0.0.1:$PORT\"|" "$CFG" > "$TMP/config.toml"

"$BIN" --config "$TMP/config.toml" >"$TMP/server.log" 2>&1 &
PID=$!

for _ in $(seq 1 60); do
  if curl -sf --max-time 1 "http://127.0.0.1:$PORT/api/v1/health" >/dev/null; then
    break
  fi
  if ! kill -0 "$PID" 2>/dev/null; then
    echo "服务启动失败：" >&2
    tail -n 20 "$TMP/server.log" >&2
    exit 1
  fi
  sleep 1
done

curl -sf --max-time 2 "http://127.0.0.1:$PORT/api/v1/health" >/dev/null || {
  echo "健康检查失败：" >&2
  tail -n 20 "$TMP/server.log" >&2
  exit 1
}

echo "迁移版本（副本）："
sqlite3 "$TMP/cloudroom.db" "SELECT version FROM schema_migrations ORDER BY version;"

echo "迁移演练通过：数据库副本可正常启动并通过健康检查。"
