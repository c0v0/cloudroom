#!/usr/bin/env bash
# CloudRoom 一键验收（唯一验收入口；AGENTS.md「执行轮/验收轮」）
#
# 覆盖：密钥扫描 → Rust fmt/clippy/构建/测试 → 前端类型检查/构建 → ESLint（配置了就跑）
#       → 端到端集成（test-server/run-tests.sh） → 依赖漏洞/许可 → 锁文件快照
#
# 退出码：任何 FAIL → 1；--strict 时 SKIP 也算失败；否则 SKIP 只提示（缺工具/缺前置）。
#
# 用法：
#   ./scripts/check.sh                        # 本机可跑的全部门禁
#   ./scripts/check.sh --strict               # CI/发布前：缺工具、被跳过的项都算失败
#   SKIP_INTEGRATION=1 ./scripts/check.sh     # 跳过端到端（缺 release 产物时默认跳过）
#   SKIP_NETWORK=1 ./scripts/check.sh         # 跳过需要网络的步骤（npm ci / npm audit）
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STRICT=0
for a in "$@"; do [ "$a" = "--strict" ] && STRICT=1; done
SKIP_INTEGRATION="${SKIP_INTEGRATION:-0}"
SKIP_NETWORK="${SKIP_NETWORK:-0}"

PASS=0; FAIL=0; SKIP=0
section() { printf '\n=== %s ===\n' "$*"; }
pass() { PASS=$((PASS + 1)); printf '  PASS  %s\n' "$*"; }
fail() { FAIL=$((FAIL + 1)); printf '  FAIL  %s\n' "$*"; }
skip() { SKIP=$((SKIP + 1)); printf '  SKIP  %s\n' "$*"; }
note() { printf '  ----  %s\n' "$*"; }

gate() { # gate <名称> <命令...>：失败打印输出尾部
  local name="$1"; shift
  local log; log="$(mktemp)"
  if "$@" >"$log" 2>&1; then
    pass "$name"
  else
    fail "$name"
    tail -n 15 "$log" | sed 's/^/  | /'
  fi
  rm -f "$log"
}

cargo_in() { local dir="$1"; shift; ( cd "$dir" && cargo "$@" ); }
cargo_audit() {
  cargo audit --file backend/Cargo.lock &&
    cargo audit --file frontend-server/Cargo.lock
}
npm_in()   { local dir="$1"; shift; ( cd "$dir" && npm "$@" ); }
has()      { command -v "$1" >/dev/null 2>&1; }

section "1/10 密钥扫描（工作区 + 全部 git 历史）"
gate "scripts/secret-scan.sh --all" scripts/secret-scan.sh --all

section "2/10 Rust 格式（cargo fmt --check）"
if has cargo; then
  for crate in backend frontend-server; do
    gate "cargo fmt --check（$crate）" cargo_in "$crate" fmt --all -- --check
  done
else
  skip "未安装 cargo"
fi

section "3/10 Rust 静态检查（cargo clippy, -D warnings）"
if has cargo && cargo clippy --version >/dev/null 2>&1; then
  CLIPPY_ARGS=(--all-targets --locked -- -D warnings)
  for crate in backend frontend-server; do
    gate "cargo clippy（$crate, -D warnings）" cargo_in "$crate" clippy "${CLIPPY_ARGS[@]}"
  done
else
  skip "未安装 cargo-clippy（rustup component add clippy）"
fi

section "4/10 Rust 构建（cargo build --locked）"
if has cargo; then
  for crate in backend frontend-server; do
    gate "cargo build --locked（$crate）" cargo_in "$crate" build --locked --bins
  done
else
  skip "未安装 cargo"
fi

section "5/10 Rust 测试（单元 + tests/e2e.rs）"
if has cargo; then
  gate "cargo test --locked（backend）" cargo_in backend test --locked
  gate "cargo test --locked（frontend-server）" cargo_in frontend-server test --locked
else
  skip "未安装 cargo"
fi

