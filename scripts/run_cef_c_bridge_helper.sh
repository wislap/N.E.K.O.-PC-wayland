#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_DIR="${NEKO_CEF_SDK_DIR:-${ROOT_DIR}/cef_binary_145.0.28+g51162e8+chromium-145.0.7632.160_linux64}"
URL="${1:-https://example.com}"

if [[ ! -d "${SDK_DIR}" ]]; then
  echo "CEF SDK dir not found: ${SDK_DIR}" >&2
  exit 1
fi

export NEKO_CEF_SDK_DIR="${SDK_DIR}"
export LD_LIBRARY_PATH="${SDK_DIR}/Release:${ROOT_DIR}/target/debug${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"

cd "${ROOT_DIR}"
cargo build --features cef_osr --bin cef-c-bridge-helper
exec ./target/debug/cef-c-bridge-helper "${URL}"
