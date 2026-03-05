#!/usr/bin/env bash
set -euo pipefail

# 统一通过 Docker 构建 musl 目标平台二进制
# 用法:
#   ./docker/build-binary.sh [target] [docker_platform] [image]
# 示例:
#   ./docker/build-binary.sh x86_64-unknown-linux-musl linux/amd64 messense/rust-musl-cross:x86_64-musl

TARGET="${1:-x86_64-unknown-linux-musl}"
DOCKER_PLATFORM="${2:-linux/amd64}"
IMAGE="${3:-}"

PROJECT_NAME="bochk_check"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/dist/${TARGET}"
OUT_BIN="${OUT_DIR}/${PROJECT_NAME}"

if [[ "${TARGET}" != *"-musl" ]]; then
  echo "[error] 仅支持 musl 目标: ${TARGET}" >&2
  exit 1
fi

if [[ -z "${IMAGE}" ]]; then
  case "${TARGET}" in
    x86_64-unknown-linux-musl)
      IMAGE="messense/rust-musl-cross:x86_64-musl"
      ;;
    aarch64-unknown-linux-musl)
      IMAGE="messense/rust-musl-cross:aarch64-musl"
      ;;
    armv7-unknown-linux-musleabihf)
      IMAGE="messense/rust-musl-cross:armv7-musleabihf"
      ;;
    *)
      echo "[error] 未内置该 musl 目标镜像，请手动传入 image 参数: ${TARGET}" >&2
      exit 1
      ;;
  esac
fi

mkdir -p "${OUT_DIR}"

echo "[build] target=${TARGET}"
echo "[build] platform=${DOCKER_PLATFORM}"
echo "[build] image=${IMAGE}"

docker run --rm \
  --platform="${DOCKER_PLATFORM}" \
  -v "${ROOT_DIR}:/workspace" \
  -w /workspace \
  "${IMAGE}" \
  sh -lc "set -e; cargo build --release --target ${TARGET}; cp target/${TARGET}/release/${PROJECT_NAME} dist/${TARGET}/${PROJECT_NAME}; strip dist/${TARGET}/${PROJECT_NAME} || true"

echo "[ok] output=${OUT_BIN}"
