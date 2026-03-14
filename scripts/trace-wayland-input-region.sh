#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
trace_dir="${repo_root}/target/wayland-traces"
mkdir -p "${trace_dir}"

timestamp="$(date +%Y%m%d-%H%M%S)"
full_log="${trace_dir}/trace-${timestamp}.log"
focus_log="${trace_dir}/trace-${timestamp}.focus.log"

region="${NEKO_WAYLAND_DEBUG_INPUT_REGION:-0,0,220,320}"

echo "writing full Wayland trace to: ${full_log}"
echo "writing filtered surface trace to: ${focus_log}"
echo "debug input region: ${region}"

(
  cd "${repo_root}"
  env \
    WAYLAND_DEBUG=1 \
    NEKO_WAYLAND_TRACE_INPUT_REGION=1 \
    NEKO_WAYLAND_TRACE_WIDGET_TREE=1 \
    NEKO_WAYLAND_DEBUG_INPUT_REGION="${region}" \
    cargo run --bin neko-pc-wayland
) 2>&1 | tee "${full_log}"

rg \
  "set_input_region|create_region|wl_surface@|wl_region@|commit|GTK widget tree|applied raw Wayland input region|received frontend input region" \
  "${full_log}" > "${focus_log}" || true

echo
echo "filtered trace excerpt:"
sed -n '1,200p' "${focus_log}"