section "6/10 前端（依赖 → 类型检查 → 构建）"
if has node && has npm; then
  if [ ! -d frontend/node_modules ]; then
    if [ "$SKIP_NETWORK" = 1 ]; then
      skip "frontend/node_modules 缺失且 SKIP_NETWORK=1（先跑 npm ci）"
    else
      gate "npm ci（frontend，按 lockfile）" npm_in frontend ci --no-audit --no-fund
    fi
  else
    note "frontend/node_modules 已存在（跳过 npm ci；干净复现见 scripts/repro.sh）"
  fi
  if [ -d frontend/node_modules ]; then
    gate "npm run typecheck（tsc -b）" npm_in frontend run typecheck
    gate "npm test（vitest run）" npm_in frontend test
    gate "npm run build（vite build）" npm_in frontend run build
  fi
else
  skip "未安装 node/npm"
fi

section "7/10 前端 ESLint"
gate "npm run lint（frontend，--max-warnings=0）" npm_in frontend run lint

section "8/10 端到端集成（test-server/run-tests.sh）"
if [ "$SKIP_INTEGRATION" = 1 ]; then
  skip "SKIP_INTEGRATION=1"
elif [ ! -x release/cloudroom-server ] || [ ! -x release/cloudroom-frontend-server ] \
  || [ ! -x release/livekit-server ] || [ ! -f release/frontend/index.html ]; then
  skip "缺 release/ 产物（跑 scripts/build-release.sh；livekit-server 需按 release/deployment-zh.md 获取）"
else
  gate "test-server/run-tests.sh（全流程）" bash test-server/run-tests.sh
fi

section "9/10 依赖漏洞"
if [ "$SKIP_NETWORK" = 1 ]; then
  skip "SKIP_NETWORK=1（npm audit 需要网络）"
elif [ -d frontend/node_modules ] && has npm; then
  AUDIT_LOG="$(mktemp)"
  if ( cd frontend && npm audit --omit=dev --audit-level=high ) >"$AUDIT_LOG" 2>&1; then
    pass "npm audit --omit=dev --audit-level=high"
  elif grep -qiE 'ENOTFOUND|EAI_AGAIN|ENETUNREACH|network|ECONNREFUSED|fetch failed' "$AUDIT_LOG"; then
    skip "npm audit 无法联网（离线环境）"
  else
    fail "npm audit（发现高危漏洞）"
    tail -n 15 "$AUDIT_LOG" | sed 's/^/  | /'
  fi
  rm -f "$AUDIT_LOG"
else
  skip "frontend/node_modules 不存在或 npm 缺失"
fi
if has cargo-audit; then
  gate "cargo audit（backend + frontend-server）" cargo_audit
else
  skip "cargo audit 未执行：未安装 cargo-audit（cargo install cargo-audit --locked）"
fi

section "10/10 依赖许可 + 锁文件快照"
if has jq; then
  LICENSE_LOG="$(mktemp)"
  scripts/license-report.sh >"$LICENSE_LOG" 2>&1
  case "$?" in
    0) pass "scripts/license-report.sh（无未知许可）" ;;
    3) skip "许可清单前置条件缺失（$(grep -m1 'SKIP' "$LICENSE_LOG" | sed 's/^\[license\] //')）" ;;
    *) fail "scripts/license-report.sh（存在未知许可）" ;;
  esac
  sed -n '1,6p' "$LICENSE_LOG" | sed 's/^/  ----  /'
  rm -f "$LICENSE_LOG"
else
  skip "未安装 jq（许可清单需要）"
fi
printf '  锁文件快照（sha256 前 16 位）：\n'
for f in backend/Cargo.lock frontend-server/Cargo.lock frontend/package-lock.json; do
  printf '    %s  %s\n' "$(sha256sum "$f" | cut -c1-16)" "$f"
done
pass "锁文件已入库；Rust 门禁统一 --locked、前端统一 npm ci"

printf '\n==============================================\n'
printf ' 结果：%d PASS / %d FAIL / %d SKIP\n' "$PASS" "$FAIL" "$SKIP"
printf '==============================================\n'

if [ "$FAIL" -gt 0 ]; then exit 1; fi
if [ "$STRICT" = 1 ] && [ "$SKIP" -gt 0 ]; then
  echo " --strict：存在 SKIP 项，视为失败"
  exit 1
fi
exit 0
