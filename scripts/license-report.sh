#!/usr/bin/env bash
# 依赖许可清单 + “未知许可”门禁（不装额外工具：cargo metadata + lockfile/node_modules）
#
# 用法：scripts/license-report.sh
# 退出码：0 = 全部依赖有许可声明；1 = 有依赖缺许可（需处理或加例外）；3 = 前置条件缺失（调用方按 SKIP 处理）
#
# 例外：把 `pkg@version`（Rust 用 `name version`）写进 scripts/license-exceptions.txt 并注明原因。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
EXCEPTIONS="$ROOT/scripts/license-exceptions.txt"

command -v jq >/dev/null 2>&1 || { echo "[license] SKIP：需要 jq"; exit 3; }
command -v cargo >/dev/null 2>&1 || { echo "[license] SKIP：需要 cargo（Rust 部分）"; exit 3; }

excepted() { # $1=标识（pkg@version 或 "name version"）
  [ -f "$EXCEPTIONS" ] || return 1
  grep -vE '^[[:space:]]*(#|$)' "$EXCEPTIONS" 2>/dev/null | grep -qxF "$1"
}

UNKNOWN=0
declare -A COUNTS=()

add() { # $1=许可表达式（空=未知） $2=标识
  if [ -z "$1" ]; then
    if excepted "$2"; then
      COUNTS["(except) 未知"]=$(( ${COUNTS["(except) 未知"]:-0} + 1 ))
    else
      echo "  UNKNOWN  $2"
      UNKNOWN=$((UNKNOWN + 1))
    fi
    return
  fi
  COUNTS["$1"]=$(( ${COUNTS["$1"]:-0} + 1 ))
}

echo "[license] Rust（cargo metadata，backend + frontend-server）..."
for manifest in backend/Cargo.toml frontend-server/Cargo.toml; do
  META="$(mktemp)"
  if ! cargo metadata --locked --format-version 1 --manifest-path "$manifest" >"$META" 2>/dev/null; then
    echo "[license] SKIP：cargo metadata 失败（$manifest，需要网络或本地 registry）"
    rm -f "$META"
    exit 3
  fi
  while IFS=$'\t' read -r lic name ver; do
    add "$lic" "$name $ver"
  done < <(jq -r '.packages[] | select(.source != null) | [(.license // ""), .name, .version] | @tsv' "$META")
  rm -f "$META"
done

echo "[license] Node（frontend/node_modules，按 package-lock.json 精确枚举）..."
if [ ! -d frontend/node_modules ]; then
  echo "[license] SKIP：frontend/node_modules 不存在（先 npm ci）"
  exit 3
fi
while IFS= read -r pkg_path; do
  f="frontend/$pkg_path/package.json"
  [ -f "$f" ] || continue   # 平台相关的可选依赖未安装，跳过
  lic="$(jq -r 'if .license then .license elif .licenses then ([.licenses[].type] | join(" OR ")) else "" end' "$f" 2>/dev/null)"
  add "$lic" "$(jq -r '.name + "@" + .version' "$f" 2>/dev/null)"
done < <(jq -r '.packages | keys[] | select(. != "")' frontend/package-lock.json)

echo "[license] 许可分布（Top 15）："
for k in "${!COUNTS[@]}"; do printf '%s\t%s\n' "${COUNTS[$k]}" "$k"; done | sort -rn | head -15 | sed 's/^/  /'

if [ "$UNKNOWN" -gt 0 ]; then
  echo "[license] FAIL：$UNKNOWN 个已安装依赖缺少许可声明"
  exit 1
fi
echo "[license] PASS：未发现缺少许可声明的依赖"
