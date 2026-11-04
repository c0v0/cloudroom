#!/usr/bin/env bash
# Reproducible x86_64-unknown-linux-musl release build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ACTION="${1:-all}"
case "$ACTION" in
  all|backend|frontend-server|frontend) ;;
  *)
    echo "用法: $0 [all|backend|frontend-server|frontend]" >&2
    exit 2
    ;;
esac
if [ "$#" -gt 1 ]; then
  echo "用法: $0 [all|backend|frontend-server|frontend]" >&2
  exit 2
fi

for tool in cargo rustup musl-gcc cmake; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "[build-musl] 缺少 $tool。安装 musl 构建依赖：" >&2
    echo "  sudo apt-get install -y --no-install-recommends musl-tools musl-dev cmake" >&2
    echo "  rustup target add x86_64-unknown-linux-musl" >&2
    exit 1
  fi
done
if ! rustup target list --installed 2>/dev/null | grep -qx 'x86_64-unknown-linux-musl'; then
  echo "[build-musl] 缺少 rustup target x86_64-unknown-linux-musl。" >&2
  echo "  rustup target add x86_64-unknown-linux-musl" >&2
  exit 1
fi

export CLOUDROOM_TARGET=x86_64-unknown-linux-musl
export CC_x86_64_unknown_linux_musl=musl-gcc
export AR_x86_64_unknown_linux_musl=ar
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
export CMAKE_C_COMPILER=musl-gcc
export CMAKE_AR=ar
export AWS_LC_SYS_PREBUILT_NASM=1
export CARGO_BUILD_JOBS=1
export CMAKE_BUILD_PARALLEL_LEVEL=1
export RUSTFLAGS="-C target-feature=+crt-static"

exec "$ROOT/scripts/build-release.sh" "$ACTION"
