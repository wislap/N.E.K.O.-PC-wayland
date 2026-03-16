#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_DIR="${NEKO_CEF_SDK_DIR:-}"
OUT_BIN="${ROOT_DIR}/target/debug/cef-c-standalone-helper"

if [[ -z "${SDK_DIR}" ]]; then
  echo "NEKO_CEF_SDK_DIR is not set" >&2
  exit 1
fi

if [[ ! -d "${SDK_DIR}/include" || ! -f "${SDK_DIR}/Release/libcef.so" ]]; then
  echo "invalid CEF SDK dir: ${SDK_DIR}" >&2
  exit 1
fi

mkdir -p "${ROOT_DIR}/target/debug"

cc \
  -std=c11 \
  -I"${SDK_DIR}" \
  -I"${SDK_DIR}/include" \
  "${ROOT_DIR}/src/cef/capi_probe.c" \
  "${ROOT_DIR}/src/cef/c_standalone_helper_main.c" \
  -L"${SDK_DIR}/Release" \
  -Wl,-rpath,"${SDK_DIR}/Release" \
  -Wl,-rpath,'$ORIGIN' \
  -lcef \
  -o "${OUT_BIN}"

echo "built ${OUT_BIN}"
