#!/usr/bin/env bash
# 生成 release/ 下的运行产物（本地/测试环境一条命令；不碰 livekit-server 与部署模板）
#
#   release/cloudroom-server            ← backend（优先静态 musl）
#   release/cloudroom-frontend-server   ← frontend-server（优先静态 musl）
#   release/frontend/                   ← frontend 构建产物（与 frontend/dist 文件列表一致）
#
# 用法：
#   scripts/build-release.sh              # 全部
#   scripts/build-release.sh backend      # 只构建后端
#   CLOUDROOM_TARGET=x86_64-unknown-linux-musl scripts/build-release.sh   # 强制目标
#
# 生产发布流程（含 LiveKit、systemd、Nginx）见 release/deployment-zh.md。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET="${CLOUDROOM_TARGET:-}"
if [ -z "$TARGET" ] && command -v rustup >/dev/null 2>&1; then
  if rustup target list --installed 2>/dev/null | grep -qx 'x86_64-unknown-linux-musl'; then
    TARGET=x86_64-unknown-linux-musl
  fi
fi
if [ -n "$TARGET" ]; then
  echo "[build-release] 目标：$TARGET（静态）"
else
  echo "[build-release] 提示：未安装 musl target，回退 host target（二进制动态链接；部署请用 musl）"
  echo "              如需静态：rustup target add x86_64-unknown-linux-musl"
fi

TARGET_ARGS=()
[ -n "$TARGET" ] && TARGET_ARGS=(--target "$TARGET")
target_dir() { # $1= crate 目录
  local base="${CARGO_TARGET_DIR:-$1/target}"
  if [ -n "$TARGET" ]; then echo "$base/$TARGET/release"; else echo "$base/release"; fi
}

build_backend() {
  echo "[build-release] backend/cloudroom-server"
  ( cd backend && cargo build --release --locked "${TARGET_ARGS[@]}" --bin cloudroom-server )
  cp "$(target_dir backend)/cloudroom-server" release/cloudroom-server
}

build_frontend_server() {
  echo "[build-release] frontend-server/cloudroom-frontend-server"
  ( cd frontend-server && cargo build --release --locked "${TARGET_ARGS[@]}" --bin cloudroom-frontend-server )
  cp "$(target_dir frontend-server)/cloudroom-frontend-server" release/cloudroom-frontend-server
}

build_frontend() {
  echo "[build-release] frontend → release/frontend/"
  ( cd frontend
    [ -d node_modules ] || npm ci --no-audit --no-fund
    npm run build
  )
  rm -rf release/frontend
  cp -r frontend/dist release/frontend
}

ARTIFACTS=()
case "${1:-all}" in
  all)
    build_backend
    build_frontend_server
    build_frontend
    ARTIFACTS=(
      release/cloudroom-server
      release/cloudroom-frontend-server
      release/frontend/index.html
    )
    ;;
  backend)
    build_backend
    ARTIFACTS=(release/cloudroom-server)
    ;;
  frontend-server)
    build_frontend_server
    ARTIFACTS=(release/cloudroom-frontend-server)
    ;;
  frontend)
    build_frontend
    ARTIFACTS=(release/frontend/index.html)
    ;;
  *)
    echo "用法: $0 [all|backend|frontend-server|frontend]" >&2
    exit 2
    ;;
esac

echo "[build-release] 完成："
for artifact in "${ARTIFACTS[@]}"; do
  if [ ! -e "$artifact" ]; then
    echo "[build-release] 错误：缺少构建产物 $artifact" >&2
    exit 1
  fi
done
ls -lh "${ARTIFACTS[@]}" | awk '{print "  " $9 "  " $5}'
