#!/usr/bin/env bash
# CloudRoom 密钥扫描（零依赖：git + grep）
#
# 用法：
#   scripts/secret-scan.sh              # 工作区（已跟踪 + 未跟踪，忽略 gitignore 命中的路径）
#   scripts/secret-scan.sh --history    # 全部 git 历史（所有分支/tag 可达的提交）
#   scripts/secret-scan.sh --staged     # 暂存区（pre-commit 用）
#   scripts/secret-scan.sh --all        # 工作区 + 历史
#
# 命中时打印 `位置: 内容(截断)` 并退出码 1；无命中退出 0。
# 误报处理：把能精确识别该行的片段（ERE）加进 scripts/secret-allowlist.txt，
# 并注明为什么它不是凭据。allowlist 按“整行子串匹配”。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
ALLOWLIST="$ROOT/scripts/secret-allowlist.txt"
GIT=(git -c core.quotepath=false)

# ---------- 检测模式（ERE） ----------
# 私钥头由两段拼接，避免扫描器源码里的字面量自命中。
PEM_KEY_HEAD='-----BEGIN [A-Z ]*PRIVATE'
PEM_KEY_TAIL='[ -]KEY-----'
# 1) 高置信度凭据形态
HIGH_PATTERNS=(
  "${PEM_KEY_HEAD}${PEM_KEY_TAIL}"
  'AKIA[0-9A-Z]{16}'
  '(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}'
  'github_pat_[A-Za-z0-9_]{22,}'
  'glpat-[A-Za-z0-9_-]{20,}'
  'xox[baprs]-[A-Za-z0-9-]{10,}'
  'sk_live_[A-Za-z0-9]{20,}'
  'sk-ant-[A-Za-z0-9_-]{20,}'
  'sk-[A-Za-z0-9]{32,}'
  'AIza[0-9A-Za-z_-]{35}'
  '[0-9]{8,10}:AA[A-Za-z0-9_-]{33}'
  'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}'
)
# 2) 通用“敏感键 = 字面量”赋值（大小写不敏感，值 >= 12 字符）。
#    排除 ${VAR} 变量引用（值以 $ 开头）与 <占位符> 形式，降低误报。
GENERIC_PATTERN="(secret|token|passwd|password|credential|api[_-]?key|apikey|access[_-]?key|private[_-]?key)[a-z0-9_-]*[[:space:]]*[:=][[:space:]]*[\"'][^\"'\$<]{12,}[\"']"
# 3) 敏感文件名（出现即命中）
NAME_PATTERN='(^|/)(\.env(\..+)?|\.npmrc|\.netrc|id_rsa|id_ed25519|credentials(\.json)?)$|\.(pem|key|p12|pfx|jks|keystore)$'

PATTERN_ARGS=(-e "$GENERIC_PATTERN")
for p in "${HIGH_PATTERNS[@]}"; do PATTERN_ARGS+=(-e "$p"); done

# ---------- allowlist ----------
ALLOW=()
if [ -f "$ALLOWLIST" ]; then
  while IFS= read -r p; do
    case "$p" in ''|'#'*) continue ;; esac
    ALLOW+=("$p")
  done < <(grep -vE '^[[:space:]]*(#|$)' "$ALLOWLIST")
fi
allowlisted() { # $1=整行
  local p
  for p in "${ALLOW[@]}"; do
    grep -qE -e "$p" <<<"$1" && return 0
  done
  return 1
}

HITS=0
TRUNC=160
emit() { # $1=位置 $2=内容
  allowlisted "$1: $2" && return 0
  HITS=$((HITS + 1))
  printf '  HIT  %s: %s\n' "$1" "${2:0:$TRUNC}"
}

# 解析 `路径:行号:内容`（调用方补路径前缀），在**当前 shell** 内遍历（不能用管道，否则计数丢失）
scan_stream() { # $1=路径前缀 $2=是否给路径加前缀（1=是）
  local prefix="$1" path_is_label="$2" hit f rest ln c
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    f="${hit%%:*}"; rest="${hit#*:}"
    ln="${rest%%:*}"; c="${rest#*:}"
    if [ "$path_is_label" = 1 ]; then f="$prefix$f"; else ln="$prefix$ln"; fi
    emit "$f:$ln" "$c"
  done
}

check_names() { # $1=路径前缀；stdin=路径列表
  local prefix="$1" f
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    grep -qE -e "$NAME_PATTERN" <<<"$f" && emit "$prefix$f" "(敏感文件名)"
  done
}

worktree_scan() {
  echo "[secret-scan] 工作区..."
  local -a files=()
  local f
  while IFS= read -r -d '' f; do
    [ -f "$f" ] && files+=("$f")
  done < <("${GIT[@]}" ls-files -z --cached --others --exclude-standard)
  [ "${#files[@]}" -eq 0 ] && return 0
  printf '%s\n' "${files[@]}" | check_names ""
  scan_stream "" 1 < <(grep -nHIE "${PATTERN_ARGS[@]}" -- "${files[@]}" 2>/dev/null)
}

history_scan() {
  echo "[secret-scan] git 历史（$("${GIT[@]}" rev-list --all --count) 个提交）..."
  local rev
  while IFS= read -r rev; do
    scan_stream "$rev:" 1 < <("${GIT[@]}" grep -nIE "${PATTERN_ARGS[@]}" "$rev" -- 2>/dev/null)
  done < <("${GIT[@]}" rev-list --all)
  # 历史中出现过的敏感文件名
  "${GIT[@]}" log --all --name-only --pretty=format: | sort -u | check_names ""
}

staged_scan() {
  echo "[secret-scan] 暂存区..."
  local f
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    grep -qE -e "$NAME_PATTERN" <<<"$f" && emit "(staged) $f" "(敏感文件名)"
    scan_stream "(staged) $f:" 0 < <("${GIT[@]}" show ":$f" 2>/dev/null | grep -nIE "${PATTERN_ARGS[@]}" 2>/dev/null)
  done < <("${GIT[@]}" diff --cached --name-only --diff-filter=ACMR)
}

case "${1:-worktree}" in
  --history) history_scan ;;
  --staged)  staged_scan ;;
  --all)     worktree_scan; history_scan ;;
  worktree|--) worktree_scan ;;
  *) echo "用法: $0 [--history|--staged|--all]" >&2; exit 2 ;;
esac

echo "[secret-scan] 命中 $HITS 处"
[ "$HITS" -eq 0 ]
