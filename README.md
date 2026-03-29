# N.E.K.O.-PC-wayland

Wayland-native desktop host prototype for `N.E.K.O`.

## Current scope

This project is the first Rust host skeleton for Linux Wayland. It intentionally starts small:

- discovers the sibling `N.E.K.O` repo
- starts `launcher.py`
- parses `NEKO_EVENT` messages to resolve the real runtime ports
- opens the resolved frontend URL through the experimental CEF OSR + raw Wayland host pipeline
- injects a minimal host/frontend IPC bridge as `window.NekoHost`
- keeps the Wayland strategy/input-region design scaffold for future native window work

It does **not** yet implement transparent desktop-pet behavior, input-region shaping, tray integration, or compositor-specific Wayland handling.

## Why this exists

The Electron shell in `N.E.K.O.-PC` works well on Windows because the transparent overlay + click-through model maps well to Win32 behavior. Under native Linux Wayland, that model is not reliable enough across GNOME, KDE, Hyprland, Sway, and niri.

This Rust host is the first step toward a better architecture:

- UI: keep the existing HTML/CSS/JS frontend
- Backend: still managed by the existing Python launcher
- Desktop host: move Linux windowing responsibility into a native Rust layer

## Requirements

- Rust toolchain
- local Linux CEF SDK/runtime via `NEKO_CEF_SDK_DIR`
- `N.E.K.O` repository available locally
- `uv` recommended, or a working `python3`

## Running

From this directory:

```bash
export NEKO_CEF_SDK_DIR=/path/to/cef_binary_xxx_linux64
cargo run
```

Or run the explicit binary target:

```bash
export NEKO_CEF_SDK_DIR=/path/to/cef_binary_xxx_linux64
cargo run --bin neko-pc-wayland
```

If repo auto-discovery fails:

```bash
NEKO_CEF_SDK_DIR=/path/to/cef_binary_xxx_linux64 \
NEKO_REPO_ROOT=/path/to/N.E.K.O \
cargo run
```

## Runtime Config

Primary environment variables:

- `NEKO_CEF_SDK_DIR`: required CEF SDK/runtime root
- `NEKO_REPO_ROOT`: optional explicit `N.E.K.O` repo path
- `NEKO_WAYLAND_HOST_MODE`: optional host mode, one of `raw_only`, `official_helper_probe`, `official_helper_run`, `c_standalone_probe`
- `NEKO_WAYLAND_FULLSCREEN`: enable fullscreen when set to `1`/`true`
- `NEKO_WAYLAND_WINDOW_WIDTH` / `NEKO_WAYLAND_WINDOW_HEIGHT`: host window size
- `NEKO_WAYLAND_RENDER_WIDTH` / `NEKO_WAYLAND_RENDER_HEIGHT`: frontend render size for CEF paths
- `NEKO_WAYLAND_RENDER_FPS`: target render fps, clamped by CEF limits
- `NEKO_WAYLAND_TRANSPARENT_BACKGROUND`: toggle transparent rendering
- `NEKO_WAYLAND_DISPLAY_ID` / `NEKO_WAYLAND_DISPLAY_INDEX` / `NEKO_WAYLAND_DISPLAY_NAME`: select target output
- `NEKO_WAYLAND_DEBUG_INPUT_REGION`: inject a debug input region
- `NEKO_WAYLAND_TRACE_INPUT_REGION`: verbose input-region logging

Default mode:

```bash
export NEKO_CEF_SDK_DIR=/path/to/cef_binary_xxx_linux64
cargo run
```

Standalone helper probe example:

```bash
export NEKO_CEF_SDK_DIR=/path/to/cef_binary_xxx_linux64
export NEKO_WAYLAND_HOST_MODE=c_standalone_probe
cargo run
```

## IPC bridge

The host injects `window.NekoHost` into the page.

Frontend -> host:

```js
window.NekoHost.postMessage({ cmd: 'ping', nonce: 1 })
window.NekoHost.postMessage({ cmd: 'get_host_info' })
window.NekoHost.postMessage({ cmd: 'log', message: 'hello from frontend' })
window.NekoHost.setInputRegion([
  { x: 120, y: 80, width: 320, height: 480 },
  { x: 420, y: 420, width: 360, height: 180 }
])
```

Host -> frontend:

```js
const off = window.NekoHost.addEventListener((payload) => {
  console.log('host event', payload)
})

window.addEventListener('neko-host-message', (event) => {
  console.log('host event via DOM', event.detail)
})
```

The host currently emits:

- `ready`
- `pong`
- `host_info`
- `log_ack`
- `input_region_applied`
- `error`

## What to build next

The next meaningful milestones are:

1. add a loading window and structured logs
2. add compositor detection
3. build a real Wayland-native input-region engine
4. route interactive region geometry over the new IPC bridge
