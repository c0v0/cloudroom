#!/usr/bin/env bash
# 干净 clone 复现（AGENTS.md「验收轮」要求）：不改当前工作区
#
#   1) 从当前 HEAD 克隆到临时目录（被 .gitignore 的产物不会带过去）
#   2) npm ci（严格按 package-lock.json）+ 前端构建
#   3) cargo test --locked（backend + frontend-server，锁文件变化即失败）
#
# 需要网络（npm registry / crates.io）。端到端集成需要 release/ 产物 + LiveKit 二进制，
# 见 scripts/check.sh（本脚本只覆盖“干净环境装依赖并跑测试”）。
#
# 用法：
#   scripts/repro.sh                 # 跑完清理临时目录
#   scripts/repro.sh --keep          # 保留临时目录（排查用，会打印路径）
#   scripts/repro.sh --dir <路径>     # 指定克隆目标目录
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

KEEP=0
DST=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --keep) KEEP=1; shift ;;
    --dir) DST="${2:-}"; shift 2 ;;
    *) echo "用法: $0 [--keep] [--dir <路径>]" >&2; exit 2 ;;
  esac
done

SHA="$(git rev-parse HEAD)"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [ -z "$DST" ]; then
  DST="$(mktemp -d /tmp/cloudroom-repro.XXXXXX)"
fi
REPO="$DST/repo"

PASS=0; FAIL=0
step() { # step <名称> <命令...>：失败继续跑，最后汇总
  local name="$1"; shift
  local log; log="$(mktemp)"
  if "$@" >"$log" 2>&1; then
    PASS=$((PASS + 1)); printf '  PASS  %s\n' "$name"
  else
    FAIL=$((FAIL + 1)); printf '  FAIL  %s\n' "$name"
    tail -n 15 "$log" | sed 's/^/  | /'
  fi
  rm -f "$log"
}

cleanup() {
  if [ "$KEEP" = 1 ]; then
    echo "[repro] 保留：$DST"
  else
    case "$DST" in
      /tmp/cloudroom-repro.*) rm -rf "$DST" ;;
      *) echo "[repro] 非临时目录，不自动删除：$DST" ;;
    esac
  fi
}
trap cleanup EXIT INT TERM

echo "[repro] HEAD=$SHA（$BRANCH）→ $REPO"
step "git clone（HEAD，不含工作区改动）" git clone --quiet --no-checkout "$ROOT" "$REPO"
[ -d "$REPO/.git" ] || { echo "[repro] clone 失败，终止"; exit 1; }
step "checkout 指定提交" git -C "$REPO" checkout --quiet "$SHA"

step "clone 工作区干净（无未提交/未跟踪文件）" bash -c \
  '[ -z "$(git -C "'"$REPO"'" status --porcelain)" ]'

step "npm ci（frontend，严格按 lockfile）" bash -c \
  'cd "'"$REPO"'/frontend" && npm ci --no-audit --no-fund'
step "package-lock.json 未被 npm ci 改写" bash -c \
  'git -C "'"$REPO"'" diff --exit-code -- frontend/package-lock.json'
step "npm run build（前端类型检查 + 构建）" bash -c \
  'cd "'"$REPO"'/frontend" && npm run build'
step "cargo test --locked（backend）" bash -c \
  'cd "'"$REPO"'/backend" && cargo test --locked'
step "cargo test --locked（frontend-server）" bash -c \
  'cd "'"$REPO"'/frontend-server" && cargo test --locked'

printf '\n[repro] 结果：%d PASS / %d FAIL\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
