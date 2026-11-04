#!/usr/bin/env bash
# 启用仓库自带 git hooks（pre-commit 密钥扫描 + commit-msg 格式检查）。
# 只改本地仓库配置（core.hooksPath），不动全局配置、不装任何依赖。
#
# 用法：scripts/install-hooks.sh
# 回退：git config --local --unset core.hooksPath
# 手动替代（不想改配置时）：
#   cp .githooks/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

[ -d .githooks ] || { echo "缺少 .githooks/ 目录" >&2; exit 1; }
chmod +x .githooks/* scripts/*.sh
git config --local core.hooksPath .githooks

echo "已启用：core.hooksPath=$(git config --local core.hooksPath)"
echo "  pre-commit  → scripts/secret-scan.sh --staged（密钥扫描）"
echo "  commit-msg  → type(scope): 说明 格式检查"
