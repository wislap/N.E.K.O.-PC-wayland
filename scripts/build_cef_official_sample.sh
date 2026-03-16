#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_DIR="${NEKO_CEF_SDK_DIR:-}"

if [[ -z "${SDK_DIR}" ]]; then
  echo "NEKO_CEF_SDK_DIR is not set" >&2
  exit 1
fi

BUILD_DIR="${ROOT_DIR}/.cache/cef-sdk-sample-build"
OUT_BIN="${ROOT_DIR}/target/debug/cef-official-sample"
SAMPLE_RELEASE_DIR="${BUILD_DIR}/tests/cefsimple_capi/Release"

mkdir -p "${BUILD_DIR}"
mkdir -p "${ROOT_DIR}/target/debug"

cmake -S "${SDK_DIR}" -B "${BUILD_DIR}" -DPROJECT_ARCH=x86_64 >/dev/null

if ! cmake --build "${BUILD_DIR}" --target cefsimple_capi -j"$(nproc)" >/dev/null; then
  mkdir -p "${SAMPLE_RELEASE_DIR}"
  rm -rf "${SAMPLE_RELEASE_DIR}/locales"
  cp -a "${SDK_DIR}/Resources/locales" "${SAMPLE_RELEASE_DIR}/locales"
  cmake --build "${BUILD_DIR}" --target cefsimple_capi -j"$(nproc)" >/dev/null
fi

cp "${BUILD_DIR}/tests/cefsimple_capi/Release/cefsimple_capi" "${OUT_BIN}"
echo "built ${OUT_BIN}"
