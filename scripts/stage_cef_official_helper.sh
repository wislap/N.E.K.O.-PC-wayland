#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/target/debug/cef-official-helper"
SOURCE_CANDIDATES=(
  "/tmp/cef-sdk-build/tests/cefsimple_capi/Release"
  "${ROOT_DIR}/.cache/cef-sdk-sample-build/tests/cefsimple_capi/Release"
)
SDK_ARCHIVE_CANDIDATES=(
  "${ROOT_DIR}"/cef_binary_*.tar.bz2
)

SOURCE_DIR=""
for candidate in "${SOURCE_CANDIDATES[@]}"; do
  if [[ -x "${candidate}/cefsimple_capi" ]]; then
    SOURCE_DIR="${candidate}"
    break
  fi
done

if [[ -z "${SOURCE_DIR}" ]]; then
  echo "No built official CEF sample was found." >&2
  echo "Expected one of:" >&2
  printf '  %s\n' "${SOURCE_CANDIDATES[@]}" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

cp "${SOURCE_DIR}/cefsimple_capi" "${OUT_DIR}/cef-helper"

for name in \
  libcef.so \
  libEGL.so \
  libGLESv2.so \
  libvk_swiftshader.so \
  libvulkan.so.1 \
  chrome-sandbox \
  icudtl.dat \
  resources.pak \
  chrome_100_percent.pak \
  chrome_200_percent.pak \
  v8_context_snapshot.bin \
  vk_swiftshader_icd.json; do
  if [[ -e "${SOURCE_DIR}/${name}" ]]; then
    cp -a "${SOURCE_DIR}/${name}" "${OUT_DIR}/${name}"
  fi
done

rm -rf "${OUT_DIR}/locales"
mkdir -p "${OUT_DIR}/locales"

SDK_ARCHIVE=""
for candidate in "${SDK_ARCHIVE_CANDIDATES[@]}"; do
  if [[ -f "${candidate}" ]]; then
    SDK_ARCHIVE="${candidate}"
    break
  fi
done

if [[ -n "${SDK_ARCHIVE}" ]]; then
  ARCHIVE_ROOT="$(tar -tjf "${SDK_ARCHIVE}" | sed -n '1p' | cut -d/ -f1)"
  if [[ -n "${ARCHIVE_ROOT}" ]]; then
    tar -xjf "${SDK_ARCHIVE}" \
      -C "${OUT_DIR}/locales" \
      --wildcards \
      --strip-components=3 \
      "${ARCHIVE_ROOT}/Resources/locales/*"
  fi
fi

if [[ ! -f "${OUT_DIR}/locales/en-US.pak" ]]; then
  rm -rf "${OUT_DIR}/locales"
  cp -a "${SOURCE_DIR}/locales" "${OUT_DIR}/locales"
fi

if [[ ! -f "${OUT_DIR}/locales/en-US.pak" ]]; then
  echo "staged helper locales are unreadable (expected ${OUT_DIR}/locales/en-US.pak)" >&2
  exit 1
fi

echo "staged official helper into ${OUT_DIR}"
